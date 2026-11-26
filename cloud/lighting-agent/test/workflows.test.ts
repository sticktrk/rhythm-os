import assert from "node:assert/strict";
import { test } from "node:test";
import { infer, turnSchema, validateSteps, type Turn } from "../src/planner.ts";

const room = "r_" + "1".repeat(24),
  sensor = "m_" + "2".repeat(24),
  scene = "c_" + "3".repeat(24),
  schedule = "s_" + "4".repeat(24);
const turn: Turn = {
  turn: crypto.randomUUID(),
  messages: [{ role: "user", text: "Help with my lights" }],
  targets: [
    { ref: room, tools: [], included: true },
    { ref: sensor, tools: ["start_motion_walk", "plan_motion_target"] },
    { ref: scene, tools: ["plan_home_scene_edit"] },
    { ref: schedule, tools: ["plan_light_schedule_edit"] },
  ],
};
const step = (
  target: string,
  operation: string,
  args: Record<string, number | string>,
) => ({ target, operation, arguments: args });

const draft = "d_" + "5".repeat(24);
const creation = "plan_generated_home_scene";
const draftTurn: Turn = {
  turn: crypto.randomUUID(),
  messages: [
    { role: "user", text: "Create a whole-home Northern Lights scene" },
  ],
  targets: [{ ref: draft, included: true, tools: [creation] }],
};
const palette = {
  color_1: "#0b8457",
  color_2: "#5F4B8B",
  brightness_percent: 55,
};

type ModelRequest = {
  input: { role: string; content: unknown }[];
};
function modelReply(
  decision: unknown,
  receive?: (body: ModelRequest) => void,
): typeof fetch {
  return async (_url, init) => {
    receive?.(JSON.parse(String(init?.body)));
    return Response.json({
      status: "completed",
      output: [
        {
          type: "message",
          role: "assistant",
          content: [{ type: "output_text", text: JSON.stringify(decision) }],
        },
      ],
    });
  };
}
const plan = (provider: typeof fetch, turn: Turn, photo?: string) =>
  infer({ OPENAI_API_KEY: "test-only" }, turn, photo, provider);

test("the OpenAI planner sends bounded context and reads the final answer after reasoning", async () => {
  const request: Turn = {
    ...turn,
    messages: [
      { role: "user", text: "Help me choose a warmer color." },
      { role: "assistant", text: "What color is the wall?" },
      { role: "user", text: "" },
    ],
  };
  const answer = { text: "Amber would complement that wall.", steps: [] };
  const message = (text: string, phase: string) => ({
    type: "message",
    role: "assistant",
    phase,
    content: [{ type: "output_text", text }],
  });
  const provider: typeof fetch = async (url, init) => {
    assert.equal(url, "https://api.openai.com/v1/responses");
    assert.equal(init?.method, "POST");
    assert.equal(init?.redirect, "manual");
    assert.equal(
      new Headers(init?.headers).get("authorization"),
      "Bearer test-only",
    );
    assert.ok(init?.signal);
    const body = JSON.parse(String(init.body));
    assert.equal(body.model, "gpt-6.2-sol");
    assert.deepEqual(body.reasoning, { effort: "low" });
    assert.equal(body.store, false);
    assert.deepEqual(body.text.format, { type: "json_object" });
    assert.ok(body.max_output_tokens > 0);
    assert.equal(body.input[0].role, "developer");
    assert.match(body.input[0].content, /JSON/);
    assert.deepEqual(body.input.slice(1), [
      { role: "user", content: request.messages[0].text },
      { role: "assistant", content: request.messages[1].text },
      {
        role: "user",
        content: [
          {
            type: "input_text",
            text: "Here is the photo for our conversation.",
          },
          {
            type: "input_image",
            image_url: "data:image/jpeg;base64,fixture",
            detail: "auto",
          },
        ],
      },
    ]);
    assert.doesNotMatch(String(init.body), /test-only|previous_response_id/);
    return Response.json({
      status: "completed",
      output: [
        { type: "reasoning", summary: [] },
        message("Looking at the wall.", "commentary"),
        message(JSON.stringify(answer), "final_answer"),
      ],
    });
  };
  assert.deepEqual(await plan(provider, request, "fixture"), answer);
});

test("an incomplete, refused, or malformed provider response cannot yield a decision", async () => {
  const complete = {
    type: "message",
    role: "assistant",
    content: [{ type: "output_text", text: '{"text":"Review","steps":[]}' }],
  };
  for (const envelope of [
    { status: "incomplete", output: [complete] },
    { status: "completed", output: [{ ...complete, status: "incomplete" }] },
    { status: "completed", output: [{ type: "reasoning" }] },
    { status: "completed", output: [complete, complete] },
    {
      status: "completed",
      output: [{ ...complete, content: [{ type: "refusal" }] }],
    },
    {
      status: "completed",
      output: [
        { ...complete, content: [{ type: "output_text", text: "not json" }] },
      ],
    },
  ]) {
    await assert.rejects(
      plan(async () => Response.json(envelope), turn),
      /invalid_proposal/,
    );
  }
  let calls = 0;
  const unavailable: typeof fetch = async () => {
    calls++;
    return new Response("sensitive provider details", { status: 429 });
  };
  await assert.rejects(infer({}, turn, undefined, unavailable), {
    message: "inference_unavailable",
  });
  assert.equal(calls, 0);
  await assert.rejects(plan(unavailable, turn), {
    message: "inference_unavailable",
  });
  assert.equal(calls, 1);
});

test("a phone can resolve an inferred room before a lighting proposal", async () => {
  const otherRoom = "r_" + "9".repeat(24);
  const query = "Dim the lights where I cook";
  const previous = {
    ref: room,
    tools: ["adjust_room_brightness"],
    included: true,
  };
  const request = turnSchema.parse({
    ...turn,
    room_lookup: true,
    selected: room,
    targets: [previous, { ref: otherRoom, tools: [], included: false }],
    messages: [{ role: "user", text: query }],
  });
  let systemText = "";
  const answer = (response: unknown) =>
    modelReply(response, ({ input }) => {
      systemText = String(input[0].content);
    });
  for (const room_query of ["kitchen", ""]) {
    const decision = await plan(
      answer({ text: "", steps: [], room_query }),
      request,
    );
    assert.deepEqual(decision, { text: "", steps: [], room_query });
    assert.match(systemText, /new room takes precedence/);
    assert.ok(!systemText.includes(otherRoom));
  }
  const dim = step(room, "adjust_room_brightness", { delta_percent: -10 });
  const clarify = answer({ text: "Which room?", steps: [], room_query: "" });
  const uncertain = {
    ...request,
    messages: [{ role: "user" as const, text: "Dim the room where I relax." }],
  };
  assert.deepEqual(await plan(clarify, uncertain), {
    text: "Which room?",
    steps: [],
    room_query: "",
  });
  assert.match(systemText, /clues conflict, point to several plausible rooms/);
  assert.match(systemText, /Never guess a room type or fall back to selected/);

  const unscoped = {
    ...request,
    messages: [{ role: "user" as const, text: "Dim the lights." }],
    targets: [previous],
  };
  for (const candidate of [unscoped, { ...request, sole_room: true }]) {
    await plan(clarify, candidate);
    assert.doesNotMatch(systemText, /home's only room/);
  }
  assert.deepEqual(
    (
      await plan(
        answer({ text: "Review this change.", steps: [dim] }),
        turnSchema.parse({ ...unscoped, sole_room: true }),
      )
    ).steps,
    [dim],
  );
  assert.match(systemText, /phone confirmed selected is the home's only room/);
  assert.match(systemText, /Still look up any new location/);
  await assert.rejects(
    plan(answer({ text: "Review.", steps: [dim] }), {
      ...unscoped,
      targets: [{ ...previous, included: false }],
    }),
    /invalid_proposal/,
  );
  const grounded = { ...request, room_lookup: false };
  assert.deepEqual(
    (
      await plan(
        answer({ text: "Review this change.", steps: [dim] }),
        grounded,
      )
    ).steps,
    [dim],
  );
  assert.match(systemText, /phone matched the room/);
  assert.doesNotMatch(systemText, /room_query/);

  for (const invalid of [
    { text: "Review", steps: [dim], room_query: "kitchen" },
    { text: "", steps: [], room_query: "x".repeat(81) },
    { text: "", steps: [], room_query: null },
  ]) {
    await assert.rejects(plan(answer(invalid), request));
  }

  for (const candidate of [
    { ...request, room_lookup: undefined },
    grounded,
    { ...request, messages: [{ role: "user" as const, text: "Dim " + room }] },
    { ...request, targets: [previous, { ...previous, ref: otherRoom }] },
    {
      ...request,
      motion: {
        sensor,
        room,
        stage: "waiting" as const,
        fresh: 0,
        detected: false,
        missing_target: false,
      },
    },
  ]) {
    assert.deepEqual(await plan(clarify, candidate), {
      text: "Which room?",
      steps: [],
    });
    assert.doesNotMatch(systemText, /room_query/);
  }
  assert.deepEqual(await plan(clarify, request, "photo"), {
    text: "Which room?",
    steps: [],
  });
});

test("a whole-home scene design needs only a palette from the model", async () => {
  const proposal = step(draft, creation, palette);
  const ai = modelReply(
    { text: "Emerald and violet at a soft brightness.", palette },
    ({ input }) => {
      const instructions = String(input[0].content);
      assert.match(instructions, /Make a scene/);
      assert.match(instructions, /color_1/);
      assert.doesNotMatch(
        instructions,
        /plan_generated_home_scene|plan_home_scene_edit|adjust_room_warmth|d_[a-f0-9]{24}/,
      );
      assert.equal(input.at(-1)?.content, draftTurn.messages[0].text);
    },
  );
  assert.deepEqual((await plan(ai, turnSchema.parse(draftTurn))).steps, [
    proposal,
  ]);

  const clarify = modelReply({
    text: "Which colors do you have in mind?",
    palette: null,
  });
  assert.deepEqual((await plan(clarify, draftTurn)).steps, []);
  const uncaptioned = {
    ...draftTurn,
    messages: [{ role: "user" as const, text: "" }],
  };
  const proposesDraft = modelReply({ text: "I see colorful lights.", palette });
  assert.deepEqual((await plan(proposesDraft, uncaptioned, "photo")).steps, []);
  await assert.rejects(plan(proposesDraft, uncaptioned), /message_required/);
  for (const response of [
    { text: "A palette", steps: [proposal] },
    { text: "A palette", palette, target: scene },
    { text: "A palette", palette: { ...palette, brightness_percent: 120 } },
  ]) {
    await assert.rejects(plan(modelReply(response), draftTurn));
  }
});

test("generated proposals contain only two to four consecutive hex colors and bounded brightness", () => {
  const validate = (args: Record<string, string | number>) =>
    validateSteps([step(draft, creation, args)], draftTurn);
  for (const args of [
    { ...palette, brightness_percent: 1 },
    { ...palette, color_3: "#00FF99", brightness_percent: 100 },
    { ...palette, color_3: "#00FF99", color_4: "#7788ff" },
  ])
    assert.deepEqual(validate(args), [step(draft, creation, args)]);
  for (const args of [
    { color_1: "#112233", brightness_percent: 50 },
    { ...palette, color_2: "#000" },
    { ...palette, color_3: "orange" },
    { ...palette, color_4: "#ABCDEF" },
    { ...palette, name: "My theme" },
    { ...palette, brightness_percent: 50.5 },
    { ...palette, brightness_percent: 0 },
    { ...palette, brightness_percent: 101 },
  ])
    assert.throws(() => validate(args));

  const [target] = draftTurn.targets;
  for (const targets of [
    [{ ...target, included: false }],
    [{ ...target, ref: scene }],
    [{ ...target, tools: [] }],
    [{ ...target, tools: [creation, "plan_home_scene_edit"] }],
    [target, { ref: room, tools: ["control_room_power"] }],
  ])
    assert.throws(() =>
      validateSteps([step(targets[0].ref, creation, palette)], {
        ...draftTurn,
        targets,
      }),
    );
});

test("an explicitly chosen whole-home scene creates a review without inference", async () => {
  let calls = 0;
  const ai = modelReply({ text: "Lighting advice", steps: [] }, () => {
    calls++;
  });
  const choice = {
    ref: scene,
    tools: ["plan_home_scene_edit"],
    included: true,
  };
  const message = `Apply ${scene} to the whole home.`;
  const request = (text: string, targets: Turn["targets"], photo?: string) =>
    plan(ai, { ...turn, messages: [{ role: "user", text }], targets }, photo);

  const review = await request(message, [{ ref: room, tools: [] }, choice]);
  assert.deepEqual(review.steps, [step(scene, "plan_home_scene_edit", {})]);
  assert.equal(calls, 0);

  for (const result of [
    await request(message, [choice], "photo"),
    await request(message + " Except the hall.", [choice]),
    await request(message, [{ ...choice, ref: "c_" + "9".repeat(24) }]),
    await request(message, [{ ...choice, included: false }]),
    await request(message, [{ ...choice, tools: [] }]),
    await request(message, [{ ...choice, tools: [...choice.tools, "other"] }]),
    await request(message, [
      choice,
      { ref: room, tools: ["control_room_power"] },
    ]),
  ]) {
    assert.equal(result.text, "Lighting advice");
    assert.deepEqual(result.steps, []);
  }
  assert.equal(calls, 7);
});

test("motion repair requires a walk that identified the missing intended target", () => {
  const repair = step(sensor, "plan_motion_target", { room });
  assert.throws(() => validateSteps([repair], turn));
  const walked = {
    ...turn,
    motion: {
      sensor,
      room,
      detected: true,
      missing_target: true,
      stage: "detected" as const,
      fresh: 0,
    },
  };
  assert.deepEqual(validateSteps([repair], walked), [repair]);
  assert.throws(() =>
    validateSteps([repair], {
      ...walked,
      targets: walked.targets.map((t) => ({ ...t, included: false })),
    }),
  );
  assert.throws(() =>
    validateSteps([repair], {
      ...walked,
      motion: { ...walked.motion, missing_target: false },
    }),
  );
});
test("saved settings require a named, offered capability and a bounded edit", () => {
  const shift = step(schedule, "plan_light_schedule_edit", {
    delta_minutes: -15,
    transition: "sleep",
  });
  assert.deepEqual(validateSteps([shift], turn), [shift]);
  assert.throws(() =>
    validateSteps(
      [{ ...shift, arguments: { delta_minutes: -800, transition: "all" } }],
      turn,
    ),
  );
  assert.throws(() =>
    validateSteps([shift, step(scene, "plan_home_scene_edit", {})], turn),
  );
  assert.throws(() =>
    validateSteps([step(room, "plan_home_scene_edit", {})], turn),
  );
});

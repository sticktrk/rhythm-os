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
    ({
      run: async (
        _model: string,
        input: { messages: { content: string }[] },
      ) => {
        systemText = input.messages[0].content;
        return { response };
      },
    }) as unknown as Ai;
  for (const room_query of ["kitchen", ""]) {
    const decision = await infer(
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
  assert.deepEqual(await infer(clarify, uncertain), {
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
    await infer(clarify, candidate);
    assert.doesNotMatch(systemText, /home's only room/);
  }
  assert.deepEqual(
    (
      await infer(
        answer({ text: "Review this change.", steps: [dim] }),
        turnSchema.parse({ ...unscoped, sole_room: true }),
      )
    ).steps,
    [dim],
  );
  assert.match(systemText, /phone confirmed selected is the home's only room/);
  assert.match(systemText, /Still look up any new location/);
  await assert.rejects(
    infer(answer({ text: "Review.", steps: [dim] }), {
      ...unscoped,
      targets: [{ ...previous, included: false }],
    }),
    /invalid_proposal/,
  );
  const grounded = { ...request, room_lookup: false };
  assert.deepEqual(
    (
      await infer(
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
    await assert.rejects(infer(answer(invalid), request));
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
    assert.deepEqual(await infer(clarify, candidate), {
      text: "Which room?",
      steps: [],
    });
    assert.doesNotMatch(systemText, /room_query/);
  }
  assert.deepEqual(await infer(clarify, request, "photo"), {
    text: "Which room?",
    steps: [],
  });
});

test("a whole-home scene design needs only a palette from the model", async () => {
  const proposal = step(draft, creation, palette);
  const ai = {
    run: async (_model: string, input: { messages: { content: string }[] }) => {
      assert.match(input.messages[0].content, /Make a scene/);
      assert.match(input.messages[0].content, /color_1/);
      assert.doesNotMatch(
        input.messages[0].content,
        /plan_generated_home_scene|plan_home_scene_edit|adjust_room_warmth|d_[a-f0-9]{24}/,
      );
      assert.equal(input.messages.at(-1)?.content, draftTurn.messages[0].text);
      return {
        response: {
          text: "Emerald and violet at a soft brightness.",
          palette,
        },
      };
    },
  } as unknown as Ai;
  assert.deepEqual((await infer(ai, turnSchema.parse(draftTurn))).steps, [
    proposal,
  ]);

  const clarify = {
    run: async () => ({
      response: { text: "Which colors do you have in mind?", palette: null },
    }),
  } as unknown as Ai;
  assert.deepEqual((await infer(clarify, draftTurn)).steps, []);
  const uncaptioned = {
    ...draftTurn,
    messages: [{ role: "user" as const, text: "" }],
  };
  const proposesDraft = {
    run: async () => ({
      response: { text: "I see colorful lights.", palette },
    }),
  } as unknown as Ai;
  assert.deepEqual(
    (await infer(proposesDraft, uncaptioned, "photo")).steps,
    [],
  );
  await assert.rejects(infer(proposesDraft, uncaptioned), /message_required/);
  for (const response of [
    { text: "A palette", steps: [proposal] },
    { text: "A palette", palette, target: scene },
    { text: "A palette", palette: { ...palette, brightness_percent: 120 } },
  ]) {
    const invalid = { run: async () => ({ response }) } as unknown as Ai;
    await assert.rejects(infer(invalid, draftTurn));
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
  const ai = {
    run: async () => {
      calls++;
      return { response: { text: "Lighting advice", steps: [] } };
    },
  } as unknown as Ai;
  const choice = {
    ref: scene,
    tools: ["plan_home_scene_edit"],
    included: true,
  };
  const message = `Apply ${scene} to the whole home.`;
  const request = (text: string, targets: Turn["targets"], photo?: string) =>
    infer(ai, { ...turn, messages: [{ role: "user", text }], targets }, photo);

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

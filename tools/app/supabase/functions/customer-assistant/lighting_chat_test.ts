import { lightingChat } from "./lighting_chat.ts";
import { Rejection } from "./contract.ts";

const input = {
  messages: [{ role: "user", text: "make it cozier" }],
  tools: ["adjust_room_warmth"],
  room: {
    canonical_kelvin: 3300,
    name: "PRIVATE ROOM",
    credential: "PRIVATE KEY",
    mode: "PRIVATE MODE",
    light_readback: ["PRIVATE LIGHT"],
  },
};
const config = { key: "fixture-key", model: "fixture-model" };
const reserve = () => Promise.resolve(true);
const proposal = {
  type: "function_call",
  name: "adjust_room_warmth",
  arguments: '{"kelvin":3000}',
};
const reasoning = { type: "reasoning", summary: [] };
async function reject(p: Promise<unknown>, expected: number) {
  try {
    await p;
  } catch (e) {
    if (e instanceof Rejection && e.status === expected) return;
    throw e;
  }
  throw Error("Expected rejection " + expected);
}
Deno.test("sends only selected capabilities and projected readings and returns an unexecuted proposal", async () => {
  const reply = await lightingChat(
    input,
    reserve,
    { key: config.key },
    async (url, init) => {
      const req = new Request(url, init);
      const text = await req.text(), body = JSON.parse(text);
      if (
        req.url !== "https://api.openai.com/v1/responses" ||
        req.redirect !== "error" ||
        req.headers.get("authorization") !== "Bearer fixture-key" ||
        body.model !== "gpt-6.2-sol" || body.reasoning?.effort !== "low" ||
        body.max_output_tokens !== 2048 || body.store !== false ||
        body.parallel_tool_calls !== false ||
        /PRIVATE|fixture-key|room_id/.test(text) ||
        !text.includes("3300") || body.tools.length !== 1 ||
        body.tools[0].name !== proposal.name
      ) throw Error();
      return Response.json({
        status: "completed",
        output: [reasoning, proposal],
      });
    },
  );
  if (
    JSON.stringify(reply) !==
      JSON.stringify({ operation: proposal.name, arguments: { kelvin: 3000 } })
  ) throw Error();
});
Deno.test("rejects excess usage, missing configuration and unsupported client requests before inference", async () => {
  const noFetch = () => {
    throw Error("unexpected provider call");
  };
  await reject(lightingChat(input, reserve, { model: "test" }, noFetch), 503);
  await reject(
    lightingChat(input, () => Promise.resolve(false), config, noFetch),
    429,
  );
  await reject(
    lightingChat(
      { ...input, tools: ["apply_customer_plan"] },
      reserve,
      config,
      noFetch,
    ),
    400,
  );
  await reject(
    lightingChat(
      { ...input, messages: [{ role: "developer", text: "unsafe" }] },
      reserve,
      config,
      noFetch,
    ),
    400,
  );
});
Deno.test("rejects unoffered operations, mixed targets, bad values and duplicate calls", async () => {
  for (
    const output of [
      [{ ...proposal, name: "set_room_color" }],
      [{ ...proposal, arguments: '{"kelvin":3000,"room_id":"other"}' }],
      [{ ...proposal, arguments: '{"kelvin":0}' }],
      [proposal, proposal],
      [{ ...proposal, status: "incomplete" }],
      [{
        type: "message",
        content: [{ type: "refusal", refusal: "Declined" }],
      }, proposal],
    ]
  ) {
    await reject(
      lightingChat(
        input,
        reserve,
        config,
        () => Promise.resolve(Response.json({ status: "completed", output })),
      ),
      502,
    );
  }
});
Deno.test("preserves model overrides and returns an ordinary conversational follow-up", async () => {
  const reply = await lightingChat(
    input,
    reserve,
    config,
    async (url, init) => {
      const body = await new Request(url, init).json();
      if (body.model !== config.model || "reasoning" in body) throw Error();
      return Response.json({
        status: "completed",
        output: [reasoning, {
          type: "message",
          content: [{
            type: "output_text",
            text: "Would you like it warmer or dimmer?",
          }],
        }],
      });
    },
  );
  if (reply.text !== "Would you like it warmer or dimmer?") throw Error();
});

Deno.test("an incomplete response cannot propose a lighting change", async () => {
  await reject(
    lightingChat(input, reserve, config, () =>
      Promise.resolve(Response.json({
        status: "incomplete",
        incomplete_details: { reason: "max_output_tokens" },
        output: [reasoning, proposal],
      }))),
    502,
  );
});

Deno.test("returns only completed final answers and accepts earlier model responses without phase", async () => {
  const message = (
    text: string,
    phase?: string | null,
    status = "completed",
  ) => ({
    type: "message",
    phase,
    status,
    content: [{ type: "output_text", text }],
  });
  const invoke = (output: unknown[]) =>
    lightingChat(
      input,
      reserve,
      config,
      () => Promise.resolve(Response.json({ status: "completed", output })),
    );
  const commentary = message("Looking up the room.", "commentary");
  for (const phase of ["final_answer", null, undefined]) {
    const reply = await invoke([commentary, message("Target is 50%.", phase)]);
    if (reply.text !== "Target is 50%.") throw Error();
  }
  await reject(invoke([commentary]), 502);
  await reject(
    invoke([message("The lights", "final_answer", "incomplete")]),
    502,
  );
});

Deno.test("a conversation can start without room data or mutation tools", async () => {
  const turn = { ...input, room: {}, tools: [], scope: "unresolved" };
  for (const reason of ["list", "clarify"]) {
    const reply = await lightingChat(
      turn,
      reserve,
      config,
      async (_url, init) => {
        const request = JSON.parse(String(init?.body));
        if (
          request.tools.length !== 1 || request.tools[0].name !== "show_rooms"
        ) throw Error();
        return Response.json({
          status: "completed",
          output: [{
            type: "function_call",
            name: "show_rooms",
            arguments: JSON.stringify({ reason }),
          }],
        });
      },
    );
    if (reply.rooms !== reason) throw Error();
  }
  await reject(
    lightingChat(turn, reserve, config, () =>
      Promise.resolve(
        Response.json({ status: "completed", output: [proposal] }),
      )),
    502,
  );
  await reject(
    lightingChat({ ...input, scope: "unresolved" }, reserve, config),
    400,
  );
});

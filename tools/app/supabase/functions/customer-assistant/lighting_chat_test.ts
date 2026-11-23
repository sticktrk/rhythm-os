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
    config,
    async (url, init) => {
      const req = new Request(url, init);
      const text = await req.text(), body = JSON.parse(text);
      if (
        req.url !== "https://api.openai.com/v1/responses" ||
        req.headers.get("authorization") !== "Bearer fixture-key" ||
        body.store !== false || /PRIVATE|fixture-key|room_id/.test(text) ||
        !text.includes("3300") || body.tools.length !== 1 ||
        body.tools[0].name !== proposal.name
      ) throw Error();
      return Response.json({ status: "completed", output: [proposal] });
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
Deno.test("returns an ordinary conversational follow-up with no mutation", async () => {
  const reply = await lightingChat(
    input,
    reserve,
    config,
    () =>
      Promise.resolve(Response.json({
        status: "completed",
        output: [{
          type: "message",
          content: [{
            type: "output_text",
            text: "Would you like it warmer or dimmer?",
          }],
        }],
      })),
  );
  if (reply.text !== "Would you like it warmer or dimmer?") throw Error();
});

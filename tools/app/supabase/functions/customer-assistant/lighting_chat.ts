import { lightingModelSettings } from "../_shared/lighting_model.ts";
import { type Fields, object, Rejection } from "./contract.ts";

// These are the room-scoped MCP tools the mobile preview can render and
// confirm. The phone intersects this list with tools/list from its live hub.
import { lightingTools as specs } from "./lighting_tools.ts";
const instructions = [
  "You are Rhythm's lighting assistant. Answer briefly using ordinary lighting language.",
  "Use the supplied hub reading as evidence. Its brightness and Kelvin are targets. Never claim to see a physical result.",
  "Use an available tool to propose one concrete adjustment for an explicit request. The resident confirms on their phone.",
  "If the goal is unclear, ask one short question or offer two distinct lighting options. Never say a proposed change happened.",
  "For lasting profiles, schedules, motion or account setup, point to Guided tools. Stick to lighting.",
].join(" ");
type Tool = keyof typeof specs;
function tool(value: unknown): value is Tool {
  return typeof value === "string" && Object.hasOwn(specs, value);
}
export async function lightingChat(
  input: unknown,
  reserve: () => Promise<boolean>,
  config: { key?: string; model?: string },
  request: typeof fetch = fetch,
) {
  const invalid = () => new Rejection(400, "invalid_chat_turn");
  if (
    !object(input) || !Array.isArray(input.messages) ||
    input.messages.length < 1 ||
    input.messages.length > 8 || !Array.isArray(input.tools) ||
    input.tools.length > 5 || !input.tools.every(tool) || !object(input.room)
  ) throw invalid();
  const messages = input.messages.map((m) => {
    if (
      !object(m) || (m.role !== "user" && m.role !== "assistant") ||
      typeof m.text !== "string" || !m.text.trim() || m.text.length > 1000
    ) throw invalid();
    return { role: m.role, content: m.text };
  });
  if (messages.at(-1)?.role !== "user") throw invalid();
  // Older preview builds explicitly select a room. New builds start in chat
  // and resolve names on the phone, where the hub inventory stays private.
  const scope = input.scope ?? "selected";
  if (
    typeof scope !== "string" ||
    !["selected", "unresolved", "mentioned", "continuing", "clarified"]
      .includes(scope)
  ) throw invalid();
  if (
    scope === "unresolved" &&
    (input.tools.length || Object.keys(input.room).length)
  ) throw invalid();
  const reading: Fields = {};
  for (const [k, v] of Object.entries(input.room)) {
    if (
      ([
        "canonical_brightness",
        "canonical_kelvin",
        "time_offset_minutes",
        "brightness_offset",
      ].includes(k) &&
        typeof v === "number" && Number.isFinite(v)) ||
      (["canonical_power", "adaptation_enabled"].includes(k) &&
        typeof v === "boolean") ||
      (["mode", "state"].includes(k) &&
        ["day", "night", "sleep", "active", "off", "low_glow", "mood"].includes(
          v as string,
        ) && typeof v === "string")
    ) {
      reading[k] = v;
    }
  }
  const offered = [...new Set(input.tools as Tool[])];
  if (!config.key) throw new Rejection(503, "chat_unavailable");
  if (!await reserve()) throw new Rejection(429, "chat_daily_limit");
  const tools: Fields[] = offered.map((name) => ({
    type: "function",
    name,
    description: specs[name].description,
    strict: true,
    parameters: {
      type: "object",
      properties: specs[name].fields,
      required: Object.keys(specs[name].fields),
      additionalProperties: false,
    },
  }));
  if (scope !== "selected") {
    tools.push({
      type: "function",
      name: "show_rooms",
      description:
        "Display the actual rooms on the phone. Use list when asked which rooms exist. Use clarify when a room-specific request has no resolved room, names an unknown room, or needs more than one room. The phone offers the choices in the conversation.",
      strict: true,
      parameters: {
        type: "object",
        properties: { reason: { type: "string", enum: ["list", "clarify"] } },
        required: ["reason"],
        additionalProperties: false,
      },
    });
  }
  const context = scope === "selected"
    ? "The resident picked one room in the app. All actions affect only that room. If they name another room, ask them to select it first."
    : [
      "Start with the resident's request. Answer general lighting questions directly without asking for a room.",
      "For live status or an adjustment, use the phone's room context. If no room is resolved, call show_rooms with clarify. Never invent room names or readings.",
      "One proposal affects one room. Use show_rooms to clarify a multi-room request. Use list when asked which rooms exist.",
      scope === "continuing"
        ? "The reading is for the previous room. If the latest message names another room, it did not match on this phone; clarify instead of acting on the previous room."
        : scope === "mentioned"
        ? "The phone matched a room named in the latest request."
        : scope === "clarified"
        ? "The resident has answered which room to use. Address their outstanding request using the supplied reading."
        : "There is no resolved room or live reading.",
    ].join(" ");
  let data;
  try {
    const res = await request("https://api.openai.com/v1/responses", {
      method: "POST",
      redirect: "error",
      headers: {
        authorization: "Bearer " + config.key,
        "content-type": "application/json",
      },
      signal: AbortSignal.timeout(30000),
      body: JSON.stringify({
        ...lightingModelSettings(config.model),
        input: [{
          role: "developer",
          content: instructions + " " + context +
            "\nCurrent room reading: " + JSON.stringify(reading),
        }, ...messages],
        tools,
        parallel_tool_calls: false,
        // The Responses API counts reasoning and visible output in this limit.
        max_output_tokens: 2048,
        store: false,
      }),
    });
    if (!res.ok) throw Error();
    data = await res.json();
  } catch {
    throw new Rejection(503, "chat_unavailable");
  }
  const invalidReply = () => new Rejection(502, "invalid_chat_reply");
  if (
    !object(data) || data.status !== "completed" || !Array.isArray(data.output)
  ) throw invalidReply();
  if (
    data.output.some((item) =>
      object(item) && item.type === "message" && Array.isArray(item.content) &&
      item.content.some((part: unknown) =>
        object(part) && part.type === "refusal"
      )
    )
  ) throw invalidReply();
  const calls = data.output.filter((x) =>
    object(x) && x.type === "function_call"
  );
  if (calls.length) {
    const { name, arguments: raw } = calls[0];
    if (
      calls.length !== 1 ||
      (calls[0].status !== undefined && calls[0].status !== "completed") ||
      !tools.some((t) => t.name === name) ||
      typeof raw !== "string"
    ) throw invalidReply();
    let args;
    try {
      args = JSON.parse(raw);
    } catch {
      throw invalidReply();
    }
    if (name === "show_rooms") {
      if (
        !object(args) || Object.keys(args).length !== 1 ||
        (args.reason !== "list" && args.reason !== "clarify")
      ) throw invalidReply();
      return { rooms: args.reason };
    }
    if (!tool(name)) throw invalidReply();
    const fields = specs[name].fields;
    if (
      !object(args) || Object.keys(args).length !== Object.keys(fields).length
    ) throw invalidReply();
    for (const [key, schema] of Object.entries(fields)) {
      const v = args[key];
      if (
        schema.type === "integer"
          ? typeof v !== "number" || !Number.isInteger(v) ||
            v < schema.minimum || v > schema.maximum
          : !schema.enum.some((item: string) => item === v)
      ) throw invalidReply();
    }
    return { operation: name, arguments: args };
  }
  const answers = data.output.filter((item) =>
    object(item) && item.type === "message" && Array.isArray(item.content) &&
    (item.phase == null || item.phase === "final_answer")
  );
  if (
    answers.some((item) =>
      item.status !== undefined && item.status !== "completed"
    )
  ) throw invalidReply();
  const text = answers
    .flatMap((x) => x.content).filter((x) =>
      object(x) && x.type === "output_text" && typeof x.text === "string"
    )
    .map((x) => x.text.trim()).join(" ").slice(0, 1000);
  if (!text) throw invalidReply();
  return { text };
}

import { getAgentByName } from "agents";
import { Buffer } from "node:buffer";
import jpeg from "jpeg-js";
import { z } from "zod";
import { ConversationIndex, LightingConversation } from "./conversation.ts";
import { Failure, id, infer, turnSchema } from "./planner.ts";

export { ConversationIndex, LightingConversation };
export interface Env {
  CONVERSATIONS: DurableObjectNamespace<LightingConversation>;
  RESIDENT_INDEX: DurableObjectNamespace<ConversationIndex>;
  AI: Ai;
  SUPABASE_URL: string;
  SUPABASE_PUBLISHABLE_KEY: string;
  ACCEPT_NEW_TURNS: string;
  PILOT_PERCENT?: string;
  PILOT_RESIDENTS?: string;
}
const json = (value: unknown, status = 200) =>
  Response.json(value, {
    status,
    headers: {
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
    },
  });
async function bounded(req: Request, max: number) {
  if (Number(req.headers.get("content-length")) > max || !req.body)
    throw new Failure(413, "invalid_size");
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let count = 0;
  try {
    for (;;) {
      const item = await reader.read();
      if (item.done) break;
      count += item.value.length;
      if (count > max) {
        await reader.cancel();
        throw new Failure(413, "invalid_size");
      }
      chunks.push(item.value);
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks, count);
}
async function access(
  env: Env,
  bearer: string,
  home: string,
  instance: string,
  action: string,
) {
  const res = await fetch(
    new URL("/functions/v1/lighting-agent-access", env.SUPABASE_URL),
    {
      method: "POST",
      redirect: "manual",
      cache: "no-store",
      headers: {
        authorization: bearer,
        apikey: env.SUPABASE_PUBLISHABLE_KEY,
        "content-type": "application/json",
      },
      body: JSON.stringify({ home, instance, action }),
      signal: AbortSignal.timeout(10000),
    },
  );
  if (!res.ok)
    throw new Failure(
      [401, 403, 429].includes(res.status) ? res.status : 503,
      "account_unavailable",
    );
  return z.object({ actor: id, hub: id.optional() }).parse(await res.json());
}
export function normalizedPhoto(raw: Uint8Array): string {
  if (raw.length < 4 || raw[0] !== 255 || raw[1] !== 216)
    throw new Failure(400, "invalid_photo");
  try {
    const pixels = jpeg.decode(raw, {
      useTArray: true,
      maxResolutionInMP: 2,
      maxMemoryUsageInMB: 64,
      tolerantDecoding: false,
    });
    if (pixels.width > 1600 || pixels.height > 1600) throw new Error();
    return Buffer.from(
      jpeg.encode(
        { width: pixels.width, height: pixels.height, data: pixels.data },
        80,
      ).data,
    ).toString("base64");
  } catch {
    throw new Failure(400, "invalid_photo");
  }
}
async function route(req: Request, env: Env) {
  const path = new URL(req.url).pathname;
  if (path === "/health" && req.method === "GET")
    return json({ service: "lighting-agent", protocol: 1 });
  const op = path.match(
    /^\/v1\/(open|status|turn|photo|prepare|claim|receipt|cancel|clear|forget)$/,
  )?.[1];
  if (!op) return json({}, 404);
  if (req.method !== "POST") return json({}, 405);
  const bearer = req.headers.get("authorization") ?? "";
  if (!/^Bearer [^\s]+$/i.test(bearer)) return json({}, 401);
  const home = req.headers.get("x-rhythm-home") ?? "",
    instance = req.headers.get("x-rhythm-installation") ?? "";
  if (
    op !== "forget" &&
    (!id.safeParse(home).success || !instance.length || instance.length > 160)
  )
    return json({}, 400);
  const auth = await access(
    env,
    bearer,
    home,
    instance,
    op === "forget" ? "identity" : "check",
  );
  const index = env.RESIDENT_INDEX.getByName(auth.actor);
  if (op === "forget") {
    await index.forget();
    return json({ queued: true });
  }
  if (!auth.hub) return json({}, 403);
  const enabled =
    (env.PILOT_RESIDENTS ?? "").split(",").includes(auth.actor) ||
    (parseInt(auth.actor.slice(0, 4), 16) / 65536) * 100 <
      Number(env.PILOT_PERCENT ?? 0);
  if (!enabled && !["clear", "status", "receipt", "cancel"].includes(op))
    return json({ error: "legacy_chat" }, 403);
  if (
    env.ACCEPT_NEW_TURNS !== "true" &&
    ["turn", "photo", "claim"].includes(op)
  )
    return json({ error: "pilot_paused" }, 503);
  const name = Buffer.from(
    await crypto.subtle.digest(
      "SHA-256",
      new TextEncoder().encode(
        JSON.stringify([auth.actor, auth.hub, instance]),
      ),
    ),
  ).toString("hex");
  if (!(await index.register(name))) return json({}, 410);
  const agent = await getAgentByName(env.CONVERSATIONS, name);
  if (op === "open") {
    const reply = await agent.open();
    return json(
      reply.code === 200
        ? { ...reply.data, scene_drafts: true, room_lookup: true }
        : reply.data,
      reply.code,
    );
  }
  const session = req.headers.get("x-rhythm-session") ?? "";
  if (!id.safeParse(session).success) return json({}, 400);
  let payload: unknown, photo: string | undefined;
  if (op === "photo") {
    if (!req.headers.get("content-type")?.startsWith("multipart/form-data;"))
      return json({}, 415);
    const bytes = await bounded(req, 1_600_000);
    const form = await new Response(bytes, {
      headers: { "content-type": req.headers.get("content-type")! },
    }).formData();
    const meta = form.get("turn"),
      image = form.get("photo");
    if (
      typeof meta !== "string" ||
      meta.length > 24000 ||
      !image ||
      typeof image === "string" ||
      image.type !== "image/jpeg"
    )
      return json({}, 400);
    payload = JSON.parse(meta);
    photo = normalizedPhoto(new Uint8Array(await image.arrayBuffer()));
  } else {
    if (!req.headers.get("content-type")?.startsWith("application/json"))
      return json({}, 415);
    payload = JSON.parse((await bounded(req, 24000)).toString("utf8"));
  }
  if (op !== "turn" && op !== "photo") {
    const body = z.record(z.string(), z.unknown()).parse(payload);
    const reply = await agent.control(session, op, body);
    return json(reply.data, reply.code);
  }
  const turn = turnSchema.parse(payload);
  if (
    turn.messages.at(-1)?.role !== "user" ||
    (!photo && !turn.messages.at(-1)?.text.trim())
  )
    return json({}, 400);
  const admission = await agent.control(session, "begin", { turn: turn.turn });
  if (admission.code !== 200) return json(admission.data, admission.code);
  try {
    await access(env, bearer, home, instance, "reserve");
    const decision = await infer(env.AI, turn, photo);
    await access(env, bearer, home, instance, "check");
    const reply = await agent.finish(
      session,
      turn.turn,
      decision.steps,
      Boolean(photo),
    );
    return json(
      {
        ...reply.data,
        ...(reply.code === 200
          ? {
              text: decision.text,
              proposed: decision.steps.length > 0,
              room_query: decision.room_query,
            }
          : {}),
      },
      reply.code,
    );
  } catch (error) {
    await agent.control(session, "abort", { turn: turn.turn });
    if (error instanceof Failure) throw error;
    throw new Failure(503, "inference_unavailable");
  }
}
export default {
  async fetch(req: Request, env: Env) {
    try {
      return await route(req, env);
    } catch (error) {
      if (error instanceof Failure)
        return json({ error: error.message }, error.status);
      if (error instanceof SyntaxError || error instanceof z.ZodError)
        return json({ error: "invalid_request" }, 400);
      return json({ error: "service_unavailable" }, 503);
    }
  },
} satisfies ExportedHandler<Env>;

import { createClient } from "npm:@supabase/supabase-js@2.118.0";
import {
  type Fields,
  object,
  Rejection,
  reviewCopy,
  uuid,
  validated,
} from "./contract.ts";
import { lightingChat } from "./lighting_chat.ts";
import { contract, mcp, response, versions } from "./mcp.ts";

const handoffs: Record<string, string> = {
  sign_in: "Open Account in the Rhythm app and sign in.",
  password_recovery: "Use Forgot password on the Rhythm sign-in screen.",
  plan_change: "Review plans and billing in the Rhythm account settings.",
  account_deletion:
    "Review the account deletion consequences in the Rhythm app and confirm there.",
};
const safeHandoff = (step: string) => ({
  status: "handoff",
  actor: "resident",
  surface: "rhythm_app",
  step,
  instructions: handoffs[step],
});

function data<T>(
  query: { data: T; error: { code?: string; message?: string } | null },
): T {
  if (query.error) {
    const code = query.error.code;
    if (code === "42501") throw new Rejection(403, "access_required");
    if (code === "P0001") throw new Rejection(409, "review_changed_or_expired");
    if (code === "22023" || code === "22P02") {
      throw new Rejection(400, "invalid_arguments");
    }
    throw new Rejection(503, "account_service_unavailable");
  }
  return query.data;
}

interface Backend {
  url: string;
  publicKey: string;
  serviceKey: string;
  fetch?: typeof fetch;
  chatModel?: { key?: string; model?: string; request: typeof fetch };
}

function configuredBackend(): Backend {
  return {
    url: Deno.env.get("SUPABASE_URL")!,
    publicKey: Deno.env.get("SB_PUBLISHABLE_KEY") ??
      Deno.env.get("SUPABASE_ANON_KEY")!,
    serviceKey: Deno.env.get("SB_SECRET_KEY") ??
      Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!,
  };
}

async function session(req: Request, backend?: Backend) {
  const token = /^Bearer ([^\s]+)$/i.exec(
    req.headers.get("authorization") ?? "",
  )?.[1];
  if (!token) throw new Rejection(401, "sign_in");
  const { url, publicKey, serviceKey, fetch } = backend ?? configuredBackend();
  const auth = { persistSession: false, autoRefreshToken: false };
  const client = createClient(url, publicKey, {
    global: { headers: { Authorization: `Bearer ${token}` }, fetch },
    auth,
  });
  const user = await client.auth.getUser(token);
  if (user.error && ![400, 401, 403].includes(user.error.status ?? 0)) {
    throw new Rejection(503, "account_service_unavailable");
  }
  if (user.error || !user.data.user || user.data.user.is_anonymous) {
    throw new Rejection(401, "sign_in");
  }
  // Reads execute with the resident's RLS policy and explicit membership.
  // The service client is reserved for the actor-bound review state machine.
  const service = createClient(url, serviceKey, { auth, global: { fetch } });
  return { client, service, userId: user.data.user.id };
}
type Session = Awaited<ReturnType<typeof session>>;
async function transition(
  s: Session,
  step: string,
  args: Fields = {},
  id?: string,
) {
  const result = data(
    await s.service.rpc("customer_assistant_review", {
      p_actor: s.userId,
      p_step: step,
      p_args: args,
      p_id: id ?? null,
    }),
  );
  if (!object(result)) throw new Rejection(503, "invalid_receipt");
  return step === "cancel"
    ? { status: result.status, plan_id: result.plan_id }
    : reviewCopy(result);
}
async function home(s: Session, id: string) {
  const value = data(
    await s.client.from("homes").select("id,owner_id,member_ids").eq("id", id)
      .contains("member_ids", [s.userId]).maybeSingle(),
  );
  if (!value) throw new Rejection(403, "home_access_required");
  return value;
}

async function execute(
  s: Session,
  name: string,
  arguments_: unknown,
): Promise<Fields> {
  const args = validated(name, arguments_);
  switch (name) {
    case "get_account_handoff":
      return safeHandoff(args.step as string);
    case "list_homes": {
      const homes = data(
        await s.client.from("homes").select("id,name,owner_id").contains(
          "member_ids",
          [s.userId],
        ).order("id"),
      ) ?? [];
      return {
        homes: homes.map((h) => ({
          home_id: h.id,
          display_name: h.name,
          role: h.owner_id === s.userId ? "owner" : "member",
        })),
      };
    }
    case "list_home_hubs":
    case "select_hub": {
      await home(s, args.home_id as string);
      let query = s.client.from("hubs").select(
        "id,name,home_id,enabled,server_instance_id",
      ).eq("home_id", args.home_id).eq("type", "server");
      if (name === "select_hub") query = query.eq("id", args.hub_id);
      const rows = data(await query.order("id")) ?? [];
      if (name === "list_home_hubs") {
        return {
          hubs: rows.map((h) => ({
            home_id: h.home_id,
            hub_id: h.id,
            display_name: h.name,
            enabled: h.enabled,
            server_instance_id: h.server_instance_id,
          })),
        };
      }
      const hub = rows[0];
      if (!hub) throw new Rejection(403, "hub_access_required");
      if (!hub.server_instance_id) {
        throw new Rejection(409, "live_hub_identity_required");
      }
      return {
        status: "handoff",
        actor: "resident_app",
        surface: "rhythm_app",
        home_id: hub.home_id,
        hub_id: hub.id,
        server_instance_id: hub.server_instance_id,
      };
    }
    case "list_home_members": {
      const members = data(
        await s.client.rpc("customer_assistant_home_members", {
          p_home_id: args.home_id,
        }),
      );
      if (!Array.isArray(members) || !members.length) {
        throw new Rejection(403, "home_access_required");
      }
      return {
        members: members.map(({ member_id, display_name, role, is_self }) => ({
          member_id,
          display_name,
          role,
          is_self,
        })),
      };
    }
    case "read_account_status": {
      const [sub, backup] = await Promise.all([
        s.client.from("subscriptions").select("tier,ended_at").eq(
          "user_id",
          s.userId,
        ).eq("status", "active").order("started_at", { ascending: false })
          .limit(1).maybeSingle(),
        s.client.from("user_cloud_snapshots").select("captured_at").eq(
          "user_id",
          s.userId,
        ).maybeSingle(),
      ]);
      const plan = data(sub), snapshot = data(backup);
      return {
        plan: plan && (!plan.ended_at || Date.parse(plan.ended_at) > Date.now())
          ? plan.tier
          : "basic",
        has_backup: !!snapshot,
        captured_at: snapshot?.captured_at ?? null,
      };
    }
    case "list_pending_actions": {
      const rows = data(
        await s.service.from("customer_assistant_reviews").select(
          "id,arguments,expires_at,state",
        ).eq("actor_id", s.userId)
          .in("state", ["review", "awaiting_device", "claimed"]).gt(
            "expires_at",
            new Date().toISOString(),
          ).order("expires_at", { ascending: false }).limit(16),
      ) ?? [];
      const visible = data(
        await s.client.from("homes").select("id").contains("member_ids", [
          s.userId,
        ]),
      ) ?? [];
      const ids = new Set(visible.map((h) => h.id));
      return {
        actions: rows.filter((r) =>
          r.arguments.change === "join_home" || ids.has(r.arguments.home_id)
        ).map((r) =>
          reviewCopy({
            ...r.arguments,
            plan_id: r.id,
            status: r.state,
            expires_at: r.expires_at,
            requires_confirmation: r.state === "review",
          })
        ),
      };
    }
    case "plan_customer_change":
      return transition(s, "plan", args);
    case "cancel_customer_plan":
      return transition(s, "cancel", {}, args.plan_id as string);
    case "apply_customer_plan":
      return transition(
        s,
        "confirm",
        { confirmed: args.confirmed },
        args.plan_id as string,
      );
    default:
      throw new Rejection(404, "unknown_operation");
  }
}

async function readPayload(req: Request): Promise<Fields> {
  const limit = 16384;
  if (Number(req.headers.get("content-length")) > limit) {
    throw new Rejection(413, "request_too_large");
  }
  const reader = req.body?.getReader();
  if (!reader) throw new Rejection(400, "invalid_request");
  const buffer = new Uint8Array(limit);
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (size + value.byteLength > limit) {
        await reader.cancel();
        throw new Rejection(413, "request_too_large");
      }
      buffer.set(value, size);
      size += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  try {
    const parsed = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(
        buffer.subarray(0, size),
      ),
    );
    if (object(parsed)) return parsed;
  } catch { /* Return one error for malformed JSON and encoding. */ }
  throw new Rejection(400, "invalid_request");
}

export async function handleRequest(req: Request, backend?: Backend) {
  if (req.method === "OPTIONS") return response(null, 204);
  if (req.method !== "POST") {
    return response({ error: "method_not_allowed" }, 405);
  }
  try {
    const s = await session(req, backend);
    const b = await readPayload(req);
    if ("jsonrpc" in b) {
      if (req.headers.has("origin")) {
        throw new Rejection(403, "browser_origin_not_supported");
      }
      const version = req.headers.get("mcp-protocol-version");
      if (version && !versions.includes(version)) {
        throw new Rejection(400, "unsupported_protocol");
      }
      return await mcp(b, (name, args) => execute(s, name, args));
    }
    if ("lighting_chat" in b) {
      const config = backend?.chatModel ?? {
        key: Deno.env.get("OPENAI_API_KEY"),
        model: Deno.env.get("RHYTHM_CHAT_MODEL"),
        request: fetch,
      };
      const reserve = async () =>
        data(
          await s.service.rpc(
            "reserve_lighting_chat_turn",
            { p_actor: s.userId },
          ),
        ) === true;
      return response(
        await lightingChat(b.lighting_chat, reserve, config, config.request),
      );
    }
    if (b.contract === true) return response(await contract());
    if (uuid(b.device_claim)) {
      return response(await transition(s, "claim", {}, b.device_claim));
    }
    if (
      uuid(b.device_complete) &&
      (b.outcome === "acknowledged" || b.outcome === "uncertain")
    ) {
      return response(
        await transition(
          s,
          "complete",
          { outcome: b.outcome },
          b.device_complete,
        ),
      );
    }
    return response(await execute(s, String(b.operation), b.arguments ?? {}));
  } catch (e) {
    const status = e instanceof Rejection ? e.status : 503;
    return response(
      status === 401 ? safeHandoff("sign_in") : {
        error: e instanceof Rejection
          ? e.message
          : "account_service_unavailable",
      },
      status,
    );
  }
}

// A narrow bridge to account authority and the existing shared chat allowance.
// Each call authenticates a live resident. The Worker holds only a public key.
import { createClient } from "npm:@supabase/supabase-js@2.118.0";

const reply = (status: number, value: unknown = {}) =>
  Response.json(value, {
    status,
    headers: { "cache-control": "no-store" },
  });

export async function access(
  req: Request,
  env: (key: string) => string | undefined = (key) => Deno.env.get(key),
  http: typeof fetch = fetch,
) {
  if (req.method !== "POST") return reply(405);
  const token = /^Bearer ([^\s]+)$/i.exec(
    req.headers.get("authorization") ?? "",
  )?.[1];
  if (!token) return reply(401);
  const url = env("SUPABASE_URL")!;
  const key = env("SB_PUBLISHABLE_KEY") ?? env("SUPABASE_ANON_KEY")!;
  const auth = { persistSession: false, autoRefreshToken: false };
  const client = createClient(url, key, {
    auth,
    global: { fetch: http, headers: { Authorization: "Bearer " + token } },
  });
  try {
    const { data, error } = await client.auth.getUser(token);
    if (error || !data.user || data.user.is_anonymous) {
      return reply(error && (error.status ?? 0) >= 500 ? 503 : 401);
    }
    const actor = data.user.id;
    const body = await req.json();
    if (body.action === "discover") {
      const endpoint = env("RHYTHM_LIGHTING_AGENT_URL");
      return reply(200, { endpoint: endpoint || null });
    }
    if (body.action === "identity") return reply(200, { actor });
    if (
      !["check", "reserve"].includes(body.action) ||
      typeof body.home !== "string" ||
      !/^[0-9a-f-]{36}$/i.test(body.home) ||
      typeof body.instance !== "string" ||
      !body.instance.length || body.instance.length > 160
    ) return reply(400);
    const home = await client.from("homes").select("id").eq("id", body.home)
      .contains("member_ids", [actor]).maybeSingle();
    if (home.error) return reply(503);
    if (!home.data) return reply(403);
    const hub = await client.from("hubs").select("id").eq("home_id", body.home)
      .eq("type", "server").eq("server_instance_id", body.instance).eq(
        "enabled",
        true,
      ).maybeSingle();
    if (hub.error) return reply(503);
    if (!hub.data) return reply(403);
    if (body.action === "reserve") {
      const service = createClient(
        url,
        env("SB_SECRET_KEY") ?? env("SUPABASE_SERVICE_ROLE_KEY")!,
        { auth, global: { fetch: http } },
      );
      const reserved = await service.rpc("reserve_lighting_chat_turn", {
        p_actor: actor,
      });
      if (reserved.error) return reply(503);
      if (reserved.data !== true) return reply(429);
    }
    return reply(200, { actor, hub: hub.data.id });
  } catch {
    return reply(503);
  }
}

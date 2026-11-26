import { createClient } from "npm:@supabase/supabase-js@2.118.0";
import {
  authenticateActivityDevice,
  readBearerToken,
  sha256Hex,
} from "../_shared/activity_device_auth.ts";
import { apnsProvider } from "./apns.ts";

const messages = {
  connection: "An assistant is asking to connect to your hub.",
  plan: "An assistant is asking to change your lighting.",
} as const;

type Ring = {
  kind: keyof typeof messages;
  request_id: string;
  expires_at_epoch_ms: number;
  home_id: string;
  hub_id: string;
  server_instance_id: string | null;
};

function validRing(body: unknown, now: number): body is Ring {
  if (!body || typeof body !== "object") return false;
  const r = body as Partial<Ring>;
  return (r.kind === "connection" || r.kind === "plan") &&
    typeof r.request_id === "string" &&
    /^[A-Za-z0-9_-]{1,128}$/.test(r.request_id) &&
    typeof r.expires_at_epoch_ms === "number" &&
    Number.isSafeInteger(r.expires_at_epoch_ms) &&
    r.expires_at_epoch_ms > now && r.expires_at_epoch_ms <= now + 15 * 60_000 &&
    typeof r.home_id === "string" && r.home_id.length > 0 &&
    typeof r.hub_id === "string" && r.hub_id.length > 0 &&
    (r.server_instance_id === null || typeof r.server_instance_id === "string");
}

const reply = (status: number, value: unknown = {}) =>
  Response.json(value, { status, headers: { "cache-control": "no-store" } });

export function createAttentionHandler(
  env: (key: string) => string | undefined = (key) => Deno.env.get(key),
  http: typeof fetch = fetch,
  now: () => number = Date.now,
) {
  const keyId = env("APNS_KEY_ID"), teamId = env("APNS_TEAM_ID");
  const privateKey = env("APNS_PRIVATE_KEY"), bundleId = env("APNS_BUNDLE_ID");
  const provider = keyId && teamId && privateKey && bundleId
    ? apnsProvider({ keyId, teamId, privateKey, bundleId }, now)
    : undefined;

  return async (req: Request) => {
    if (req.method !== "POST") return reply(405);
    const token = readBearerToken(req.headers.get("authorization"));
    if (!token) return reply(401);
    const body: unknown = await req.json().catch(() => null);
    if (!validRing(body, now())) return reply(400);
    try {
      const client = createClient(
        env("SUPABASE_URL")!,
        (env("SB_SECRET_KEY") ?? env("SUPABASE_SERVICE_ROLE_KEY"))!,
        {
          auth: { persistSession: false, autoRefreshToken: false },
          global: { fetch: http },
        },
      );
      const auth = await authenticateActivityDevice(client, token, {
        homeId: body.home_id,
        hubId: body.hub_id,
        serverInstanceId: body.server_instance_id,
      }, true);
      if (!auth.ok) return reply(auth.status);
      const quota = await client.rpc("reserve_hub_attention_ring", {
        p_hub_id: auth.tokenRow.hub_id,
      });
      if (quota.error) throw quota.error;
      if (quota.data !== true) return reply(429);
      if (!provider || !bundleId) {
        return reply(200, { delivered: 0, reason: "push_not_configured" });
      }

      const { home_id, hub_id } = auth.tokenRow;
      const { request_id, kind } = body;
      const payload = JSON.stringify({
        aps: {
          alert: { title: "Rhythm", body: messages[kind] },
          sound: "default",
        },
        rhythm: { kind, home_id, hub_id, request_id },
      });
      // APNs accepts only 64 bytes in the collapse header. Preserve longer
      // request IDs in the payload and use their digest for APNs grouping.
      const collapseId = request_id.length <= 64
        ? request_id
        : await sha256Hex(request_id);
      const headers = {
        authorization: "bearer " + await provider(),
        "content-type": "application/json",
        "apns-topic": bundleId,
        "apns-push-type": "alert",
        "apns-collapse-id": collapseId,
        "apns-expiration": String(Math.floor(body.expires_at_epoch_ms / 1000)),
      };

      let delivered = 0;
      // Each member can register ten devices. Batch the membership filter to
      // keep each query below PostgREST's result cap and URI size limit.
      for (let start = 0; start < auth.memberIds.length; start += 25) {
        const devices = await client.from("push_devices").select(
          "id,environment,token",
        )
          .eq("app_id", bundleId).in(
            "user_id",
            auth.memberIds.slice(start, start + 25),
          );
        if (devices.error) throw devices.error;
        const results = await Promise.all(devices.data.map(async (device) => {
          const host = device.environment === "development"
            ? "api.sandbox.push.apple.com"
            : "api.push.apple.com";
          // fetch negotiates HTTP/2 with APNs over TLS. A failed delivery to
          // one phone should not prevent another resident from receiving it.
          const res = await http(
            "https://" + host + "/3/device/" + encodeURIComponent(device.token),
            {
              method: "POST",
              redirect: "error",
              signal: AbortSignal.timeout(8_000),
              headers,
              body: payload,
            },
          ).catch(() => null);
          if (!res) return 0;
          if (res.status === 400 || res.status === 410) {
            const detail = await res.json().catch(() => null);
            if (res.status === 410 || detail?.reason === "BadDeviceToken") {
              // Every registration gets a new ID, so stale responses cannot
              // remove a newly registered token or its new account owner.
              const removed = await client.from("push_devices").delete().eq(
                "id",
                device.id,
              );
              if (removed.error) throw removed.error;
            }
          } else {
            await res.body?.cancel();
          }
          return res.status === 200 ? 1 : 0;
        }));
        for (const count of results) delivered += count;
      }
      return reply(200, { delivered });
    } catch {
      return reply(503);
    }
  };
}

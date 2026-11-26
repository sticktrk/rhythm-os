import type { SupabaseClient } from "npm:@supabase/supabase-js@2.118.0";
import {
  serverActivityTokenCanAuthenticate,
  type ServerActivityTokenState,
} from "./server_activity_token.ts";

type DeviceToken = ServerActivityTokenState & {
  id: string;
  user_id: string;
  home_id: string;
  hub_id: string;
  server_instance_id: string | null;
};

export function readBearerToken(header: string | null): string | null {
  return /^Bearer ([^\s]+)$/.exec(header ?? "")?.[1] ?? null;
}

export async function sha256Hex(value: string): Promise<string> {
  const bytes = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return Array.from(
    new Uint8Array(bytes),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
}

// Existing activity credentials can predate instance binding. New notification
// requests must match all three scope fields, including a legacy null binding.
export async function authenticateActivityDevice(
  client: SupabaseClient,
  token: string,
  scope: { homeId: string; hubId: string; serverInstanceId: string | null },
  requireExactInstance = false,
): Promise<
  | { ok: true; tokenRow: DeviceToken; memberIds: string[] }
  | { ok: false; status: 401 | 403; error: string }
> {
  const lookup = await client.from("server_light_activity_device_tokens")
    .select(
      "id,user_id,home_id,hub_id,server_instance_id,activated_at,activation_expires_at,revoked_at",
    ).eq("token_hash", await sha256Hex(token)).is("revoked_at", null)
    .maybeSingle();
  if (lookup.error) throw lookup.error;
  const row: DeviceToken | null = lookup.data;
  if (!row || !serverActivityTokenCanAuthenticate(row)) {
    return { ok: false, status: 401, error: "Invalid device token" };
  }
  if (row.home_id !== scope.homeId || row.hub_id !== scope.hubId) {
    return { ok: false, status: 403, error: "Device token scope mismatch" };
  }
  const home = await client.from("homes").select("member_ids").eq(
    "id",
    row.home_id,
  ).maybeSingle();
  if (home.error) throw home.error;
  const members = home.data?.member_ids;
  let authorized = Array.isArray(members) && members.includes(row.user_id);
  if (authorized) {
    const hub = await client.from("hubs").select(
      "home_id,type,server_instance_id",
    )
      .eq("id", row.hub_id).maybeSingle();
    if (hub.error) throw hub.error;
    authorized = hub.data?.home_id === row.home_id &&
      hub.data?.type === "server" &&
      !(row.server_instance_id && hub.data.server_instance_id &&
        hub.data.server_instance_id !== row.server_instance_id);
  }
  if (!authorized) {
    const revoked = await client.from("server_light_activity_device_tokens")
      .update({ revoked_at: new Date().toISOString() }).eq("id", row.id).is(
        "revoked_at",
        null,
      );
    if (revoked.error) throw revoked.error;
    return {
      ok: false,
      status: 403,
      error: "Device token no longer authorized",
    };
  }
  if (
    (requireExactInstance || row.server_instance_id) &&
    row.server_instance_id !== scope.serverInstanceId
  ) {
    return { ok: false, status: 403, error: "Server identity mismatch" };
  }
  return { ok: true, tokenRow: row, memberIds: members };
}

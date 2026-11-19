import { type JsonObject, jsonResponse, readString } from "../_shared/auth.ts";

const DEFAULT_SCOPE = "beta_admin";

export function readScope(body: JsonObject): string | Response {
  const scope = readString(body, "scope") ?? DEFAULT_SCOPE;
  if (scope !== DEFAULT_SCOPE) {
    return jsonResponse(
      { error: `Unsupported support access scope: ${scope}` },
      400,
    );
  }
  return scope;
}

export async function storeGrant(
  adminClient: any,
  userId: string,
  hub: { id: string; home_id: string },
  action: "grant" | "revoke",
  body: JsonObject,
  payload: JsonObject,
): Promise<Response> {
  const scope = readScope(body);
  if (scope instanceof Response) return scope;
  const { data, error } = await adminClient.rpc(
    "store_customer_support_grant",
    {
      p_actor: userId,
      p_hub: hub.id,
      p_action: action,
      p_payload: payload,
      p_review: readString(body, "assistant_plan_id"),
    },
  );
  if (error?.code === "PGRST202") {
    if (action === "grant") {
      return jsonResponse({
        error: "Support access is updating. Retry the grant shortly.",
      }, 503);
    }
    // The previous schema supports withdrawing the encrypted credential even
    // before PostgREST knows about the transactional storage function.
    const revoked = await adminClient.from("hub_support_access_grants")
      .update({
        revoked_at: new Date().toISOString(),
        encrypted_token: { version: "support_access_token_revoked_v1" },
      })
      .eq("hub_id", hub.id).eq("home_id", hub.home_id).eq("scope", scope)
      .is("revoked_at", null).select("token_id");
    if (revoked.error) {
      return jsonResponse({ error: "Could not revoke support access" }, 503);
    }
    const tokenIds = (revoked.data ?? []).map((row: { token_id: string }) =>
      row.token_id
    );
    return jsonResponse({
      status: "ok",
      revoked_count: tokenIds.length,
      token_ids: tokenIds,
    });
  }
  if (error?.code === "42501") {
    return jsonResponse(
      { error: "Support access requires resident review" },
      403,
    );
  }
  if (error?.code === "P0001") {
    return jsonResponse({ error: "Review support access again" }, 409);
  }
  if (error) {
    return jsonResponse({ error: "Could not update support access" }, 500);
  }
  return jsonResponse(data);
}

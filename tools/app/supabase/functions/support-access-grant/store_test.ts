import { storeGrant } from "./store.ts";

Deno.test("missing grant RPC preserves legacy revoke and defers grant", async () => {
  const filters: Record<string, unknown> = {};
  let envelope: unknown, updates = 0;
  const query = {
    update(row: { revoked_at: string; encrypted_token: unknown }) {
      if (!row.revoked_at) throw Error("missing revocation time");
      updates++;
      envelope = row.encrypted_token;
      return this;
    },
    eq(key: string, value: string) {
      filters[key] = value;
      return this;
    },
    is(key: string, value: null) {
      filters[key] = value;
      return this;
    },
    select: () => Promise.resolve({ data: [{ token_id: "fixture" }] }),
  };
  const client = {
    rpc: () => Promise.resolve({ error: { code: "PGRST202" } }),
    from: (table: string) => {
      if (table !== "hub_support_access_grants") throw Error("wrong table");
      return query;
    },
  };
  const hub = { id: "hub", home_id: "home" };
  const grant = await storeGrant(client, "actor", hub, "grant", {}, {});
  if (grant.status !== 503 || updates) throw Error("grant should retry");
  const revoke = await storeGrant(client, "actor", hub, "revoke", {}, {});
  const result = await revoke.json();
  if (
    revoke.status !== 200 || result.revoked_count !== 1 ||
    result.token_ids[0] !== "fixture" ||
    updates !== 1 ||
    JSON.stringify(envelope) !==
      '{"version":"support_access_token_revoked_v1"}' ||
    JSON.stringify(filters) !==
      '{"hub_id":"hub","home_id":"home","scope":"beta_admin","revoked_at":null}'
  ) {
    throw Error("revocation did not target the active scoped grant");
  }
});

import { access } from "./access.ts";

const actor = "9e15ce55-ef25-45ce-9717-df3cf8dbec34";
const home = "6285d087-c9bc-45f2-8fae-feb0bd09550d";
const hub = "10de4c0d-79f5-43b3-9ac3-f79708921d37";
function same(v: unknown, expected: unknown) {
  if (JSON.stringify(v) !== JSON.stringify(expected)) {
    throw Error("Unexpected fixture result: " + JSON.stringify(v));
  }
}
Deno.test("live resident, membership and enabled installation are required before reserving shared quota", async () => {
  let member = true,
    paired = true,
    anonymous = false,
    available = true,
    reservations = 0;
  const env = (
    key: string,
  ) => ({
    SUPABASE_URL: "https://account.invalid",
    SUPABASE_ANON_KEY: "public",
    SUPABASE_SERVICE_ROLE_KEY: "private",
  }[key]);
  const http: typeof fetch = async (input, init) => {
    const req = new Request(input, init), url = new URL(req.url);
    if (url.pathname === "/auth/v1/user") {
      same(req.headers.get("authorization"), "Bearer resident");
      return Response.json({ id: actor, is_anonymous: anonymous });
    }
    if (url.pathname === "/rest/v1/homes") {
      same(url.searchParams.get("id"), "eq." + home);
      same(url.searchParams.get("member_ids"), "cs.{" + actor + "}");
      return Response.json(member ? [{ id: home }] : []);
    }
    if (url.pathname === "/rest/v1/hubs") {
      same(url.searchParams.get("home_id"), "eq." + home);
      same(url.searchParams.get("server_instance_id"), "eq.box");
      same(url.searchParams.get("type"), "eq.server");
      same(url.searchParams.get("enabled"), "eq.true");
      return Response.json(paired ? [{ id: hub }] : []);
    }
    same(url.pathname, "/rest/v1/rpc/reserve_lighting_chat_turn");
    same(await req.json(), { p_actor: actor });
    same(req.headers.get("apikey"), "private");
    reservations++;
    return Response.json(available);
  };
  const call = (action: string) =>
    access(
      new Request("https://fixture.invalid", {
        method: "POST",
        headers: {
          authorization: "Bearer resident",
          "content-type": "application/json",
        },
        body: JSON.stringify({ action, home, instance: "box" }),
      }),
      env,
      http,
    );
  same(await (await call("check")).json(), { actor, hub });
  member = false;
  same((await call("reserve")).status, 403);
  member = true;
  paired = false;
  same((await call("reserve")).status, 403);
  paired = true;
  anonymous = true;
  same((await call("reserve")).status, 401);
  anonymous = false;
  same(reservations, 0);
  same((await call("reserve")).status, 200);
  available = false;
  same((await call("reserve")).status, 429);
  same(reservations, 2);
});

import assert from "node:assert/strict";
import { createAttentionHandler } from "./handler.ts";
import { sha256Hex } from "../_shared/activity_device_auth.ts";

const home = "6285d087-c9bc-45f2-8fae-feb0bd09550d";
const hub = "10de4c0d-79f5-43b3-9ac3-f79708921d37";
const actor = "9e15ce55-ef25-45ce-9717-df3cf8dbec34";
const resident = "34479b7b-d82f-48f5-a1f0-73cb724779c3";
const epoch = 1_800_000_000_000;
const ring = {
  home_id: home,
  hub_id: hub,
  server_instance_id: "fixture-box",
  kind: "connection",
  request_id: "req_12-A",
  expires_at_epoch_ms: epoch + 300_000,
};
const devices = [
  { id: "device1", environment: "development", token: "ab12" },
  { id: "device2", environment: "production", token: "cd34" },
];

async function fixture(configured = true) {
  const keys = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["sign", "verify"],
  );
  const encoded = btoa(
    String.fromCharCode(
      ...new Uint8Array(
        await crypto.subtle.exportKey("pkcs8", keys.privateKey),
      ),
    ),
  );
  const env: Record<string, string> = {
    SUPABASE_URL: "https://account.invalid",
    SUPABASE_SERVICE_ROLE_KEY: "fixture",
    APNS_TEAM_ID: "fixtureteam",
    APNS_KEY_ID: "fixturekey",
    APNS_BUNDLE_ID: "app.fixture",
  };
  if (configured) {
    env.APNS_PRIVATE_KEY = [
      "-----BEGIN PRIVATE KEY-----",
      encoded,
      "-----END PRIVATE KEY-----",
    ].join("\n");
  }
  const state = {
    members: [actor, resident],
    planExpiry: null as string | null,
    pendingToken: false,
    now: epoch,
    quota: 0,
    revoked: 0,
    lookups: 0,
    deletes: [] as string[],
    databaseQueries: [] as URL[],
    pushes: [] as Request[],
    apns: (_: URL) => new Response(null, { status: 200 }),
  };
  const validHash = await sha256Hex("fixture-upload");
  const http: typeof fetch = async (input, init) => {
    const req = new Request(input, init);
    const url = new URL(req.url), path = url.pathname;
    if (url.host !== "account.invalid") {
      state.pushes.push(req);
      return state.apns(url);
    }
    state.databaseQueries.push(url);
    assert.equal(req.headers.get("apikey"), "fixture");
    if (path === "/rest/v1/server_light_activity_device_tokens") {
      if (req.method === "PATCH") {
        state.revoked++;
        return new Response(null, { status: 204 });
      }
      state.lookups++;
      assert.equal(url.searchParams.get("revoked_at"), "is.null");
      return Response.json(
        url.searchParams.get("token_hash") === "eq." + validHash
          ? [{
            id: "upload",
            user_id: actor,
            home_id: home,
            hub_id: hub,
            server_instance_id: ring.server_instance_id,
            activated_at: state.pendingToken
              ? null
              : new Date(epoch).toISOString(),
            activation_expires_at: state.planExpiry,
            revoked_at: null,
          }]
          : [],
      );
    }
    if (path === "/rest/v1/homes") {
      assert.equal(url.searchParams.get("id"), "eq." + home);
      return Response.json([{ member_ids: state.members }]);
    }
    if (path === "/rest/v1/hubs") {
      assert.equal(url.searchParams.get("id"), "eq." + hub);
      return Response.json([{
        type: "server",
        home_id: home,
        server_instance_id: ring.server_instance_id,
      }]);
    }
    if (path === "/rest/v1/rpc/reserve_hub_attention_ring") {
      assert.deepEqual(await req.json(), { p_hub_id: hub });
      return Response.json(++state.quota <= 6);
    }
    assert.equal(path, "/rest/v1/push_devices");
    if (req.method === "DELETE") {
      state.deletes.push(url.searchParams.get("id")!);
      return new Response(null, { status: 204 });
    }
    return Response.json(devices);
  };
  const handle = createAttentionHandler(
    (key) => env[key],
    http,
    () => state.now,
  );
  function send(overrides = {}, token = "fixture-upload") {
    return handle(
      new Request("https://edge.invalid/hub-attention", {
        method: "POST",
        headers: { authorization: "Bearer " + token },
        body: JSON.stringify({ ...ring, ...overrides }),
      }),
    );
  }
  return { state, send, publicKey: keys.publicKey };
}

Deno.test("authenticate before ringing, including live membership and device instance binding", async () => {
  const { state, send } = await fixture();
  assert.equal((await send({}, "unknown")).status, 401);
  assert.equal((await send({ home_id: "other-home" })).status, 403);
  assert.equal((await send({ hub_id: "other-hub" })).status, 403);
  assert.equal((await send({ server_instance_id: "other-box" })).status, 403);
  state.pendingToken = true;
  state.planExpiry = new Date(0).toISOString();
  assert.equal((await send()).status, 401);
  state.pendingToken = false;
  state.members = [resident];
  assert.equal((await send()).status, 403);
  assert.equal(state.revoked, 1);
  assert.equal(state.quota, 0);
  assert.deepEqual(state.pushes, []);
});

Deno.test("bounded request identifiers and expiration precede any database access", async () => {
  const { state, send } = await fixture();
  for (
    const request of [
      { kind: "arbitrary" },
      { request_id: "a".repeat(129) },
      { request_id: "bad id" },
      { expires_at_epoch_ms: epoch },
      { expires_at_epoch_ms: epoch + 900_001 },
    ]
  ) {
    assert.equal((await send(request)).status, 400);
  }
  assert.equal(state.lookups, 0);
});

Deno.test("each resident's APNs devices receive only the fixed alert and opaque IDs", async () => {
  const { state, send, publicKey } = await fixture();
  const extras = {
    hub_name: "Test Fixture Box",
    room: "Fixture Bedroom",
    match_code: "47",
  };
  assert.deepEqual(await (await send(extras)).json(), { delivered: 2 });
  assert.deepEqual(state.pushes.map((r) => r.url), [
    "https://api.sandbox.push.apple.com/3/device/ab12",
    "https://api.push.apple.com/3/device/cd34",
  ]);
  const filter =
    state.databaseQueries.find((url) => url.pathname.endsWith("/push_devices"))!
      .searchParams;
  assert.equal(filter.get("app_id"), "eq.app.fixture");
  assert.equal(
    filter.get("user_id"),
    "in.(" + [actor, resident].join(",") + ")",
  );
  for (const req of state.pushes) {
    assert.equal(req.method, "POST");
    assert.equal(req.headers.get("apns-topic"), "app.fixture");
    assert.equal(req.headers.get("apns-collapse-id"), ring.request_id);
    assert.equal(
      req.headers.get("apns-expiration"),
      String(ring.expires_at_epoch_ms / 1000),
    );
    assert.equal(req.headers.get("apns-push-type"), "alert");
    const payload = await req.text();
    for (const privateValue of Object.values(extras)) {
      assert(!payload.includes(privateValue));
    }
    assert.deepEqual(JSON.parse(payload), {
      aps: {
        alert: {
          title: "Rhythm",
          body: "An assistant is asking to connect to your hub.",
        },
        sound: "default",
      },
      rhythm: {
        kind: "connection",
        home_id: home,
        hub_id: hub,
        request_id: ring.request_id,
      },
    });
  }
  const bearer = state.pushes[0].headers.get("authorization")!;
  assert(bearer.startsWith("bearer "));
  const [header, claims, signature] = bearer.slice(7).split(".");
  const decode = (part: string) =>
    atob(part.replaceAll("-", "+").replaceAll("_", "/"));
  assert.deepEqual(JSON.parse(decode(header)), {
    alg: "ES256",
    kid: "fixturekey",
  });
  assert.deepEqual(JSON.parse(decode(claims)), {
    iss: "fixtureteam",
    iat: epoch / 1000,
  });
  assert(
    await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      publicKey,
      Uint8Array.from(decode(signature), (c) => c.charCodeAt(0)),
      new TextEncoder().encode(header + "." + claims),
    ),
  );

  const longId = "a".repeat(128);
  state.now += 49 * 60_000;
  await send({
    kind: "plan",
    request_id: longId,
    expires_at_epoch_ms: state.now + 1000,
  });
  assert.equal(state.pushes[2].headers.get("authorization"), bearer);
  assert.equal(
    state.pushes[2].headers.get("apns-collapse-id"),
    await sha256Hex(longId),
  );
  const plan = await state.pushes[2].json();
  assert.equal(
    plan.aps.alert.body,
    "An assistant is asking to change your lighting.",
  );
  assert.equal(plan.rhythm.request_id, longId);
  state.now += 60_000;
  await send({ expires_at_epoch_ms: state.now + 1000 });
  assert.notEqual(state.pushes[4].headers.get("authorization"), bearer);
});

Deno.test("only terminal APNs device failures remove the selected registration", async () => {
  const { state, send } = await fixture();
  state.apns = (url) =>
    url.host.startsWith("api.sandbox")
      ? new Response(null, { status: 410 })
      : Response.json({ reason: "InvalidProviderToken" }, { status: 403 });
  assert.deepEqual(await (await send()).json(), { delivered: 0 });
  assert.deepEqual(state.deletes, ["eq.device1"]);
  state.apns = (url) =>
    url.host.startsWith("api.sandbox")
      ? new Response(null, { status: 200 })
      : Response.json({ reason: "BadDeviceToken" }, { status: 400 });
  assert.deepEqual(await (await send()).json(), { delivered: 1 });
  assert.deepEqual(state.deletes, ["eq.device1", "eq.device2"]);
});

Deno.test("missing APNs configuration is a quiet success; the seventh request is throttled", async () => {
  const { state, send } = await fixture(false);
  for (let n = 0; n < 6; n++) {
    const res = await send();
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), {
      delivered: 0,
      reason: "push_not_configured",
    });
  }
  assert.equal((await send()).status, 429);
  assert.deepEqual(state.pushes, []);
  assert(
    !state.databaseQueries.some((url) =>
      url.pathname.endsWith("/push_devices")
    ),
  );
});

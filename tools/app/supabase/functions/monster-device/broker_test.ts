import { createMonsterBroker, privateIp } from "./broker.ts";
function assert(
  condition: unknown,
  message = "assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}
const DSN = "ACFIXTURE123456";
const secrets: Record<string, string> = {
  MONSTER_OWNER_USER_ID: "owner",
  MONSTER_EMAIL: "fixture@example.invalid",
  MONSTER_PASSWORD: "fixture-password",
};
const fixtureAyla = { appId: "fixture-app", appSecret: "fixture-app-secret" };
function fixture(
  options: {
    owned?: boolean;
    fail?: string;
    missingKey?: boolean;
    model?: string;
    env?: Record<string, string | undefined>;
    ayla?: { appId: string; appSecret: string };
  } = {},
) {
  const env = { ...secrets, ...options.env };
  const calls: { url: string; body: any; headers: Headers }[] = [];
  let owned = options.owned ?? true;
  let clock = 100000000;
  const properties = Object.entries({
    power: "boolean",
    brightness: "integer",
    color_bright: "integer",
    color_select: "integer",
    color_saturation: "integer",
    mode: "string",
  }).map(([name, base_type]) => ({
    property: { name, base_type, read_only: false },
  }));
  const fetcher: typeof fetch = async (input, init) => {
    const url = String(input);
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    const headers = new Headers(init?.headers);
    calls.push({ url, body, headers });
    assert(init?.redirect === "error", "credentials must not follow redirects");
    if (options.fail && url.includes(options.fail)) {
      return new Response("fixture-password provider-token private response", {
        status: 403,
      });
    }
    let result: unknown;
    if (url.endsWith("/auth/login")) {
      assert(headers.get("x-copilot-sdk-version") === "6.0.6");
      assert(
        body.deviceDetails.osType === "ANDROID" &&
          body.deviceDetails.deviceType === "PHONE",
      );
      assert(body.authenticationDetails.password === secrets.MONSTER_PASSWORD);
      result = { accessToken: "provider-token", tokenType: "Bearer" };
    } else if (url.endsWith("/acquire_ticket")) {
      assert(headers.get("Authorization") === "Bearer provider-token");
      result = { partnerTicket: "partner-token" };
    } else if (url.endsWith("/token_sign_in")) {
      assert(body.app_id === fixtureAyla.appId);
      assert(body.app_secret === fixtureAyla.appSecret);
      result = { access_token: "ayla-token", expires_in: 300 };
    } else {
      assert(headers.get("Authorization") === "auth_token ayla-token");
      if (url.endsWith("/devices.json")) {
        if (body) {
          assert(
            body.device.dsn === DSN &&
              /^[0-9a-f]{32}$/.test(body.device.setup_token),
          );
          owned = true;
          result = { device: { dsn: DSN, lan_ip: "192.168.5.200" } };
        } else {result = owned
            ? [{ device: { dsn: DSN, lan_ip: "192.168.5.200" } }]
            : [];}
      } else if (url.endsWith("/properties.json")) {
        result = [...properties, {
          property: {
            name: "device_id",
            value: options.model ?? "xt-16ft-hw-neon-led-rgbic",
          },
        }];
      } else if (url.endsWith("/connection_config.json")) {
        result = {
          local_key: options.missingKey ? undefined : "fixture-lan-key",
          local_key_id: 1234,
        };
      } else throw new Error("unexpected fixture URL");
    }
    return Response.json(result);
  };
  const handle = createMonsterBroker({
    env: (n) => env[n],
    fetch: fetcher,
    now: () => clock,
    ayla: options.ayla ?? fixtureAyla,
  });
  const call = (body: unknown, user = "owner") =>
    handle(
      new Request("https://rhythm.invalid/monster-device", {
        method: "POST",
        body: JSON.stringify(body),
      }),
      user,
    );
  return {
    calls,
    call,
    advance: () => {
      clock += 600001;
    },
  };
}
Deno.test("shared account is the default; every authenticated user may use it", async () => {
  const shared = fixture({ env: { MONSTER_OWNER_USER_ID: undefined } });
  assert(
    (await shared.call({ action: "key", dsn: DSN }, "other-user")).status ===
      200,
  );
  const ticket = await (await shared.call({ action: "begin", dsn: DSN }, "a"))
    .json();
  // Tickets still bind the user who began commissioning, even when shared.
  assert(
    (await shared.call(
      { action: "complete", dsn: DSN, ticket: ticket.ticket },
      "b",
    )).status === 400,
  );
  assert(
    (await shared.call(
      { action: "complete", dsn: DSN, ticket: ticket.ticket },
      "a",
    )).status === 200,
  );
});
Deno.test("owner isolation and malformed requests make no vendor calls", async () => {
  const f = fixture();
  for (
    const body of [{ action: "key" }, { action: "key", dsn: 12345678 }, {
      action: "key",
      dsn: "../secret",
    }, null]
  ) assert((await f.call(body)).status === 400);
  assert(
    (await f.call({ action: "key", dsn: DSN }, "other-user")).status === 403,
  );
  assert(f.calls.length === 0);
});
Deno.test("key lookup uses snake_case, validates model and never returns cloud credentials", async () => {
  const f = fixture();
  const response = await f.call({ action: "key", dsn: DSN });
  const text = await response.text();
  assert(response.status === 200);
  assert(response.headers.get("cache-control") === "no-store");
  const data = JSON.parse(text);
  assert(data.local_key === "fixture-lan-key" && data.dsn === DSN);
  for (
    const secret of [
      "fixture-password",
      "provider-token",
      "ayla-token",
      "partner-token",
      "fixture-app-secret",
    ]
  ) assert(!text.includes(secret));
  await f.call({ action: "key", dsn: DSN });
  assert(f.calls.filter((c) => c.url.endsWith("/auth/login")).length === 1);
});
Deno.test("begin/complete binds setup proof to owner DSN expiry and idempotent registration", async () => {
  const f = fixture({ owned: false });
  const ticket = await (await f.call({ action: "begin", dsn: DSN })).json();
  assert(/^[a-f0-9]{32}$/.test(ticket.setup_token));
  assert(
    (await f.call({
      action: "complete",
      dsn: "ACOTHER1234567",
      ticket: ticket.ticket,
    })).status === 400,
  );
  assert(
    (await f.call({
      action: "complete",
      dsn: DSN,
      ticket: ticket.ticket + "X",
    })).status === 400,
  );
  assert(
    (await f.call({ action: "complete", dsn: DSN, ticket: ticket.ticket }))
      .status === 200,
  );
  assert(
    (await f.call({ action: "complete", dsn: DSN, ticket: ticket.ticket }))
      .status === 200,
  );
  const registrations = f.calls.filter((c) =>
    c.url.endsWith("/devices.json") && c.body
  );
  assert(
    registrations.length === 1 &&
      registrations[0].body.device.setup_token === ticket.setup_token,
  );
  f.advance();
  assert(
    (await f.call({ action: "complete", dsn: DSN, ticket: ticket.ticket }))
      .status === 400,
  );
});
Deno.test("unowned key lookup never registers and provider failures are sanitized", async () => {
  const unowned = fixture({ owned: false });
  assert((await unowned.call({ action: "key", dsn: DSN })).status === 404);
  assert(!unowned.calls.some((c) => c.url.endsWith("/devices.json") && c.body));
  const f = fixture({ fail: "/auth/login" });
  const response = await f.call({ action: "key", dsn: DSN });
  const text = await response.text();
  assert(response.status === 502 && text === '{"error":"monster_login"}');
});
Deno.test("only the account email and password are secrets", async () => {
  // Unpinned Ayla app credentials fail closed instead of sending a login.
  const unpinned = fixture({
    ayla: { appId: "REPLACE_WITH_MONSTER_APP_ID", appSecret: "x" },
  });
  const response = await unpinned.call({ action: "key", dsn: DSN });
  assert(response.status === 503);
  assert(!unpinned.calls.some((c) => c.url.endsWith("/token_sign_in")));

  // Tickets are bound to the account credentials: a password change (or a
  // different deployment) cannot complete a ticket issued before it.
  const before = fixture({ owned: false });
  const ticket = await (await before.call({ action: "begin", dsn: DSN }))
    .json();
  const after = fixture({
    owned: false,
    env: { MONSTER_PASSWORD: "rotated-password" },
  });
  assert(
    (await after.call({ action: "complete", dsn: DSN, ticket: ticket.ticket }))
      .status === 400,
  );
  const same = fixture({ owned: false });
  assert(
    (await same.call({ action: "complete", dsn: DSN, ticket: ticket.ticket }))
      .status === 200,
  );
});
Deno.test("unsupported model and missing key fail closed", async () => {
  assert(
    (await fixture({ model: "unknown-product" }).call({
      action: "key",
      dsn: DSN,
    })).status === 422,
  );
  assert(
    (await fixture({ missingKey: true }).call({ action: "key", dsn: DSN }))
      .status === 502,
  );
  for (
    const ip of [
      "127.0.0.1",
      "169.254.169.254",
      "8.8.8.8",
      "192.168.1.999",
      "example.com",
      "172.32.1.1",
    ]
  ) assert(!privateIp(ip));
  for (const ip of ["192.168.1.1", "10.1.2.3", "172.16.1.1"]) {
    assert(privateIp(ip));
  }
});

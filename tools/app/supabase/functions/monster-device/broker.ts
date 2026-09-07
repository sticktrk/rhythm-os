// Monster credentials and provider tokens stay in this cloud-only module.
// No credential values or provider bodies are included in errors or logs.
type Env = (name: string) => string | undefined;
type Json = Record<string, any>;
type AylaApp = { appId: string; appSecret: string };
type Dependencies = {
  env: Env;
  fetch: typeof fetch;
  now?: () => number;
  ayla?: AylaApp;
};
const DEVICE = "https://ads-field.aylanetworks.com";
// Ayla application credentials of the Monster Gen2 app. They identify the
// vendor app (not a person) and ship inside the public Monster build, so they
// are pinned here; only the shared account email and password are secrets.
const AYLA_APP: AylaApp = {
  appId: "RGBIC-yQ-id",
  appSecret: "REMOVED_PRIVATE_VALUE",
};
const encoder = new TextEncoder();
const dsnPattern = /^[A-Za-z0-9]{8,32}$/;
class Failure extends Error {
  constructor(readonly stage: string, readonly status = 502) {
    super(stage);
  }
}
function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
    },
  });
}
async function boundedText(
  body: ReadableStream<Uint8Array> | null,
  limit: number,
) {
  if (!body) throw new Failure("invalid_response");
  const reader = body.getReader();
  const parts: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > limit) {
        await reader.cancel();
        throw new Failure("body_too_large", 413);
      }
      parts.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return new TextDecoder().decode(bytes);
}
function b64(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll(
    "/",
    "_",
  ).replaceAll("=", "");
}
function unb64(value: string) {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw new Failure("invalid_ticket", 400);
  return Uint8Array.from(
    atob(value.replaceAll("-", "+").replaceAll("_", "/")),
    (c) => c.charCodeAt(0),
  );
}
export function createMonsterBroker(deps: Dependencies) {
  const now = deps.now ?? Date.now;
  let cached: { token: string; expires: number } | undefined;
  let pending: Promise<string> | undefined;
  const ayla = deps.ayla ?? AYLA_APP;
  function env(name: string): string {
    const value = deps.env(name);
    if (!value) throw new Failure("cloud_not_configured", 503);
    return value;
  }
  function aylaApp(): AylaApp {
    if (
      !ayla.appId || !ayla.appSecret || ayla.appId.startsWith("REPLACE_WITH") ||
      ayla.appSecret.startsWith("REPLACE_WITH")
    ) throw new Failure("cloud_not_configured", 503);
    return ayla;
  }
  async function request(
    url: string,
    stage: string,
    signal: AbortSignal,
    body?: unknown,
    auth?: string,
  ): Promise<any> {
    try {
      const headers: Record<string, string> = {
        "Content-Type": "application/json",
        Accept: "application/json",
      };
      if (url.includes(".bycopilot.com/")) {
        headers["x-copilot-sdk-version"] = "6.0.6";
      }
      if (auth) headers.Authorization = auth;
      const res = await deps.fetch(url, {
        method: body === undefined ? "GET" : "POST",
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: "error",
        signal,
      });
      if (!res.ok) {
        if (res.status === 401) cached = undefined;
        await res.body?.cancel();
        throw new Failure(stage);
      }
      return JSON.parse(await boundedText(res.body, 2 * 1024 * 1024));
    } catch (e) {
      // Keep a typed failure's own status (for example 413 from boundedText);
      // everything else collapses to this stage's bounded 502.
      throw e instanceof Failure ? e : new Failure(stage);
    }
  }
  async function authenticate(signal: AbortSignal): Promise<string> {
    if (cached && cached.expires > now() + 60000) return cached.token;
    if (pending) return pending;
    pending = (async () => {
      const token = await request(
        "https://api.monstergen2.bycopilot.com/v4/auth/login",
        "monster_login",
        signal,
        {
          authenticationDetails: {
            applicationId: "MONSTERGEN2",
            email: env("MONSTER_EMAIL"),
            password: env("MONSTER_PASSWORD"),
          },
          deviceDetails: {
            applicationVersion: "1.0.45(3)",
            deviceId: crypto.randomUUID(),
            deviceModel: "Rhythm cloud broker",
            deviceType: "PHONE",
            osType: "ANDROID",
            osVersion: "15",
            timezone: {
              currentTimeInClientInMilliseconds: now(),
              offsetFromUTCInMilliseconds: 0,
              timeZoneId: "UTC",
            },
          },
        },
      );
      if (typeof token.accessToken !== "string" || !token.accessToken) {
        throw new Failure("monster_login");
      }
      const ticket = await request(
        "https://sphere.bycopilot.com/v2/partner/162fa71e-46d6-4cc6-9eab-db1925fdcb30/acquire_ticket",
        "partner_ticket",
        signal,
        { applicationId: "MONSTERGEN2" },
        `${token.tokenType || "Bearer"} ${token.accessToken}`,
      );
      if (typeof ticket.partnerTicket !== "string" || !ticket.partnerTicket) {
        throw new Failure("partner_ticket");
      }
      const session = await request(
        "https://user-field.aylanetworks.com/api/v1/token_sign_in",
        "ayla_login",
        signal,
        {
          token: ticket.partnerTicket,
          app_id: aylaApp().appId,
          app_secret: aylaApp().appSecret,
        },
      );
      if (typeof session.access_token !== "string" || !session.access_token) {
        throw new Failure("ayla_login");
      }
      const expires = Number(session.expires_in);
      cached = {
        token: session.access_token,
        expires: now() +
          Math.min(Number.isFinite(expires) ? Math.max(0, expires) : 0, 300) *
            1000,
      };
      return session.access_token;
    })();
    try {
      return await pending;
    } finally {
      pending = undefined;
    }
  }
  async function ticketKey() {
    // Derived from the shared account credentials so no third secret is
    // needed. Changing the password invalidates outstanding setup tickets.
    const material = encoder.encode(
      `rhythm-monster-ticket-v1\n${env("MONSTER_EMAIL")}\n${
        env("MONSTER_PASSWORD")
      }`,
    );
    const digest = await crypto.subtle.digest("SHA-256", material);
    return crypto.subtle.importKey(
      "raw",
      digest,
      { name: "HMAC", hash: "SHA-256" },
      false,
      ["sign", "verify"],
    );
  }
  async function issueTicket(userId: string, dsn: string) {
    const token = Array.from(
      crypto.getRandomValues(new Uint8Array(16)),
      (b) => b.toString(16).padStart(2, "0"),
    ).join("");
    const data = encoder.encode(
      JSON.stringify({
        user_id: userId,
        dsn,
        setup_token: token,
        expires: now() + 600000,
      }),
    );
    const signature = new Uint8Array(
      await crypto.subtle.sign("HMAC", await ticketKey(), data),
    );
    return { setup_token: token, ticket: `${b64(data)}.${b64(signature)}` };
  }
  async function verifyTicket(
    value: unknown,
    userId: string,
    dsn: string,
  ): Promise<string> {
    try {
      if (typeof value !== "string" || value.length > 2048) throw new Error();
      const parts = value.split(".");
      if (parts.length !== 2) throw new Error();
      const data = unb64(parts[0]);
      const signature = unb64(parts[1]);
      if (
        !await crypto.subtle.verify("HMAC", await ticketKey(), signature, data)
      ) throw new Error();
      const ticket = JSON.parse(new TextDecoder().decode(data));
      if (
        ticket.user_id !== userId || ticket.dsn !== dsn ||
        typeof ticket.expires !== "number" || ticket.expires <= now() ||
        ticket.expires > now() + 600000 ||
        !/^[a-f0-9]{32}$/.test(ticket.setup_token)
      ) throw new Error();
      return ticket.setup_token;
    } catch {
      throw new Failure("invalid_ticket", 400);
    }
  }
  async function ownedDevice(
    dsn: string,
    auth: string,
    signal: AbortSignal,
  ): Promise<Json | undefined> {
    const devices = await request(
      `${DEVICE}/apiv1/devices.json`,
      "device_lookup",
      signal,
      undefined,
      auth,
    );
    if (!Array.isArray(devices)) throw new Failure("device_lookup");
    return devices.map((row) => row?.device).find((device) =>
      device?.dsn === dsn
    );
  }
  async function credentials(
    device: Json,
    dsn: string,
    auth: string,
    signal: AbortSignal,
  ) {
    const properties = await request(
      `${DEVICE}/apiv1/dsns/${dsn}/properties.json`,
      "capabilities",
      signal,
      undefined,
      auth,
    );
    if (!Array.isArray(properties)) throw new Failure("capabilities");
    const props = new Map(
      properties.map((row) => [row?.property?.name, row?.property]),
    );
    const models =
      (deps.env("MONSTER_MODEL_ALLOWLIST") || "xt-16ft-hw-neon-led-rgbic")
        .split(",").map((s) => s.trim());
    if (!models.includes(props.get("device_id")?.value)) {
      throw new Failure("unsupported_model", 422);
    }
    const required: Record<string, string> = {
      power: "boolean",
      brightness: "integer",
      color_bright: "integer",
      color_select: "integer",
      color_saturation: "integer",
      mode: "string",
    };
    for (const [name, type] of Object.entries(required)) {
      const p = props.get(name);
      if (p?.base_type !== type || p?.read_only !== false) {
        throw new Failure("unsupported_capabilities", 422);
      }
    }
    const config = await request(
      `${DEVICE}/apiv1/devices/${dsn}/connection_config.json`,
      "lan_key",
      signal,
      undefined,
      auth,
    );
    if (
      typeof config.local_key !== "string" || config.local_key.length < 8 ||
      config.local_key.length > 128 ||
      !Number.isInteger(config.local_key_id) || config.local_key_id < 0 ||
      config.local_key_id > 0xffffffff
    ) throw new Failure("lan_key");
    const ip = device.lan_ip;
    if (typeof ip !== "string" || !privateIp(ip)) {
      throw new Failure("device_not_on_lan", 409);
    }
    return {
      dsn,
      ip,
      local_key: config.local_key,
      local_key_id: config.local_key_id,
    };
  }
  return async (req: Request, userId: string): Promise<Response> => {
    try {
      if (req.method !== "POST") {
        return response({ error: "method_not_allowed" }, 405);
      }
      // Default: the configured Monster account is shared, so any authenticated
      // Rhythm user can commission a light without creating their own vendor
      // login. Setting MONSTER_OWNER_USER_ID narrows the broker to that one
      // Supabase user; every other caller is refused before any vendor call.
      const owner = deps.env("MONSTER_OWNER_USER_ID");
      if (owner && userId !== owner) {
        return response({ error: "forbidden" }, 403);
      }
      let body: Json;
      try {
        body = JSON.parse(await boundedText(req.body, 4096));
      } catch {
        return response({ error: "invalid_request" }, 400);
      }
      if (
        !body || typeof body.dsn !== "string" || !dsnPattern.test(body.dsn) ||
        !["begin", "complete", "key"].includes(body.action)
      ) return response({ error: "invalid_request" }, 400);
      const signal = AbortSignal.timeout(35000);
      let setupToken: string | undefined;
      if (body.action === "complete") {
        setupToken = await verifyTicket(body.ticket, userId, body.dsn);
      }
      // Deliberate: "begin" only needs the ticket secret, but logging in first
      // surfaces a misconfigured or revoked vendor account before the caller
      // spends a BLE provisioning attempt on a ticket it can never complete.
      const auth = `auth_token ${await authenticate(signal)}`;
      if (body.action === "begin") {
        return response(await issueTicket(userId, body.dsn));
      }
      let device = await ownedDevice(body.dsn, auth, signal);
      if (!device && setupToken) {
        // setup_token is proof provisioned into the exact DSN over BLE.
        const registered = await request(
          `${DEVICE}/apiv1/devices.json`,
          "register",
          signal,
          { device: { dsn: body.dsn, setup_token: setupToken } },
          auth,
        );
        if (registered?.device?.dsn !== body.dsn) {
          throw new Failure("registration_identity");
        }
        device = registered.device;
      }
      if (!device) return response({ error: "device_not_owned" }, 404);
      return response(await credentials(device, body.dsn, auth, signal));
    } catch (e) {
      const failure = e instanceof Failure ? e : new Failure("internal", 500);
      return response({ error: failure.stage }, failure.status);
    }
  };
}
export function privateIp(value: string): boolean {
  const parts = value.split(".");
  if (
    parts.length !== 4 ||
    parts.some((p) => !/^\d{1,3}$/.test(p) || Number(p) > 255)
  ) return false;
  const [a, b] = parts.map(Number);
  return a === 10 || (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168);
}

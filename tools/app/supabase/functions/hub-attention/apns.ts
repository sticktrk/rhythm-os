export type ApnsConfig = {
  keyId: string;
  teamId: string;
  privateKey: string;
  bundleId: string;
};

function base64Url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-")
    .replaceAll("/", "_").replace(/=+$/, "");
}

async function signProviderToken(
  config: ApnsConfig,
  now: number,
): Promise<string> {
  const encoder = new TextEncoder();
  const header = base64Url(
    encoder.encode(JSON.stringify({ alg: "ES256", kid: config.keyId })),
  );
  const claims = base64Url(
    encoder.encode(
      JSON.stringify({ iss: config.teamId, iat: Math.floor(now / 1000) }),
    ),
  );
  const input = header + "." + claims;
  const der = config.privateKey.replaceAll("\\n", "\n")
    .replace(/-----BEGIN PRIVATE KEY-----|-----END PRIVATE KEY-----|\s/g, "");
  const key = await crypto.subtle.importKey(
    "pkcs8",
    Uint8Array.from(atob(der), (c) => c.charCodeAt(0)),
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"],
  );
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    encoder.encode(input),
  );
  return input + "." + base64Url(new Uint8Array(signature));
}

// Reuse a signed token (and any in-flight signing) across requests to a warm worker.
export function apnsProvider(config: ApnsConfig, now: () => number) {
  let cached: { signed: Promise<string>; issuedAt: number } | undefined;
  return async () => {
    const time = now();
    if (!cached || time - cached.issuedAt >= 50 * 60_000) {
      cached = { issuedAt: time, signed: signProviderToken(config, time) };
    }
    const entry = cached;
    try {
      return await entry.signed;
    } catch (error) {
      if (cached === entry) cached = undefined;
      throw error;
    }
  };
}

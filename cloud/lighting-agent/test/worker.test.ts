import assert from "node:assert/strict";
import { mkdtempSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Miniflare, convertV4MiniflareOptions } from "miniflare";
import jpeg from "jpeg-js";

const actor = crypto.randomUUID(),
  home = crypto.randomUUID(),
  hub = crypto.randomUUID();
const refs = [
  "r_" + "a".repeat(24),
  "r_" + "b".repeat(24),
  "r_" + "c".repeat(24),
];

test("resident turns, image evidence, two-phone claims, lost replies and erasure use the Workers runtime", async () => {
  let allowed = true,
    reservations = 0;
  const directory = mkdtempSync(join(tmpdir(), "lighting-agent-test-"));
  const create = () =>
    new Miniflare(
      convertV4MiniflareOptions({
        resourcePersistencePath: directory,
        isolatedResourcePersistencePath: directory,
        workers: [
          {
            name: "lighting",
            modules: true,
            script: readFileSync(".wrangler/test-worker/index.js", "utf8"),
            compatibilityDate: "2026-10-01",
            compatibilityFlags: ["nodejs_compat"],
            bindings: {
              SUPABASE_URL: "https://fixture.invalid",
              SUPABASE_PUBLISHABLE_KEY: "public",
              ACCEPT_NEW_TURNS: "true",
              PILOT_PERCENT: "100",
            },
            durableObjects: {
              CONVERSATIONS: {
                className: "LightingConversation",
                useSQLite: true,
              },
              RESIDENT_INDEX: {
                className: "ConversationIndex",
                useSQLite: true,
              },
            },
            serviceBindings: { AI: "fixture-ai" },
            outboundService: async (req: Request) => {
              assert.equal(new URL(req.url).hostname, "fixture.invalid");
              if (req.headers.get("authorization") !== "Bearer resident")
                return Response.json({}, { status: 401 });
              const body = (await req.json()) as {
                action: string;
                home: string;
                instance: string;
              };
              if (body.action === "identity") return Response.json({ actor });
              if (!allowed || body.home !== home || body.instance !== "fixture")
                return Response.json({}, { status: 403 });
              if (body.action === "reserve") reservations++;
              return Response.json({ actor, hub });
            },
          },
          {
            name: "fixture-ai",
            modules: true,
            compatibilityDate: "2026-10-01",
            script:
              'import {WorkerEntrypoint} from "cloudflare:workers"; export default class extends WorkerEntrypoint {async run(model,input){const last=input.messages.at(-1).content;return {response:{text:"Lighting advice",steps:typeof last==="string"?' +
              JSON.stringify(
                refs.slice(0, 2).map((target) => ({
                  target,
                  operation: "adjust_room_brightness",
                  arguments: { delta_percent: -10 },
                })),
              ) +
              ":[]}};}}",
          },
        ],
      }),
    );
  let mf = create();
  let session = "";
  const post = async (op: string, data: unknown = {}, auth = "resident") => {
    const form = data instanceof FormData ? new Response(data) : null;
    return mf.dispatchFetch("https://worker.test/v1/" + op, {
      method: "POST",
      headers: {
        authorization: "Bearer " + auth,
        "x-rhythm-home": home,
        "x-rhythm-installation": "fixture",
        "x-rhythm-session": session,
        "content-type": form?.headers.get("content-type") ?? "application/json",
      },
      body: form
        ? new Uint8Array(await form.arrayBuffer())
        : JSON.stringify(data),
    });
  };
  try {
    const bad = await post("open", {}, "other");
    assert.equal(bad.status, 401, await bad.text());
    let res = await post("open");
    assert.equal(res.status, 200, await res.clone().text());
    session = ((await res.json()) as any).session;
    const turn = {
      turn: crypto.randomUUID(),
      messages: [{ role: "user", text: "Dim both rooms except the hall" }],
      targets: refs.map((ref, i) => ({
        ref,
        tools: i < 2 ? ["adjust_room_brightness"] : [],
      })),
    };
    res = await post("turn", turn);
    assert.equal(res.status, 200, await res.clone().text());
    const { plan } = (await res.json()) as any;
    assert.deepEqual(
      plan.steps.map((s: any) => s.target),
      refs.slice(0, 2),
    );
    assert.equal((await post("turn", turn)).status, 409);
    assert.equal(reservations, 1);
    const body = { plan: plan.id, index: 0, digest: "f".repeat(64) };
    assert.equal((await post("claim", body)).status, 409);
    assert.equal((await post("prepare", body)).status, 200);
    const [a, b] = await Promise.all([
      post("claim", body),
      post("claim", body),
    ]);
    assert.deepEqual([a.status, b.status].sort(), [200, 409]);
    const { claim } = (await (a.status === 200 ? a : b).json()) as any;
    assert.equal(
      (await post("receipt", { ...body, claim, outcome: "acknowledged" }))
        .status,
      200,
    );
    assert.equal((await post("prepare", { ...body, index: 1 })).status, 200);
    assert.equal((await post("claim", { ...body, index: 1 })).status, 200);
    await mf.dispose();
    mf = create(); // New runtime process, same SQLite storage.
    const state = (await (await post("status")).json()) as any;
    assert.deepEqual(
      state.plan.steps.map((s: any) => s.status),
      ["acknowledged", "claimed"],
    );
    assert.equal(state.plan.steps[1].nonce, undefined);
    assert.equal((await post("claim", { ...body, index: 1 })).status, 409);
    allowed = false;
    assert.equal((await post("status")).status, 403);
    allowed = true;
    const pixels = jpeg.encode(
      { width: 2, height: 2, data: Buffer.alloc(16, 230) },
      80,
    ).data;
    const imageTurn = {
      ...turn,
      turn: crypto.randomUUID(),
      messages: [{ role: "user", text: "" }],
    };
    const photo = () => {
      const f = new FormData();
      f.set("turn", JSON.stringify(imageTurn));
      f.set("photo", new Blob([pixels], { type: "image/jpeg" }), "room.jpg");
      return f;
    };
    const invalid = photo();
    invalid.set(
      "photo",
      new Blob([new Uint8Array([1, 2, 3])], { type: "image/jpeg" }),
      "bad.jpg",
    );
    assert.equal((await post("photo", invalid)).status, 400);
    const large = photo();
    large.set(
      "photo",
      new Blob([new Uint8Array(1700000)], { type: "image/jpeg" }),
      "large.jpg",
    );
    assert.equal((await post("photo", large)).status, 413);
    const originalSession = session;
    session = crypto.randomUUID();
    assert.equal((await post("photo", photo())).status, 410);
    session = originalSession;
    res = await post("photo", photo());
    assert.equal(res.status, 200, await res.clone().text());
    assert.equal(((await res.json()) as any).proposed, false);
    assert.equal((await post("photo", photo())).status, 409);
    assert.equal(reservations, 2);
    assert.equal(
      ((await (await post("status")).json()) as any).lastResult,
      "photo_advice",
    );
    assert.equal((await post("clear")).status, 200);
    assert.equal((await post("status")).status, 410);
    session = ((await (await post("open")).json()) as any).session;
    assert.equal((await post("forget")).status, 200);
    await new Promise((resolve) => setTimeout(resolve, 120));
    assert.equal((await post("status")).status, 410);
    assert.equal((await post("open")).status, 410);
  } finally {
    await mf.dispose();
    rmSync(directory, { recursive: true, force: true });
  }
});

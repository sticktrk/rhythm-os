// Exercise the HTTP entry point and real Supabase clients with an in-memory
// Auth/PostgREST transport. The database transaction has a separate SQL test.
import { handleRequest } from "./handler.ts";

const actor = "00000000-0000-4000-8000-000000000001";
const homeId = "00000000-0000-4000-8000-000000000002";
const hubId = "00000000-0000-4000-8000-000000000003";
function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function fixture(
  reviewResponse?: (step: string, args: Record<string, unknown>) => unknown,
) {
  let authCalls = 0;
  const seen: string[] = [];
  const backend = {
    url: "https://fixture.invalid",
    publicKey: "fixture-public",
    serviceKey: "fixture-service",
    fetch: async (input: string | Request | URL, init?: RequestInit) => {
      const req = new Request(input, init);
      const url = new URL(req.url);
      const path = url.pathname;
      const bearer = req.headers.get("authorization");
      seen.push(path);
      if (path === "/auth/v1/user") {
        authCalls++;
        if (bearer === "Bearer auth-offline") {
          return Response.json({ message: "backend unavailable" }, {
            status: 500,
          });
        }
        if (bearer !== "Bearer fixture-resident") {
          return Response.json({ message: "expired" }, { status: 401 });
        }
        return Response.json({
          id: actor,
          aud: "authenticated",
          role: "authenticated",
          is_anonymous: false,
        });
      }
      if (path === "/rest/v1/rpc/customer_assistant_review") {
        equal(bearer, "Bearer fixture-service");
        const body = await req.json();
        equal(body.p_actor, actor);
        if (reviewResponse) {
          return Response.json(reviewResponse(body.p_step, body.p_args));
        }
        return Response.json({
          code: "P0001",
          message: "sensitive database context",
        }, { status: 400 });
      }
      equal(bearer, "Bearer fixture-resident");
      if (path === "/rest/v1/rpc/customer_assistant_home_members") {
        const args = await req.json();
        return Response.json(
          args.p_home_id === homeId
            ? [
              {
                member_id: actor,
                role: "owner",
                display_name: "Housemate name",
                is_self: true,
              },
            ]
            : [],
        );
      }
      if (path === "/rest/v1/homes") {
        equal(url.searchParams.get("member_ids"), `cs.{${actor}}`);
        if (url.searchParams.has("id")) {
          const member = url.searchParams.get("id") === `eq.${homeId}`;
          return Response.json(
            member
              ? [{ id: homeId, owner_id: actor, member_ids: [actor] }]
              : [],
          );
        }
        return Response.json([{
          id: homeId,
          name: "Private home name",
          owner_id: actor,
        }]);
      }
      if (path === "/rest/v1/hubs") {
        equal(url.searchParams.get("home_id"), `eq.${homeId}`);
        equal(url.searchParams.get("id"), `eq.${hubId}`);
        equal(url.searchParams.get("type"), "eq.server");
        return Response.json([{
          id: hubId,
          home_id: homeId,
          name: "Private hub label",
          enabled: false,
          server_instance_id: "saved-hub-instance",
        }]);
      }
      throw new Error(`unexpected fixture route ${path}`);
    },
  };
  const invoke = (
    body: unknown,
    token = "fixture-resident",
    extra: Record<string, string> = {},
  ) =>
    handleRequest(
      new Request("https://edge.invalid/functions/v1/customer-assistant", {
        method: "POST",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
          ...extra,
        },
        body: typeof body === "string" ? body : JSON.stringify(body),
      }),
      backend,
    );
  return { invoke, seen, authCalls: () => authCalls };
}

Deno.test("resident HTTP scopes reads to membership and preserves the device handoff for a saved hub", async () => {
  const { invoke, seen } = fixture();
  const homes = await invoke({ operation: "list_homes" });
  equal(homes.status, 200);
  equal((await homes.json()).homes[0].display_name, "Private home name");
  const mcp = await invoke({
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: { name: "list_homes", arguments: {} },
  });
  const projected = JSON.parse((await mcp.json()).result.content[0].text);
  equal(projected.homes, [{ home_id: homeId, role: "owner" }]);
  const members = await invoke({
    operation: "list_home_members",
    arguments: { home_id: homeId },
  });
  equal((await members.json()).members[0].display_name, "Housemate name");
  const modelMembers = await invoke({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name: "list_home_members", arguments: { home_id: homeId } },
  });
  equal(
    JSON.parse((await modelMembers.json()).result.content[0].text).members,
    [{ member_id: actor, role: "owner", is_self: true }],
  );
  const selected = await invoke({
    operation: "select_hub",
    arguments: { home_id: homeId, hub_id: hubId },
  });
  const receipt = await selected.json();
  equal([selected.status, receipt.actor, receipt.server_instance_id], [
    200,
    "resident_app",
    "saved-hub-instance",
  ]);
  const outside = await invoke({
    operation: "select_hub",
    arguments: { home_id: hubId, hub_id: hubId },
  });
  equal(outside.status, 403);
  equal(seen.filter((p) => p === "/rest/v1/hubs").length, 1);
});

Deno.test("authentication, confirmation, payload bounds and stale receipts fail closed", async () => {
  const { invoke, seen, authCalls } = fixture();
  const denied = await invoke({ contract: true }, "expired");
  equal([denied.status, (await denied.json()).step], [401, "sign_in"]);
  const unavailable = await invoke({ contract: true }, "auth-offline");
  equal([unavailable.status, await unavailable.json()], [503, {
    error: "account_service_unavailable",
  }]);
  const refused = await invoke({
    operation: "apply_customer_plan",
    arguments: { plan_id: hubId, confirmed: false },
  });
  equal(refused.status, 400);
  const stale = await invoke({
    operation: "apply_customer_plan",
    arguments: { plan_id: hubId, confirmed: true },
  });
  equal([stale.status, await stale.json()], [409, {
    error: "review_changed_or_expired",
  }]);
  equal(seen.filter((p) => p.includes("/rpc/")).length, 1);
  equal((await invoke("x".repeat(16385))).status, 413);
  equal(
    (await invoke(
      { jsonrpc: "2.0", id: 1, method: "tools/list" },
      "fixture-resident",
      { origin: "https://outside.invalid" },
    )).status,
    403,
  );
  equal(authCalls(), 6);
});

Deno.test("MCP can hand a review to the resident but cannot claim or complete it", async () => {
  const steps: string[] = [];
  const { invoke } = fixture((step) => {
    steps.push(step);
    return {
      change: "remove_member",
      plan_id: hubId,
      status: "awaiting_device",
    };
  });
  const call = async (name: string, args: object) =>
    (await invoke({
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: { name, arguments: args },
    })).json();
  const result = await call("apply_customer_plan", {
    plan_id: hubId,
    confirmed: true,
  });
  equal(JSON.parse(result.result.content[0].text).status, "awaiting_device");
  for (const name of ["device_claim", "device_complete"]) {
    equal((await call(name, { plan_id: hubId })).error.code, -32602);
  }
  equal(steps, ["confirm"]);
});

Deno.test("support revocation and remote setup disclose future support approval", async () => {
  const { invoke } = fixture((_step, args) => ({
    ...args,
    status: "review",
    plan_id: actor,
  }));
  for (const change of ["revoke_support", "enable_remote"]) {
    const plan = await invoke({
      operation: "plan_customer_change",
      arguments: { change, home_id: homeId, hub_id: hubId },
    });
    equal(plan.status, 200);
    equal(
      (await plan.json()).description.includes(
        "After this change, Rhythm support access for this hub is granted only when a resident reviews and approves it.",
      ),
      true,
    );
  }
});

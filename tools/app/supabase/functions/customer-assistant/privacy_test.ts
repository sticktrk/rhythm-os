import { type Fields, operations, Rejection } from "./contract.ts";
import { mcp } from "./mcp.ts";
import { modelResult } from "./privacy.ts";

const home = "00000000-0000-4000-8000-000000000001";
const hub = "00000000-0000-4000-8000-000000000002";
const member = "00000000-0000-4000-8000-000000000003";
const plan = "00000000-0000-4000-8000-000000000004";
const when = "2026-11-01T11:22:33.123Z";
const privateValue = "private-canary";
const prohibited = {
  name: privateValue,
  display_name: privateValue,
  address: privateValue,
  email: privateValue,
  token: privateValue,
  encrypted_hub_token: privateValue,
  staff_credential: privateValue,
  signed_storage_url:
    "https://storage.example.test/private-canary?token=private-canary",
  server_instance_id: privateValue,
  report_summary: privateValue,
  motion_history: [privateValue],
  arguments: { bearer: privateValue },
  unknown_future_field: privateValue,
};
function assertSame(actual: unknown, expected: unknown) {
  const normalize = (value: unknown): unknown =>
    Array.isArray(value)
      ? value.map(normalize)
      : value !== null && typeof value === "object"
      ? Object.fromEntries(
        Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map((
          [key, item],
        ) => [key, normalize(item)]),
      )
      : value;
  if (
    JSON.stringify(normalize(actual)) !== JSON.stringify(normalize(expected))
  ) {
    throw new Error("Unexpected provider-visible fields or values");
  }
}

async function toolResult(
  name: string,
  execute: (name: string, args: unknown) => Promise<Fields>,
) {
  const result = await mcp({
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: { name, arguments: {} },
  }, execute);
  return await result.json();
}

Deno.test("every customer MCP tool emits exactly its approved provider fields", async () => {
  const action = {
    ...prohibited,
    change: "revoke_support",
    status: "review",
    plan_id: plan,
    hub_id: hub,
    home_id: home,
    member_id: member,
    expires_at: when,
    include_bundle: false,
    requires_confirmation: true,
    previous_grant_active: true,
    description: privateValue,
    operation: privateValue,
  };
  const publicAction = {
    status: "review",
    change: "revoke_support",
    plan_id: plan,
    hub_id: hub,
    home_id: home,
    member_id: member,
    expires_at: when,
    include_bundle: false,
    requires_confirmation: true,
    previous_grant_active: true,
    operation: "apply_customer_plan",
    description: "Revoke the active Rhythm staff support grant for this hub.",
  };
  const rows: Record<string, [Fields, Fields]> = {
    list_homes: [{ homes: [{ ...prohibited, home_id: home, role: "owner" }] }, {
      homes: [{ home_id: home, role: "owner" }],
    }],
    list_home_hubs: [{
      hubs: [{ ...prohibited, home_id: home, hub_id: hub, enabled: true }],
    }, { hubs: [{ home_id: home, hub_id: hub, enabled: true }] }],
    list_home_members: [{
      members: [{
        ...prohibited,
        member_id: member,
        role: "member",
        is_self: false,
      }],
    }, { members: [{ member_id: member, role: "member", is_self: false }] }],
    select_hub: [{ home_id: home, hub_id: hub, instructions: privateValue }, {
      home_id: home,
      hub_id: hub,
      status: "handoff",
      actor: "resident_app",
      instructions: "Continue on this hub using the signed-in Rhythm app.",
    }],
    get_account_handoff: [{ step: "sign_in", instructions: privateValue }, {
      status: "handoff",
      actor: "resident",
      step: "sign_in",
      surface: "rhythm_app",
    }],
    read_account_status: [
      { plan: "pro", has_backup: true, captured_at: when },
      { plan: "pro", has_backup: true, captured_at: when },
    ],
    list_pending_actions: [{ actions: [action] }, { actions: [publicAction] }],
    plan_customer_change: [action, publicAction],
    apply_customer_plan: [action, publicAction],
    cancel_customer_plan: [{ ...action, status: "cancelled" }, {
      status: "cancelled",
      plan_id: plan,
    }],
  };
  assertSame(Object.keys(rows).sort(), operations.map((op) => op.id).sort());
  for (const [name, [raw, allowed]] of Object.entries(rows)) {
    const response = await toolResult(
      name,
      () => Promise.resolve({ ...prohibited, ...raw }),
    );
    const content = response.result.content;
    assertSame(content.length, 1);
    assertSame(JSON.parse(content[0].text), allowed);
    assertSame(response, {
      jsonrpc: "2.0",
      id: 1,
      result: {
        isError: false,
        content: [{ type: "text", text: content[0].text }],
      },
    });
  }
});

Deno.test("untrusted values cannot leak through allowed keys, timestamps or future operations", async () => {
  const invalid = {
    ...prohibited,
    role: privateValue,
    plan: privateValue,
    enabled: privateValue,
    captured_at: privateValue,
    has_backup: privateValue,
    home_id: privateValue,
    hub_id: privateValue,
    member_id: privateValue,
    plan_id: privateValue,
    status: privateValue,
    operation: privateValue,
    change: "__proto__",
    step: privateValue,
    expires_at: privateValue,
    include_bundle: { token: privateValue },
    requires_confirmation: privateValue,
    previous_grant_active: [privateValue],
    is_self: privateValue,
    description: privateValue,
  };
  for (const op of operations) {
    const response = await toolResult(
      op.id,
      () =>
        Promise.resolve({
          ...invalid,
          actions: [invalid],
          homes: [invalid],
          hubs: [invalid],
          members: [invalid],
        }),
    );
    if (JSON.stringify(response).includes(privateValue)) {
      throw new Error("Prohibited value leaked from " + op.id);
    }
  }
  assertSame(modelResult("unknown_operation", invalid), {});
  assertSame(
    JSON.parse(
      JSON.stringify(
        modelResult("read_account_status", {
          captured_at: "2026-11-01T11:22:33." + "9".repeat(50) + "Z",
        }),
      ),
    ),
    {},
  );
});

Deno.test("customer MCP publishes bounded error codes for typed and unexpected failures", async () => {
  for (
    const error of [new Error(privateValue), new Rejection(400, privateValue)]
  ) {
    const response = await toolResult(
      "list_homes",
      () => Promise.reject(error),
    );
    assertSame(response.result, {
      isError: true,
      content: [{ type: "text", text: "account_service_unavailable" }],
    });
  }
  const stale = await toolResult(
    "apply_customer_plan",
    () => Promise.reject(new Rejection(409, "review_changed_or_expired")),
  );
  assertSame(stale.result.content, [{
    type: "text",
    text: "review_changed_or_expired",
  }]);
});

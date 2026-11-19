// Resident operations execute in the account service. Only the device uses the
// enrolled hub credential. Staff operations are served by the staff API.
export type Fields = Record<string, unknown>;
type Schema = Record<string, unknown>;
const id = { type: "string", format: "uuid" };
const text = (maxLength: number) => ({
  type: "string",
  minLength: 1,
  maxLength,
});
const choice = (values: string[]) => ({ type: "string", enum: values });
export const changes = {
  grant_support:
    "Grant Rhythm staff support access to this hub, including diagnostics and authorized changes. The resident can revoke this in hub settings.",
  revoke_support:
    "Revoke the active Rhythm staff support grant for this hub. After this change, Rhythm support access for this hub is granted only when a resident reviews and approves it.",
  remove_member:
    "Remove this member from the home. Their access to the home and its hubs ends.",
  enable_remote:
    "Set up remote access to this hub. The phone will configure the device and check reachability. After this change, Rhythm support access for this hub is granted only when a resident reviews and approves it.",
  submit_report:
    "Send the reviewed report to Rhythm support, with the selected diagnostic bundle only if included.",
  join_home:
    "Join the home verified by the selected hub using local device proof on the resident’s phone.",
};
const handoffs = [
  "sign_in",
  "password_recovery",
  "plan_change",
  "account_deletion",
];
function descriptor(
  name: string,
  description: string,
  properties: Fields = {},
  required: string[] = Object.keys(properties),
  tier = "read",
) {
  return {
    id: name,
    description,
    location: "cloud",
    tier,
    method: "POST",
    path: "customer-assistant",
    effect: name === "cancel_customer_plan"
      ? "discard_review"
      : tier === "read"
      ? "read_only"
      : "persistent_change",
    confirmation: tier === "read" ? "none" : "explicit_user_confirmation",
    freshness_precondition: tier === "read"
      ? "live_account"
      : "actor_and_reviewed_resource",
    verification: "account_or_device_receipt",
    physical_confirmation_required: false,
    input_schema: {
      type: "object",
      additionalProperties: false,
      properties,
      required,
    },
    result_schema: { type: "object" },
  };
}
export const operations = [
  descriptor(
    "list_homes",
    "List the signed-in resident’s homes. Resolve the intended home before selecting a hub.",
  ),
  descriptor(
    "list_home_hubs",
    "List Rhythm servers in one of the resident’s homes.",
    { home_id: id },
  ),
  descriptor(
    "list_home_members",
    "Show owner and member identities in the selected home.",
    { home_id: id },
  ),
  descriptor(
    "select_hub",
    "Continue on this hub. The resident app connects with its existing device key.",
    { home_id: id, hub_id: id },
  ),
  descriptor(
    "read_account_status",
    "Read the signed-in resident’s effective plan and backup status.",
  ),
  descriptor(
    "get_account_handoff",
    "Tell the resident where to sign in, recover access, manage a plan or delete an account.",
    { step: choice(handoffs) },
  ),
  descriptor(
    "list_pending_actions",
    "Resume a reviewed account action on the same home and hub. An already claimed action requires verification before another attempt.",
  ),
  descriptor(
    "plan_customer_change",
    "Preview a membership, support access, remote setup, verified home join or bug report change. Review the scope, report and bundle choice with the resident.",
    {
      change: choice(Object.keys(changes)),
      home_id: id,
      hub_id: id,
      member_id: id,
      server_instance_id: text(128),
      report_summary: text(1500),
      include_bundle: { type: "boolean" },
    },
    ["change"],
  ),
  descriptor(
    "cancel_customer_plan",
    "Discard your pending review on all devices before execution. This changes only the proposal.",
    { plan_id: id },
  ),
  descriptor(
    "apply_customer_plan",
    "Apply a current review only after the person explicitly confirms. Device work returns an awaiting-device receipt until the resident app performs it.",
    { plan_id: id, confirmed: { type: "boolean", const: true } },
    undefined,
    "persistent_change",
  ),
];
export const uuid = (v: unknown): v is string =>
  typeof v === "string" &&
  /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(v);
export function object(v: unknown): v is Fields {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}
export class Rejection extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}
export function validated(name: string, args: unknown): Fields {
  const op = operations.find((op) => op.id === name);
  if (!op) throw new Rejection(404, "unknown_operation");
  if (!object(args)) throw new Rejection(400, "invalid_arguments");
  const { properties, required } = op.input_schema;
  if (required.some((key) => !(key in args))) {
    throw new Rejection(400, "missing_arguments");
  }
  for (const [key, value] of Object.entries(args)) {
    if (!Object.hasOwn(properties, key)) {
      throw new Rejection(400, "unknown_argument");
    }
    const schema = properties[key] as Schema;
    if (
      typeof value !== schema.type ||
      (schema.format === "uuid" && !uuid(value)) ||
      (Array.isArray(schema.enum) && !schema.enum.includes(value)) ||
      ("const" in schema && value !== schema.const) ||
      (typeof value === "string" &&
        (!value.trim() || value.length > Number(schema.maxLength ?? 128)))
    ) {
      throw new Rejection(400, "invalid_argument");
    }
  }
  if (name === "plan_customer_change") {
    const change = args.change as keyof typeof changes;
    const fields = change === "join_home"
      ? ["server_instance_id"]
      : change === "remove_member"
      ? ["home_id", "member_id"]
      : change === "submit_report"
      ? ["home_id", "hub_id", "report_summary", "include_bundle"]
      : ["home_id", "hub_id"];
    const allowed = new Set([
      "change",
      ...fields,
      ...(change === "join_home" ? ["home_id", "hub_id"] : []),
    ]);
    if (
      fields.some((k) => !(k in args)) ||
      Object.keys(args).some((k) => !allowed.has(k))
    ) throw new Rejection(400, "invalid_change_scope");
  }
  return args;
}
export function reviewCopy(row: Fields): Fields {
  const change = row.change as keyof typeof changes;
  return {
    ...row,
    description: changes[change],
    apply_arguments: { plan_id: row.plan_id },
  };
}

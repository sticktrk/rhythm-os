// Model-bound data uses explicit projections. Resident display names, report
// text, endpoint URLs and household activity remain on the human-facing path.
import {
  changes,
  credentialNextStep,
  type Fields,
  object,
  uuid,
} from "./contract.ts";
const oneOf = (v: unknown, choices: string[]) =>
  typeof v === "string" && choices.includes(v) ? v : undefined;
const pickId = (v: unknown) => uuid(v) ? v : undefined;
const bool = (v: unknown) => typeof v === "boolean" ? v : undefined;
function iso(v: unknown) {
  // Accept Postgres timestamp offsets and emit a fixed-size UTC timestamp.
  // User-controlled strings cannot carry extra data in a fractional suffix.
  if (
    typeof v !== "string" ||
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:\d{2})$/
      .test(v)
  ) return undefined;
  const time = Date.parse(v);
  return Number.isFinite(time) ? new Date(time).toISOString() : undefined;
}
function projected(action: Fields): Fields {
  const change = oneOf(action.change, Object.keys(changes));
  return {
    status: oneOf(action.status, [
      "review",
      "awaiting_device",
      "claimed",
      "applied",
      "acknowledged",
      "uncertain",
      "cancelled",
    ]),
    change,
    home_id: pickId(action.home_id),
    hub_id: pickId(action.hub_id),
    member_id: pickId(action.member_id),
    plan_id: pickId(action.plan_id),
    include_bundle: bool(action.include_bundle),
    previous_grant_active: bool(action.previous_grant_active),
    next_step: credentialNextStep(action),
    operation: "apply_customer_plan",
    requires_confirmation: bool(action.requires_confirmation),
    expires_at: iso(action.expires_at),
    description: change ? changes[change as keyof typeof changes] : undefined,
  };
}
export function modelResult(operation: string, raw: Fields): Fields {
  const rows = (v: unknown) => Array.isArray(v) ? v.filter(object) : [];
  switch (operation) {
    case "list_homes":
      return {
        homes: rows(raw.homes).map((r) => ({
          home_id: pickId(r.home_id),
          role: oneOf(r.role, ["owner", "member"]),
        })),
      };
    case "list_home_hubs":
      return {
        hubs: rows(raw.hubs).map((r) => ({
          home_id: pickId(r.home_id),
          hub_id: pickId(r.hub_id),
          enabled: bool(r.enabled),
        })),
      };
    case "list_home_members":
      return {
        members: rows(raw.members).map((r) => ({
          member_id: pickId(r.member_id),
          role: oneOf(r.role, ["owner", "member"]),
          is_self: bool(r.is_self),
        })),
      };
    case "select_hub":
      return {
        status: "handoff",
        actor: "resident_app",
        home_id: pickId(raw.home_id),
        hub_id: pickId(raw.hub_id),
        instructions: "Continue on this hub using the signed-in Rhythm app.",
      };
    case "get_account_handoff":
      return {
        status: "handoff",
        actor: "resident",
        step: oneOf(raw.step, [
          "sign_in",
          "password_recovery",
          "plan_change",
          "account_deletion",
        ]),
        surface: "rhythm_app",
      };
    case "read_account_status":
      return {
        plan: oneOf(raw.plan, ["basic", "pro"]),
        has_backup: bool(raw.has_backup),
        captured_at: iso(raw.captured_at),
      };
    case "list_pending_actions":
      return { actions: rows(raw.actions).map(projected) };
    case "cancel_customer_plan":
      return {
        status: oneOf(raw.status, ["cancelled"]),
        plan_id: pickId(raw.plan_id),
      };
    case "plan_customer_change":
    case "apply_customer_plan":
      return projected(raw);
    default:
      return {};
  }
}

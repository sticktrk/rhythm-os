# Account and staff assistant contracts

The account service resolves a resident's homes, membership and hubs. Lighting
control runs on the selected hub. The staff API resolves customer records and
reaches a hub through its active support grant.

| Caller | Entry point | Authority |
| --- | --- | --- |
| Resident app or resident MCP client | `POST /functions/v1/customer-assistant` | The signed-in resident's Supabase access token. The service verifies the user and applies current household membership. |
| Staff console or staff MCP client | `POST /api/assistant/mcp` on the admin API | An enabled staff identity on each request. Hub calls also require the customer's grant and matching live hub identity. Writes require the admin role. |
| Resident app after hub selection | Selected local or authorized remote hub connection | The app's enrolled device credential. The account service never needs this credential. |

MCP clients use Streamable HTTP, list the tools, and read the contract resource
advertised by the service. In the resident app, use Settings → Rhythm assistant.
Select an accessible home and hub to continue the conversation on that same
physical installation. Open the selected hub's assistant in the staff console
for grant-backed diagnostics and hub operations. A missing grant, unsupported
older hub, or unavailable connection is reported at the step that needs it.

## Reviewed changes

Resident previews are bound to a signed-in user, target, installation identity
and resource revision. Reviews expire after ten minutes. Confirming any proposed change returns an awaiting-device receipt. Membership
removal and support revocation run in a locked database transaction only after
the resident approves the reviewed scope in the app.
The app checks the displayed scope and claims the action once. Device operations validate the live hub before dispatch. Text-only reports can be sent while the hub is offline. Cancelling an unclaimed review discards it across the resident’s devices. A repeated confirmation
retrieves the current state. A lost response or interrupted device action
requires checking the result before starting another proposal. Remote provisioning rechecks the reviewed resources under the database locks when it consumes the dispatch permission.

A support grant authorizes the staff service to obtain a short-lived device
session. Revocation also disables automatic regrant from other phones. Remote
setup alone cannot opt a hub back into support. Support tokens are encrypted
on the staff service's existing grant path. The separate owner credential
stays in the resident's device credential store.

Residents review the support report text and diagnostic attachment choice
before submission. The private submission uses the review UUID so its receipt
can be checked against the correct report. An accepted asynchronous bundle
may still be collecting. Sign-in, password recovery, billing and account
deletion are handoffs to the resident.

## Model and telemetry boundary

The built-in resident and staff conversations run on their respective clients
using guided steps and live operation descriptors. Neither calls a hosted model
provider. The resident's trusted view can show household labels and their
reviewed report. The staff console can open authorized diagnostics separately.

An external MCP client chooses and operates its own model. The account MCP
response projection exposes opaque IDs, enumerated roles and outcomes, boolean
flags, normalized timestamps and published guidance. The staff projection uses
operator-scoped references and summarized diagnostic categories. Display names,
addresses, report text, raw log lines, household motion history, auth secrets
and signed URLs stay outside these cloud MCP results. Users of external clients
control what they type into their own prompts. A direct hub MCP connection can
return local room names and device evidence for setup. Configure a trusted
client when granting direct hub access.

Apply the Supabase migration before deploying the customer assistant and the
updated support-grant and remote-bootstrap functions. The admin API, app and
hub can discover support independently. Existing customers can continue using
their established local and remote controls while versions differ.

[Cloud coverage ledger](cloud-assistant-coverage.tsv) ·
[Hub contract](light-assistant-contract.md) ·
[Staff API](../../admin-api/README.md)

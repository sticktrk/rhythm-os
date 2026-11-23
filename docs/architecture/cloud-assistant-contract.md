# Account and staff assistant contracts

The account service resolves a resident's homes, membership and hubs. Lighting
control runs on the selected hub. The staff API resolves customer records and
reaches a hub through its active support grant.

| Caller | Entry point | Authority |
| --- | --- | --- |
| Resident app or resident MCP client | `POST /functions/v1/customer-assistant` | The signed-in resident's Supabase access token. The service verifies the user and applies current household membership. |
| Staff console or staff MCP client | `POST /api/assistant/mcp` on the admin API | An enabled staff identity on each request. Hub calls also require the customer's grant and matching live hub identity. Household observations and writes require the admin role. |
| Resident app after hub selection | Selected local or authorized remote hub connection | The app's enrolled device credential. The account service never needs this credential. |

MCP clients use Streamable HTTP, list the tools, and read the contract resource
advertised by the service. In the resident app, use Settings → Rhythm assistant.
Select an accessible home and hub to continue the conversation on that same
physical installation. Open the selected hub's assistant in the staff console
for grant-backed diagnostics and hub operations. A missing grant, unsupported
older hub, or unavailable connection is reported at the step that needs it.

## Reviewed changes

Resident previews are bound to a signed-in user, target, installation identity
and resource revision. A review expires after ten minutes. When the phone claims it, the device gets
at least ten more minutes to finish. Confirming any proposed change returns an
awaiting-device receipt. Membership
removal and support revocation run in a locked database transaction only after
the resident approves the reviewed scope in the app.
The app checks the displayed scope and claims the action once. Device operations validate the live hub before dispatch. Text-only reports can be sent while the hub is offline. Cancelling an unclaimed review discards it across the resident’s devices. A repeated confirmation
retrieves the current state. A lost response or interrupted device action
requires checking the result before starting another proposal. Remote provisioning rechecks the reviewed resources under the database locks when it consumes the dispatch permission.

A staff MCP client can prepare a change and request console approval. Its
`staff_apply_plan` call returns `awaiting_console_approval`. The same
administrator opens the selected hub’s pending plan in the staff console,
reviews its scope, and approves it. The console approval endpoint requires an
allowed browser origin and consumes the plan once. The service rechecks the
hub, its installation identity, and any required customer grant before dispatch.

The trusted hub catalog identifies reads that reveal household observations,
including motion evidence. Those reads require an enabled administrator and
the customer’s grant. The staff service filters them from a support operator’s
catalog and the device proxy enforces the same role check on direct requests.

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

Guided tools in the resident app and staff console run locally. Rhythm Pro also
offers a hosted lighting chat. A signed-in resident pairs their phone once on
the local network using the hub owner's authorization. The phone stores its
revocable agent credential in device secure storage scoped to the account and
installation. Away from home the hub also requires an enabled tunnel and Remote
MCP. Guided tools remain available for older hubs and account or motion tasks.

The phone discovers rooms and tools over the hub's MCP endpoint. It reads the
selected room before each turn. The authenticated customer assistant function
sends up to eight recent conversation messages, selected numerical or
enumerated lighting readings and supported adjustment types to OpenAI with
response storage disabled. Hub and account credentials, topology, device
names and raw readbacks stay on the phone. Room names typed by the resident
are part of their message. Changing rooms starts a new conversation.

A model can propose brightness, warmth, power, color or a curve preview. The
phone validates the arguments, shows the exact target and calls the selected
hub only after a tap. Its receipt distinguishes the hub's target from lamp
readback. The app can undo a preview using the hub-issued trial ID.

The Edge Function uses the server secret `OPENAI_API_KEY` and defaults to
`gpt-4.1-mini`. `RHYTHM_CHAT_MODEL` can select a Responses API model that
supports strict function calling. Apply the lighting chat usage migration
before deployment. The free preview allows 60 model requests per resident per
UTC day. The database keeps one counter per account and deletes it with the
account. Closing chat discards its transcript.

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

# Light Assistant Contract

The Light Assistant Contract is the machine-readable boundary between Rhythm's
authoritative lighting system and conversational or automated configuration
clients. A live LLM may consume the contract, but it is never the source of
truth for device identity, topology, capabilities, or mutations.

## Ownership

`rhythm-os` owns the contract because the appliance owns topology, canonical
device identity, integration coordination, persistence, and command outcomes.
The Dart SDK is a tolerant consumer. Flutter, a cloud model gateway, support
tools, and test harnesses must use the SDK contract rather than duplicate route
or capability knowledge.

The authority order remains:

1. physical or integration observation;
2. canonical server state derived from observation;
3. server command outcome;
4. command acceptance;
5. client or LLM proposal.

An assistant plan is therefore a proposal. It has no authority until the
server validates and applies it.

## Runtime resources

### `GET /api/assistant/contract`

Returns the contract envelope and operation descriptors. The envelope has a
schema version for parser compatibility and a `contract_sha256` derived from
the descriptor content. Changing an operation's method, path, effects,
confirmation, precondition, verification, physical-confirmation rule, or JSON
schemas changes the hash automatically.

The server build version is diagnostic metadata. Clients gate behavior on the
contract schema and advertised operation IDs, never on version-string guesses.

### `GET /api/assistant/topology`

Returns a fresh public topology node array with:

- the durable server instance ID;
- the current contract hash;
- server version and observation time;
- `topology_resource_sha256`, calculated from exactly the returned node array.

Display names are present because a user-facing setup flow needs them. They
must not be copied into analytics, logs, plan IDs, or model-provider telemetry
unless a separately reviewed privacy contract requires it.

### Running an operation

The same descriptor drives MCP discovery and the app's typed SDK. Each entry
includes an operation ID, location, tier, input and result schemas, and an
invocation path. The app and per-hub staff console discover the selected
server's catalog and send JSON to `POST /api/assistant/operations/{id}`.
An MCP client uses `tools/call` with the same ID and arguments. Both receive
the same operation result. The account and fleet services own the operations
that run outside the hub.

Read operations include previews. A temporary-output tool can change lights
immediately. A persistent tool accepts explicit confirmation and the arguments
from its preview. Older device-room move endpoints retain their original
request format for existing clients. Owner actions return a handoff that
identifies the person, the surface to open and the hub on which to resume.

| Resident's request | Tools and evidence |
| --- | --- |
| “A little brighter” or “warmer at this brightness” | Read the current room output, then use the brightness or warmth adjustment. Check the returned per-light evidence. |
| “Try this color on one lamp” | Use `audition_light` with the delegated lamp ID and its output. The trial expires after one minute. Use its trial ID to end it early. Lamps with a room-only route require a room-wide preview. |
| “Keep the kitchen brighter after dusk” | Inspect the active mode and profile. Preview a room tuning and compare dusk and later samples before applying it. |
| “Show me an hour earlier” | Try a negative offset. Undo the returned trial, keep a room offset, or review Absorb's shared profile and offset changes. |
| “Why didn't the hall turn on?” | Start a short walk for the chosen sensor. Inspect detection, targets, room policy, dispatch and fresh light observations. Review a target repair, then walk again. |
| “Change my saved lighting” | Discover scene, schedule, transition, profile, binding and room-preference plans. The preview names the affected settings and the apply arguments. |

The catalog also covers room and device management, pairing and triage, HA
managed-light selection, portable settings, activity uploads and appliance
maintenance. Sensitive pairing, HA-owned configuration, installation backups,
full restore and factory reset direct the person to the appropriate app or HA
surface. Portable lighting exports contain household labels for mapping and
belong in the resident's trusted client.

The route ledger records the classification of each shared, platform and add-on
HTTP route. Run `tools/check-repo-invariants.sh` when changing routes. Regenerate
the staff proxy's read classification with
`cargo run --quiet -p rhythm-os --example assistant_read_policy > admin-api/lib/src/assistant_read_operations.dart`.

[Hub route coverage](assistant-route-coverage.tsv)

### Device-room move plan and apply

`POST /api/assistant/plans/device-room-move` is read-only. It validates current
canonical identity and room existence, previews the before/after placement,
and returns a content-bound plan with the server, contract, and topology
preconditions needed for apply.

`POST /api/assistant/plans/device-room-move/apply` accepts only the reviewed
plan fields. It rejects:

- a different contract hash;
- a different plan identity;
- a different server instance;
- any change to the public topology resource;
- a different source placement.

The topology and source checks run inside the existing external topology
transaction before native preparation and again before mutation. Successful
apply reuses canonical assignment's integration preparation/readback/rollback,
coupled topology/canonical persistence, runtime reconciliation, and refresh
events. The receipt is produced after canonical readback verifies the new
parent and `UserOverride` placement.

HTTP success, canonical readback, and physical confirmation are separate
outcomes. Moving a device requires no immediate physical confirmation.
Identify explicitly requires the user to confirm which light flashed; a 204
Identify response proves only server acknowledgement.

## Compatibility and freshness

Assistant routes are additive. A previous appliance returns 404 for contract
discovery; the SDK maps only that response to `null` so callers can render a
clear unsupported state. Other transport failures remain errors and must not
be misreported as unsupported.

Previous clients ignore the routes. Current clients must refresh and re-plan
after a 409. Cached contracts may support offline explanation, but no
persistent assistant write may proceed without live server, contract, and
topology validation.

Stale-plan 409 responses guarantee that no assistant mutation was attempted.
An unclassified 5xx apply failure is conservative: its structured error sets
`mutation_may_have_applied=true`, is not retryable, and directs the client to
refresh authoritative topology before proposing another action. This covers
failures that can occur after the existing assignment path has durably
committed but before the assistant can produce its verified receipt.

Contract and plan resources are not persisted or backed up. The authoritative
topology is already part of Rhythm's installation backup. After restart,
restore, integration resync, or upgrade, clients obtain a fresh contract and
snapshot before planning again.

## Adding or changing an operation

A feature that changes an assistant answer or action must update the executable
contract in the same change:

1. Add or update the operation descriptor next to the `rhythm-os` handler.
2. Keep input and result schemas bounded and reject unknown write fields.
3. Declare effect, confirmation, freshness, verification, rollback, and
   physical-confirmation semantics.
4. Add the route to the shared route registry and every active server adapter.
5. Add typed SDK models/methods and previous-appliance behavior.
6. Carry server identity, resource hashes, and one bounded correlation ID on
   persistent writes.
7. Add tests for serialization, auth/route registration, rejection, stale
   state, authoritative readback, mixed versions, and prohibited analytics
   values.
8. Run the add-on route tests and the client parity check. CI starts a fresh
   standalone server and shares its live contract with the Dart SDK, Flutter
   tool picker, staff proxy and admin console tests. To reproduce with the
   package dependencies installed, run the following from the repository root.

   ```sh
   cargo build --locked -p rhythm-server
   python3 tools/ci/smoke-server.py --assistant-contract /tmp/hub-contract.json
   tools/ci/check-assistant-clients.sh /tmp/hub-contract.json
   cargo test --locked -p rhythm-addon mcp_on_mobile_listener
   ```

Do not expose a generic topology-write or arbitrary light-command tool. Each
assistant operation must be narrow enough for deterministic validation and an
unambiguous receipt.

## Analytics and diagnostics

Contract and snapshot reads are not product actions. Successful persistent
operations record one accepted backend outcome through the established
Supabase-backed activity path. Future Flutter surfaces add PostHog discovery,
attempt, and terminal events at the UI boundary.

Prompts, transcripts, names, native IDs, addresses, credentials, resource
hashes, plan bodies, and free-form errors are forbidden analytics properties.
Receipts and sanitized logs retain only the opaque plan/correlation IDs,
operation ID, bounded status, and verification outcome needed to reconstruct a
field failure.

## Agent connections

The appliance serves stateless MCP Streamable HTTP at `/api/assistant/mcp`.
Each POST carries a bearer credential issued specifically for the agent by a
local authenticated owner. The same URL works on the authorized HTTPS tunnel
and on the HA add-on's mobile listener. The SDK discovers the contract on the
selected app connection. Staff use the selected hub's audited support proxy.

Home Assistant's MCP Client accepts Streamable HTTP and exposes tools to an
Assist conversation agent. Its 2026.11 setup accepts an OAuth client ID and
secret. A plain bearer-header input is absent in this release. Use a scoped
agent credential for integrations with static bearer support. Keep every
credential in the transport layer, outside transcripts and tool arguments.

[Home Assistant MCP Client](https://www.home-assistant.io/integrations/mcp/) ·
[2026.11 client setup](https://github.com/home-assistant/core/blob/2026.11.2/homeassistant/components/mcp/config_flow.py) ·
[MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)

A local owner creates a bearer with `POST /api/auth/agent-tokens`, lists
registrations with `GET /api/auth/agent-tokens`, and revokes one by sending
`DELETE /api/auth/agent-tokens/{id}`. Revocation applies to the next request,
including on installations that ordinarily allow local control without a key.

For Home Assistant, create a registration with `POST /api/auth/assistant-clients`.
In **Settings → Devices & services → Add integration → Model Context Protocol
Client**, enter the add-on mobile URL ending in `/api/assistant/mcp`. Use the
HTTPS tunnel when connecting across networks. Enter the issued client ID and
secret, then paste the activation code on Rhythm's consent screen within ten
minutes. This flow uses Home Assistant's built-in My Home Assistant redirect.
Enable the Rhythm tools on the selected conversation agent. Home Assistant Assist uses the MCP tools and their descriptions. Its MCP client does not read the contract resource. The access token
lasts one hour. Refresh rotates the token, and deleting the registration revokes
both access and refresh. Agent grants occupy separate fields in the existing
auth store so an older installation can still read its owner credentials.

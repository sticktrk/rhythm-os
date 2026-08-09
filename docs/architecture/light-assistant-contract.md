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
8. Run the assistant-contract parity check.

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

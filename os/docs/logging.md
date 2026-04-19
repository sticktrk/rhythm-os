# Logging Strategy

Rhythm logs are an operator tool first. A useful log line should quickly answer one of these:

- What triggered this action?
- What target did Rhythm choose?
- Did the engine skip, dispatch, or fail?
- Where did the latency come from?

## Principles

- Prefer structured `tracing` events for new high-signal logs.
- Keep top-level `target` values coarse and stable.
- Log one event per boundary, not one event per branch.
- Treat credentials, pairing payloads, and provisioning data as redact-only.

## Stable Targets

- `sys`: process lifecycle, runtime startup, periodic summaries
- `http`: request/response spans and request IDs
- `evt`: inbound motion, button, SSE/WS event handling
- `cmd`: node actions and downstream dispatch to integrations
- `conn`: transport connection health and reconnects
- `room_sync`: discovery and topology sync
- `pair`: pairing and unpairing lifecycle
- `triage`: device review / triage workflow

Protocol-specific detail belongs in fields such as `hub_type=hue` or `integration=ha`, not in new top-level targets unless the category is broadly useful.

## Correlation

- HTTP requests carry `x-request-id` and the request span logs it.
- Internal actions that are not request-originated should create a `command_id`.
- Follow one chain:
  ingress event -> target resolution -> engine action -> dispatch -> transport result

## Required Fields For New Boundary Logs

- `event`
- `latency_ms` when timing exists
- `room_id` or `node_id` when acting on a target
- transport or hub identifiers when dispatching externally
- `reason` or `skip_reason` for non-success outcomes

## Level Rules

- `error`: user-visible failure or failed server-side request handling
- `warn`: degraded behavior, partial dispatch, retries, slow commands, client misuse worth attention
- `info`: state transitions, request completion, motion activation completion, periodic summaries
- `debug`: per-dispatch detail and verbose diagnostics
- `trace`: raw payloads and hot-loop noise

Successful high-frequency dispatches should generally stay at `debug` unless they cross a slow threshold.

## Redaction

- Never log full pairing or credentials payloads.
- Summarize JSON payloads by shape or key list instead.
- Avoid logging tokens, passwords, Wi-Fi secrets, or raw bearer headers.

## Native Runtime Controls

- `RUST_LOG` controls filtering.
- `RHYTHM_LOG_FORMAT` supports `full`, `compact`, or `json`.
- Native HTTP responses emit request-completion logs with `request_id`, `status`, and `latency_ms`.

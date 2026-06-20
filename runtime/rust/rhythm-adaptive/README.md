# rhythm-adaptive

App-runtime crate for Rhythm's existing adaptive lighting behavior.

This crate is the neutral `LightingRuntime` entrypoint for the current Rhythm
engine. The deeper curve, profile, room-state, and button-planning primitives
still live in `os/rust/core/rhythm-core`; this crate keeps the app-runtime
surface separate from the shared OS host so Rhythm can sit beside other light
apps such as removed-project.

The intended direction is:

- `rhythm-os`: host concerns such as topology, hubs, dispatch, persistence,
  HTTP/auth, event ingress, queues, and logging
- `rhythm-runtime-api`: shared runtime contract
- `rhythm-adaptive`: Rhythm app behavior
- other runtime crates: other light apps

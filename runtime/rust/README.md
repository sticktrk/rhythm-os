# Rhythm Runtime Crates

This workspace contains runtime crates that can be hosted by Rhythm OS without
being merged into the production OS crates.

## Crates

- `rhythm-runtime-api`: neutral host/runtime contract. It defines snapshots,
  input events, periodic ticks, dispatch commands, state writes, diagnostics,
  and the `LightRuntime` trait.
- `removed-circadian`: removed-project/Circadian implementation ported from the legacy
  Python add-on.
- `rhythm-adaptive`: Rhythm's existing adaptive light runtime, hosted
  behind the same neutral contract while it still reuses the production
  `rhythm-core` engine primitives.
- `rhythm-os-runtime-modules`: the default product module bundle. It links the
  concrete runtime crates into the server/add-on/appliance builds and registers
  them with the generic `rhythm-os` host registry.

## Boundary

The OS host owns:

- topology discovery and stable node ids
- durable state storage
- timers and periodic tick scheduling
- input event routing
- physical dispatch execution
- auth, HTTP, persistence, and production service concerns

Runtime crates own:

- domain-specific state interpretation
- domain-specific input action vocabulary
- lighting calculations
- dispatch gating decisions
- conversion of events into neutral `RuntimePlan` values
- runtime-local topology semantics such as removed-project zones and sections, while
  still expressing all host effects as neutral dispatches/state writes
- runtime-defined extension manifests for settings and advanced APIs

Runtime crates must not import production OS internals. Shared functionality
belongs in `rhythm-runtime-api` only when it is generally useful to multiple
runtime crates.

## Module Registration

`rhythm-os` does not hard-code concrete runtime implementations. It owns a
generic registry of `LightRuntimeModule` descriptors:

- runtime id and legacy aliases
- manifest factory
- instance lifecycle, either ephemeral per call or cached/stateful

The default server, add-on, and appliance binaries depend on
`rhythm-os-runtime-modules` and call `install_default_light_runtime_modules()`
during startup. That bundle currently registers:

- `rhythm-adaptive`
- `removed-circadian`

To add another runtime to the default product build:

1. Create a runtime crate that implements `rhythm_runtime_api::LightRuntime`.
2. Add the crate as a dependency of `runtime/rust/rhythm-os-runtime-modules`.
3. Add one `LightRuntimeModule` descriptor to that bundle.

Do not edit `os/rust/core/rhythm-os` for a new runtime. If the runtime needs
deeper host behavior, add an explicit host/runtime capability to
`rhythm-runtime-api` or implement runtime-specific glue inside the module
bundle. Avoid reaching into `rhythm-os` internals from runtime crates.

## Runtime Extensions

Runtime-specific settings and advanced APIs are declared by the runtime crate
through `LightRuntime::manifest()`. The OS host should mount those endpoints
under the runtime namespace:

```text
/api/light-runtimes/{runtime_id}/...
```

For example, removed-project/Circadian can declare `/scope`, `/settings`, and
`/areas/{area_id}/action`, which the host exposes as:

```text
/api/light-runtimes/removed-circadian/scope
/api/light-runtimes/removed-circadian/settings
/api/light-runtimes/removed-circadian/areas/{area_id}/action
```

The runtime defines the settings schema, default payload, endpoint list, and
extension handler. The OS still owns auth, HTTP mounting, request decoding,
persistence, dispatch application, logging, and diagnostics. Runtime extension
handlers should return JSON plus optional neutral `RuntimePlan` values; they
must not bypass the OS to reach hubs, storage, or controllers directly.

If an extension endpoint becomes useful across runtimes, promote it into the
core OS API instead of duplicating it under multiple runtime namespaces.

## Verification

Compile-check the runtime workspace:

```sh
cargo check --manifest-path runtime/rust/Cargo.toml
```

`removed-circadian` includes tests, including a local Python parity test that
compares selected curve outputs against
`legacy/removed-project/addon` when that ignored reference checkout
is present. Test execution is intentionally deferred for the current rebuild
pass.

Format with:

```sh
cargo fmt --all --manifest-path runtime/rust/Cargo.toml
```

The module bundle depends back on `rhythm-os`, so it is intentionally excluded
from the pure runtime workspace. It is compiled through the OS workspace:

```sh
cargo check -p rhythm-server -p rhythm-addon -p rhythm-linux-appliance --manifest-path os/Cargo.toml
```

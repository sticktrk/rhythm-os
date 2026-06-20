# rhythm-os-runtime-modules

Default light runtime module bundle for Rhythm OS product builds.

This crate is intentionally outside `os/rust/core/rhythm-os`: the OS host owns
the generic registry and plan execution, while this bundle owns which concrete
runtime crates are linked into a build.

To add a runtime to the default build, add its crate dependency here and append
a `LightRuntimeModule` descriptor in `src/lib.rs`. Do not edit `rhythm-os`
source for per-runtime registration.

Use `LightRuntimeModule::ephemeral` for stateless host adapters and
`LightRuntimeModule::cached` for runtimes that need retained state or
topology-derived rebuild logic.

# Repository guidance

This is the public Rhythm OS source repository. Preserve unrelated changes and
use focused commits. Keep configuration generic and secrets in ignored local
environment files. Never add customer data, production logs or private support
bundles to source or tests. Do not deploy or publish releases without an explicit
user request. Preserve the Apache-2.0 license and third-party notices.

The Rust workspace is at the root; Flutter is in `app/flutter/rhythm_app` and
its Dart SDK dependency is in `sdk`. Run `tools/check-repo-invariants.sh` and
focused tests for your changes. Use synthetic fixtures for device/cloud tests.

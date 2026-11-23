# Repository guidance

This is the public Rhythm OS source repository. Preserve unrelated changes and
use focused commits. Keep configuration generic and secrets in ignored local
environment files. Never add customer data, production logs or private support
bundles to source or tests. Do not deploy or publish releases without an explicit
user request. Preserve the Apache-2.0 license and third-party notices.

## Tests

During development, run only the narrowest test that covers the change: one
test, one file, one crate, or one package. Do not run the full test corpus
while iterating. Run that corpus once, as a final check, before handing work
off or opening a pull request. CI runs it on every push and pull request.

Rust workspace, from the repository root:

```text
cargo test -p <crate> <filter>
```

Flutter and Dart, one file. `--name <test>` runs a single test inside the file:

```text
cd app/flutter/rhythm_app && flutter test test/<file>_test.dart
cd sdk && dart test test/<file>_test.dart
cd admin-api && dart test test/<file>_test.dart
```

Final check only:

```bash
cargo test --workspace --all-targets --locked
cd app/flutter/rhythm_app && flutter test
cd sdk && dart test
cd admin-api && dart test
```

The Rust workspace is at the repository root. Flutter is in
`app/flutter/rhythm_app` and its Dart SDK dependency is in `sdk`.
`tools/check-repo-invariants.sh` checks source boundaries, licenses, and
deployment fixtures. Use synthetic fixtures for device and cloud tests.

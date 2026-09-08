# Contributing

Open an issue or pull request in this repository with the intended behavior,
a focused change and the tests you ran. Keep unrelated formatting and generated
build output out of changes. Follow existing Rust, Flutter and Dart patterns.

Run `./tools/check-repo-invariants.sh` and the focused tests for affected code.
For Flutter changes, run `flutter analyze` from `app/flutter/rhythm_app`.
For backend changes, run the relevant Rust or Edge Function tests. Explain any
platform or hardware validation that remains outstanding.

Use normal Git branches, commits and pushes, then open a pull request here.
Every push and PR runs public checks on GitHub-hosted runners without staff
credentials: secret scanning, repository boundaries, Edge Function fixtures, portable Rust
workspace tests, Flutter analysis/tests, Dart SDK/API tests and the admin UI
build/tests. The `Public contributor checks` result summarizes those jobs.
Native Matter SDK builds and physical-device verification remain separate from
these portable contributor checks.

Do not commit environment files, signing keys, private deployment configuration,
real customer logs, support bundles or internal investigation material. Use
synthetic data in tests. Preserve third-party licenses and notices.

Contributions to first-party code are under Apache-2.0, as described in LICENSE.

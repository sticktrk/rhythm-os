# Build Scripts

Scripts for building the Rhythm Lighting Flutter app.

## Quick Reference

```bash
# Development
./tools/app/scripts/test-flutter-web.sh           # Run Flutter web in browser
./tools/app/scripts/android-doctor.sh             # Check Android SDK/toolchain readiness
./tools/app/scripts/android-setup.sh              # Create/patch Flutter Android scaffold
./tools/app/scripts/android-run.sh                # Run on Android device/emulator

# Build
./tools/app/scripts/build-wasm.sh                 # Build WASM for Flutter
./tools/app/scripts/build-flutter-web.sh          # Build Flutter web app
./tools/app/scripts/build-mobile.sh --ios         # Build for iOS
./tools/app/scripts/build-mobile.sh --codegen     # Regenerate FRB bindings
./tools/app/scripts/android-build.sh --debug      # Build Android debug APK
./tools/app/scripts/android-build.sh --release    # Build Android release APK
./tools/app/scripts/android-build.sh --aab        # Build Android App Bundle for Play
```

---

## test-flutter-web.sh

Run the Flutter web app in development mode with hot reload. Automatically builds WASM if not present.

```bash
./tools/app/scripts/test-flutter-web.sh                # Run in Chrome (builds WASM if needed)
./tools/app/scripts/test-flutter-web.sh --port 3000    # Custom port
./tools/app/scripts/test-flutter-web.sh --edge         # Use Edge browser
./tools/app/scripts/test-flutter-web.sh --web-server   # Headless (no browser)
./tools/app/scripts/test-flutter-web.sh --skip-wasm    # Skip WASM check
```

---

## build-wasm.sh

Build WASM module for Flutter web (uses flutter_rust_bridge).

```bash
./tools/app/scripts/build-wasm.sh
```

**Prerequisites:**
- `wasm-pack`: `cargo install wasm-pack`
- `flutter_rust_bridge_codegen`: `cargo install flutter_rust_bridge_codegen`
- Rust nightly with `wasm32-unknown-unknown` target

**Output:**
- `dist/wasm/`
- `flutter/rhythm_app/web/pkg/`

---

## build-flutter-web.sh

Build the Flutter web UI.

```bash
./tools/app/scripts/build-flutter-web.sh              # Full build (includes WASM)
./tools/app/scripts/build-flutter-web.sh --skip-wasm  # Skip WASM rebuild
```

**Output:** `dist/flutter_web/`

---

## build-mobile.sh

Build for iOS, Android, or macOS. Also handles FRB codegen.

Signing requires your own Apple project team and local `APPLE_ID`, `TEAM_ID`,
`SIGNING_IDENTITY` and `KEYCHAIN_PROFILE` settings. No company signing identity
is supplied. Use your own bundle identifiers for store distribution.

```bash
./tools/app/scripts/build-mobile.sh --ios              # Build + run on iOS
./tools/app/scripts/build-mobile.sh --android          # Build + run on Android
./tools/app/scripts/build-mobile.sh --macos            # Build + run on macOS
./tools/app/scripts/build-mobile.sh --codegen --clean  # Regenerate FRB bindings
./tools/app/scripts/build-mobile.sh --ipa             # Build IPA for TestFlight
./tools/app/scripts/build-mobile.sh --testflight       # Notes are optional
./tools/app/scripts/build-mobile.sh --dmg             # Create macOS DMG
./tools/app/scripts/build-mobile.sh --sign            # Signed + notarized DMG
RHYTHM_BUILD_NUMBER=123 ./tools/app/scripts/build-mobile.sh --ipa  # Override build number
```

IPA/TestFlight builds auto-inject the next Apple build number. The script uses `--build-number`,
then `RHYTHM_BUILD_NUMBER`, then the latest TestFlight build for the current
`flutter/rhythm_app/pubspec.yaml` version plus one. Other release builds use common CI run-number
variables, then the `+build` value from `pubspec.yaml`. It does not fall back to the current time.
If TestFlight has no builds for the current version, the first build number is `1`.
The normal interactive Xcode export remains the first path. If Flutter creates
the archive but cannot see an Xcode account or distribution certificate during
IPA export, the script retries that export with the configured App Store
Connect API key and managed signing before uploading.

Direct TestFlight uploads may include build-specific tester notes:

```bash
./tools/app/scripts/build-mobile.sh --testflight \
  --testflight-notes-file /absolute/path/to/what-to-test.txt
```

Fastlane partially waits for App Store Connect processing so it can attach this
copy to the exact uploaded build before the command succeeds. If the option and
`RHYTHM_TESTFLIGHT_NOTES_FILE` are both omitted, the upload leaves the changelog
unset.

To see the latest uploaded TestFlight build for a version:

```bash
fastlane run latest_testflight_build_number \
  api_key_path:"$HOME/.config/rhythm/asc_api_key.json" \
  app_identifier:"lighting.rhythm.app" \
  version:"2.0.3"
```

---

## Android scripts

Prepare, check, run, and build the Flutter Android app.

```bash
./tools/app/scripts/android-doctor.sh
./tools/app/scripts/android-setup.sh
./tools/app/scripts/android-run.sh                 # Auto-detect first Android device/emulator
./tools/app/scripts/android-run.sh --device <device-id>
./tools/app/scripts/android-build.sh --debug
./tools/app/scripts/android-build.sh --release
./tools/app/scripts/android-build.sh --aab
```

First-time Android machine setup:

1. Install Android Studio.
2. Install the Android SDK, platform-tools, command-line tools, and an Android emulator or connect a device.
3. If Flutter cannot find the SDK, run `flutter config --android-sdk <path-to-android-sdk>`.
4. Run `./tools/app/scripts/android-doctor.sh --accept-licenses --install-rust-targets`.
5. Run `./tools/app/scripts/android-setup.sh`.

Release signing:

```bash
./tools/app/scripts/android-keystore-create.sh
./tools/app/scripts/android-build.sh --aab
```

The Android scaffold uses application id `lighting.rhythm.app`, enables local HTTP traffic for RhythmServer/HomeAssistant/Hue discovery, declares camera/location/Bluetooth/network permissions, and keeps the `rhythmapp://` callback scheme.

---

## generate-apple-secret.sh

Generate Apple client secret for Sign in with Apple.

---

## Troubleshooting

### WASM build fails
```bash
rustup target add wasm32-unknown-unknown
rustup component add rust-src --toolchain nightly
cargo install wasm-pack
```

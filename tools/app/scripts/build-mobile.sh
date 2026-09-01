#!/bin/bash
# Build and run Flutter app on mobile/desktop (iOS/Android/macOS)
#
# Usage: ./tools/app/scripts/build-mobile.sh [options]
# Options:
#   --ios              Build for iOS (default if on macOS)
#   --android          Build for Android
#   --macos            Build for macOS desktop
#   --release          Build in release mode
#   --ipa              Build IPA for TestFlight (implies --release --no-run)
#   --testflight       Build IPA and upload to TestFlight (implies --clean --release --no-run)
#   --testflight-notes-file FILE  Optional "What to Test" notes for a TestFlight upload
#   --aab              Build AAB for Google Play (implies --android --release --no-run)
#   --googleplay       Build AAB and upload to Google Play (implies --clean --release --no-run)
#   --release-all      Run --testflight then --googleplay (ship to both stores)
#   --setup-android-signing  Generate upload keystore + key.properties for release signing
#   --dmg              Create DMG for macOS distribution (implies --macos --release --no-run)
#   --sign             Sign and notarize the DMG (implies --dmg)
#   --build-number N   Override the Flutter build number
#   --no-run           Build only, don't run on device
#   --codegen          Regenerate FRB bindings and sync FFI code (then exit)
#   --clean            Clean build artifacts before building
#
# IPA/TestFlight build numbers come from the latest TestFlight build for the current app version,
# plus one. AAB/Google Play build numbers (versionCode) come from the highest version code across
# all Play Store tracks, plus one. Explicit --build-number or RHYTHM_BUILD_NUMBER values override
# this. Other release builds use common CI run-number variables, then the +build value in
# flutter/rhythm_app/pubspec.yaml. The script does not generate timestamp build numbers.
#
# First-time setup (iOS):
#   1. Install CocoaPods: brew install cocoapods
#   2. Run: ./tools/app/scripts/build-mobile.sh --ios
#   3. Trust developer cert on iPhone: Settings → General → VPN & Device Management
#
# First-time setup (macOS signing):
#   Run: ./tools/app/scripts/build-mobile.sh --setup-signing
#   This stores notarization credentials in your keychain
#
# First-time setup (TestFlight):
#   1. Create API key in App Store Connect:
#      Users and Access → Integrations → Team Keys → Generate API Key
#   2. Download the .p8 file (only downloadable once)
#   3. Run: ./tools/app/scripts/build-mobile.sh --setup-testflight
#
# First-time setup (Android release signing):
#   Required before any AAB you upload to Play. Without it, Flutter signs
#   release builds with the debug key and Play rejects the upload.
#   Run: ./tools/app/scripts/build-mobile.sh --setup-android-signing
#   Generates android/app/upload-keystore.jks + android/key.properties.
#   BACK UP the keystore — losing it means losing the ability to update
#   the app (recoverable through Play Console support, but painful).
#
# First-time setup (Google Play):
#   1. Create a service account in Google Cloud Console for the Play project
#   2. In Play Console: Users and permissions → Invite the service account,
#      grant "Release manager" (or at least Releases + tracks access)
#   3. Download the JSON key for the service account
#   4. Run: ./tools/app/scripts/build-mobile.sh --setup-googleplay

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../app" && pwd)"
REPO_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"
RUST_FFI="$FLUTTER_APP/rust"
BUILD_RECEIPT_WRITER="$SCRIPT_DIR/write-app-build-receipt.sh"

# shellcheck source=lib/app-build-profile.sh
source "$SCRIPT_DIR/lib/app-build-profile.sh"

# Find flutter command (same as build-wasm.sh)
find_flutter() {
    if command -v flutter &> /dev/null; then
        FLUTTER_CMD="$(command -v flutter)"
        if [ -L "$FLUTTER_CMD" ]; then
            FLUTTER_CMD="$(readlink -f "$FLUTTER_CMD" 2>/dev/null || greadlink -f "$FLUTTER_CMD" 2>/dev/null || echo "$FLUTTER_CMD")"
        fi
    elif [ -x "$HOME/Documents/flutter/bin/flutter" ]; then
        FLUTTER_CMD="$HOME/Documents/flutter/bin/flutter"
    elif [ -x "/opt/flutter/bin/flutter" ]; then
        FLUTTER_CMD="/opt/flutter/bin/flutter"
    else
        echo "Error: flutter not found. Install Flutter or set PATH."
        exit 1
    fi
    echo "$FLUTTER_CMD"
}

pubspec_version() {
    sed -nE 's/^[[:space:]]*version:[[:space:]]*([^ #]+).*/\1/p' "$FLUTTER_APP/pubspec.yaml" | head -1
}

pubspec_build_name() {
    local version
    version="$(pubspec_version)"
    echo "${version%%+*}"
}

pubspec_build_number() {
    local version
    version="$(pubspec_version)"

    if [ "$version" != "${version##*+}" ]; then
        echo "${version##*+}"
    fi
}

write_store_build_receipt() {
    local store="$1"
    local channel="$2"
    local artifact="$3"
    local build_number="$4"
    local version
    local source_commit
    local receipt_path

    version="$(pubspec_build_name)"
    source_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"
    receipt_path="${RHYTHM_APP_BUILD_RECEIPT_PATH:-$REPO_ROOT/.release-evidence/app-builds/$version-$channel-$build_number.json}"
    "$BUILD_RECEIPT_WRITER" \
        --store "$store" \
        --channel "$channel" \
        --version "$version" \
        --build-number "$build_number" \
        --commit "$source_commit" \
        --artifact "$artifact" \
        --output "$receipt_path"
}

resolve_testflight_build_number() {
    local build_name
    local latest_build_number
    local fastlane_output
    local fastlane_status
    local attempt=1
    local max_attempts="${RHYTHM_TESTFLIGHT_BUILD_NUMBER_ATTEMPTS:-3}"
    local retry_delay="${RHYTHM_TESTFLIGHT_BUILD_NUMBER_RETRY_DELAY_SECONDS:-10}"

    build_name="$(pubspec_build_name)"
    if [ -z "$build_name" ]; then
        echo "Error: could not read app version from $FLUTTER_APP/pubspec.yaml."
        exit 1
    fi

    if ! command -v fastlane &> /dev/null; then
        echo "Error: fastlane is required to read the latest TestFlight build number."
        echo "Install with: brew install fastlane"
        exit 1
    fi

    if [ ! -f "$ASC_API_KEY_PATH" ]; then
        echo "Error: App Store Connect API key not configured."
        echo "Run: ./tools/app/scripts/build-mobile.sh --setup-testflight"
        exit 1
    fi

    echo "Checking latest TestFlight build for $IOS_APP_IDENTIFIER $build_name..."

    while true; do
        set +e
        fastlane_output=$(FASTLANE_DISABLE_COLORS=1 FASTLANE_SKIP_UPDATE_CHECK=1 fastlane run latest_testflight_build_number \
            api_key_path:"$ASC_API_KEY_PATH" \
            app_identifier:"$IOS_APP_IDENTIFIER" \
            version:"$build_name" \
            platform:"ios" \
            initial_build_number:0 2>&1)
        fastlane_status=$?
        set -e

        if [ "$fastlane_status" -eq 0 ]; then
            break
        fi

        echo "$fastlane_output"
        if [ "$attempt" -ge "$max_attempts" ] || \
            ! grep -Eiq 'server error got 5[0-9]{2}|service unavailable|temporar|timed? out|connection reset' \
                <<<"$fastlane_output"; then
            echo "Error: failed to read latest TestFlight build number."
            exit "$fastlane_status"
        fi

        echo "App Store Connect read failed transiently; retrying in ${retry_delay}s ($attempt/$max_attempts)..."
        sleep "$retry_delay"
        attempt=$((attempt + 1))
        retry_delay=$((retry_delay * 2))
    done

    latest_build_number="$(printf '%s\n' "$fastlane_output" | sed -nE 's/.*Result:[^0-9]*([0-9]+).*/\1/p' | tail -1)"
    if ! [[ "$latest_build_number" =~ ^[0-9]+$ ]]; then
        echo "$fastlane_output"
        echo "Error: could not parse latest TestFlight build number from fastlane output."
        exit 1
    fi

    BUILD_NUMBER_SOURCE="TestFlight latest build ($latest_build_number) + 1"
    RESOLVED_BUILD_NUMBER="$((latest_build_number + 1))"
}

resolve_googleplay_build_number() {
    if ! command -v fastlane &> /dev/null; then
        echo "Error: fastlane is required to read the latest Google Play version code."
        echo "Install with: brew install fastlane"
        exit 1
    fi

    if [ ! -f "$GOOGLE_PLAY_JSON_KEY_PATH" ]; then
        echo "Error: Google Play service account key not configured."
        echo "Run: ./tools/app/scripts/build-mobile.sh --setup-googleplay"
        exit 1
    fi

    echo "Checking latest Google Play version codes for $ANDROID_PACKAGE_NAME..."

    local highest=0
    local package_missing=false
    local any_visible=false
    local skipped_no_perm=()
    local empty_tracks=()
    local track
    for track in production beta alpha internal; do
        set +e
        local fastlane_output
        fastlane_output=$(FASTLANE_DISABLE_COLORS=1 FASTLANE_SKIP_UPDATE_CHECK=1 fastlane run google_play_track_version_codes \
            package_name:"$ANDROID_PACKAGE_NAME" \
            track:"$track" \
            json_key:"$GOOGLE_PLAY_JSON_KEY_PATH" 2>&1)
        local fastlane_status=$?
        set -e

        if [ $fastlane_status -ne 0 ]; then
            if printf '%s' "$fastlane_output" | grep -q "Package not found"; then
                package_missing=true
                continue
            fi
            if printf '%s' "$fastlane_output" | grep -qiE "does not have permission|forbidden|insufficient"; then
                skipped_no_perm+=("$track")
                continue
            fi
            # Fastlane bug (2.233.0+): tracks with zero releases crash on `nil.flat_map`.
            # See https://github.com/fastlane/fastlane/issues/21500. Treat as empty track.
            if printf '%s' "$fastlane_output" | grep -qE "undefined method.*flat_map.*nil"; then
                empty_tracks+=("$track")
                any_visible=true
                continue
            fi
            echo "$fastlane_output"
            echo "Error: failed to read Google Play version codes for track '$track'."
            exit $fastlane_status
        fi

        any_visible=true
        local codes
        codes=$(printf '%s\n' "$fastlane_output" | sed -nE 's/.*Result:[[:space:]]*\[([^]]*)\].*/\1/p' | tail -1)
        if [ -n "$codes" ]; then
            local max_in_track
            max_in_track=$(printf '%s\n' "$codes" | tr ',' '\n' | tr -d ' ' | grep -E '^[0-9]+$' | sort -n | tail -1)
            if [ -n "$max_in_track" ] && [ "$max_in_track" -gt "$highest" ]; then
                highest="$max_in_track"
            fi
        fi
    done

    if [ ${#skipped_no_perm[@]} -gt 0 ]; then
        echo "Note: service account lacks permission for tracks: ${skipped_no_perm[*]}"
        echo "      (using only the tracks it can see; if higher version codes exist in"
        echo "      hidden tracks, Play will reject the upload — grant broader access then)"
    fi

    if [ ${#empty_tracks[@]} -gt 0 ]; then
        echo "Note: tracks with no releases yet: ${empty_tracks[*]} (treated as empty)"
    fi

    if [ "$highest" -eq 0 ]; then
        if [ "$package_missing" = true ] || [ ${#empty_tracks[@]} -gt 0 ]; then
            BUILD_NUMBER_SOURCE="Google Play first upload (no existing releases, defaulting to 1)"
            RESOLVED_BUILD_NUMBER="1"
            return 0
        fi
        if [ "$any_visible" = false ]; then
            echo "Error: service account has no permission to read any Play track for $ANDROID_PACKAGE_NAME."
            echo "Grant it (at minimum) view access to the internal track in Play Console →"
            echo "Users and permissions → select the service account → App permissions."
            exit 1
        fi
    fi

    BUILD_NUMBER_SOURCE="Google Play latest version code ($highest) + 1"
    RESOLVED_BUILD_NUMBER="$((highest + 1))"
}

resolve_release_build_number() {
    if [ -n "$BUILD_NUMBER_OVERRIDE" ]; then
        BUILD_NUMBER_SOURCE="command line"
        RESOLVED_BUILD_NUMBER="$BUILD_NUMBER_OVERRIDE"
        return 0
    fi

    if [ -n "${RHYTHM_BUILD_NUMBER:-}" ]; then
        BUILD_NUMBER_SOURCE="RHYTHM_BUILD_NUMBER"
        RESOLVED_BUILD_NUMBER="$RHYTHM_BUILD_NUMBER"
        return 0
    fi

    if [ "$PLATFORM" = "ios" ] && [ "$BUILD_IPA" = true ]; then
        resolve_testflight_build_number
        return 0
    fi

    if [ "$PLATFORM" = "android" ] && [ "$BUILD_AAB" = true ]; then
        resolve_googleplay_build_number
        return 0
    fi

    if [ -n "${GITHUB_RUN_NUMBER:-}" ]; then
        BUILD_NUMBER_SOURCE="GITHUB_RUN_NUMBER"
        RESOLVED_BUILD_NUMBER="$GITHUB_RUN_NUMBER"
        return 0
    fi

    if [ -n "${BITRISE_BUILD_NUMBER:-}" ]; then
        BUILD_NUMBER_SOURCE="BITRISE_BUILD_NUMBER"
        RESOLVED_BUILD_NUMBER="$BITRISE_BUILD_NUMBER"
        return 0
    fi

    if [ -n "${CI_PIPELINE_IID:-}" ]; then
        BUILD_NUMBER_SOURCE="CI_PIPELINE_IID"
        RESOLVED_BUILD_NUMBER="$CI_PIPELINE_IID"
        return 0
    fi

    if [ -n "${BUILD_NUMBER:-}" ]; then
        BUILD_NUMBER_SOURCE="BUILD_NUMBER"
        RESOLVED_BUILD_NUMBER="$BUILD_NUMBER"
        return 0
    fi

    local build_number
    build_number="$(pubspec_build_number)"
    if [ -n "$build_number" ]; then
        BUILD_NUMBER_SOURCE="pubspec.yaml"
        RESOLVED_BUILD_NUMBER="$build_number"
        return 0
    fi

    BUILD_NUMBER_SOURCE=""
    RESOLVED_BUILD_NUMBER=""
}

# macOS signing configuration
APPLE_ID="${APPLE_ID:-}"
TEAM_ID="${TEAM_ID:-}"
SIGNING_IDENTITY="${SIGNING_IDENTITY:-}"
KEYCHAIN_PROFILE="${KEYCHAIN_PROFILE:-}"
APP_NAME="Rhythm Lighting"

# TestFlight configuration
ASC_API_KEY_PATH="$HOME/.config/rhythm/asc_api_key.json"
IOS_APP_IDENTIFIER="lighting.rhythm.app"

# Google Play configuration
GOOGLE_PLAY_JSON_KEY_PATH="$HOME/.config/rhythm/google_play_key.json"
ANDROID_PACKAGE_NAME="lighting.rhythm.app"
GOOGLE_PLAY_TRACK="internal"

# Defaults
PLATFORM=""
RELEASE=""
BUILD_IPA=false
BUILD_AAB=false
BUILD_DMG=false
SIGN_APP=false
SETUP_SIGNING=false
UPLOAD_TESTFLIGHT=false
SETUP_TESTFLIGHT=false
UPLOAD_GOOGLEPLAY=false
SETUP_GOOGLEPLAY=false
RELEASE_ALL=false
SETUP_ANDROID_SIGNING=false
RUN_APP=true
RUN_CODEGEN=false
CLEAN=false
BUILD_NUMBER_OVERRIDE=""
BUILD_NUMBER_SOURCE=""
RESOLVED_BUILD_NUMBER=""
BUILD_METADATA_ARGS=""
TESTFLIGHT_NOTES_FILE="${RHYTHM_TESTFLIGHT_NOTES_FILE:-}"
TESTFLIGHT_CHANGELOG=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --ios)
            PLATFORM="ios"
            shift
            ;;
        --android)
            PLATFORM="android"
            shift
            ;;
        --macos)
            PLATFORM="macos"
            shift
            ;;
        --release)
            RELEASE="--release"
            shift
            ;;
        --ipa)
            BUILD_IPA=true
            RELEASE="--release"
            RUN_APP=false
            PLATFORM="ios"
            shift
            ;;
        --testflight)
            BUILD_IPA=true
            UPLOAD_TESTFLIGHT=true
            RELEASE="--release"
            RUN_APP=false
            PLATFORM="ios"
            CLEAN=true
            shift
            ;;
        --testflight-notes-file)
            if [ -z "${2:-}" ] || [[ "${2:-}" == --* ]]; then
                echo "Error: --testflight-notes-file requires a path."
                exit 1
            fi
            TESTFLIGHT_NOTES_FILE="$2"
            shift 2
            ;;
        --setup-testflight)
            SETUP_TESTFLIGHT=true
            shift
            ;;
        --aab)
            BUILD_AAB=true
            RELEASE="--release"
            RUN_APP=false
            PLATFORM="android"
            shift
            ;;
        --googleplay)
            BUILD_AAB=true
            UPLOAD_GOOGLEPLAY=true
            RELEASE="--release"
            RUN_APP=false
            PLATFORM="android"
            CLEAN=true
            shift
            ;;
        --setup-googleplay)
            SETUP_GOOGLEPLAY=true
            shift
            ;;
        --release-all)
            RELEASE_ALL=true
            shift
            ;;
        --setup-android-signing)
            SETUP_ANDROID_SIGNING=true
            shift
            ;;
        --dmg)
            BUILD_DMG=true
            RELEASE="--release"
            RUN_APP=false
            PLATFORM="macos"
            shift
            ;;
        --sign)
            SIGN_APP=true
            BUILD_DMG=true
            RELEASE="--release"
            RUN_APP=false
            PLATFORM="macos"
            shift
            ;;
        --build-number)
            if [ -z "${2:-}" ] || [[ "${2:-}" == --* ]]; then
                echo "Error: --build-number requires a numeric value."
                exit 1
            fi
            BUILD_NUMBER_OVERRIDE="$2"
            shift 2
            ;;
        --setup-signing)
            SETUP_SIGNING=true
            shift
            ;;
        --no-run)
            RUN_APP=false
            shift
            ;;
        --codegen)
            RUN_CODEGEN=true
            shift
            ;;
        --clean)
            CLEAN=true
            shift
            ;;
        -h|--help)
            sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# --release-all: ship to TestFlight and Google Play in one go.
# Re-exec this script for each store so each pipeline gets its own clean,
# pub-get, codesigning, and platform-specific build-number resolution.
# Build numbers are resolved per-platform (TestFlight latest+1, Play latest+1)
# unless an explicit override was passed.
if [ "$RELEASE_ALL" = true ]; then
    echo "Release all: TestFlight + Google Play"
    echo ""

    EXTRA_ARGS=()
    if [ -n "$BUILD_NUMBER_OVERRIDE" ]; then
        EXTRA_ARGS+=(--build-number "$BUILD_NUMBER_OVERRIDE")
    fi
    if [ -n "$TESTFLIGHT_NOTES_FILE" ]; then
        EXTRA_ARGS+=(--testflight-notes-file "$TESTFLIGHT_NOTES_FILE")
    fi

    echo "==> 1/2  TestFlight"
    "$0" "${EXTRA_ARGS[@]}" --testflight

    echo ""
    echo "==> 2/2  Google Play"
    "$0" "${EXTRA_ARGS[@]}" --googleplay

    echo ""
    echo "Release all complete: shipped to TestFlight + Google Play."
    exit 0
fi

if [ "$UPLOAD_TESTFLIGHT" = true ] && [ -n "$TESTFLIGHT_NOTES_FILE" ]; then
    if [ ! -f "$TESTFLIGHT_NOTES_FILE" ]; then
        echo "Error: TestFlight notes file not found: $TESTFLIGHT_NOTES_FILE"
        exit 1
    fi

    TESTFLIGHT_CHANGELOG="$(< "$TESTFLIGHT_NOTES_FILE")"
    if ! grep -q '[^[:space:]]' <<<"$TESTFLIGHT_CHANGELOG"; then
        echo "Error: TestFlight notes file is empty: $TESTFLIGHT_NOTES_FILE"
        exit 1
    fi

    TESTFLIGHT_NOTES_LENGTH="$(
        awk '{ total += length($0) + (NR > 1 ? 1 : 0) } END { print total + 0 }' \
            "$TESTFLIGHT_NOTES_FILE"
    )"
    if [ "$TESTFLIGHT_NOTES_LENGTH" -gt 4000 ]; then
        echo "Error: TestFlight notes exceed Apple's 4000-character limit ($TESTFLIGHT_NOTES_LENGTH)."
        exit 1
    fi
    echo "Using TestFlight 'What to Test' notes: $TESTFLIGHT_NOTES_FILE ($TESTFLIGHT_NOTES_LENGTH characters)"
    echo ""
fi

# Auto-detect platform
if [ -z "$PLATFORM" ]; then
    if [[ "$OSTYPE" == "darwin"* ]]; then
        PLATFORM="ios"
    else
        PLATFORM="android"
    fi
fi

echo "Building for: $PLATFORM"
echo ""

if [ -n "$BUILD_NUMBER_OVERRIDE" ] || [ -n "$RELEASE" ]; then
    resolve_release_build_number
fi

if [ -n "$RESOLVED_BUILD_NUMBER" ]; then
    if ! [[ "$RESOLVED_BUILD_NUMBER" =~ ^[0-9]+$ ]]; then
        echo "Error: build number must be numeric, got '$RESOLVED_BUILD_NUMBER'."
        exit 1
    fi
    BUILD_METADATA_ARGS="--build-number=$RESOLVED_BUILD_NUMBER"
    echo "Using build number: $RESOLVED_BUILD_NUMBER ($BUILD_NUMBER_SOURCE)"
    echo ""
fi

# Setup signing credentials in keychain
if [ "$SETUP_SIGNING" = true ]; then
    echo "Setting up notarization credentials..."
    echo "This will store your credentials securely in the macOS keychain."
    echo ""
    echo "Apple ID: $APPLE_ID"
    echo "Team ID: $TEAM_ID"
    echo ""
    xcrun notarytool store-credentials "$KEYCHAIN_PROFILE" \
        --apple-id "$APPLE_ID" \
        --team-id "$TEAM_ID"
    echo ""
    echo "Credentials stored in keychain profile: $KEYCHAIN_PROFILE"
    echo "You can now use --sign to create signed DMGs."
    exit 0
fi

# Setup TestFlight API key
if [ "$SETUP_TESTFLIGHT" = true ]; then
    echo "Setting up App Store Connect API key for TestFlight uploads..."
    echo ""
    echo "You'll need:"
    echo "  - API Key ID (from App Store Connect)"
    echo "  - Issuer ID (from App Store Connect)"
    echo "  - Path to downloaded .p8 key file"
    echo ""

    read -p "API Key ID: " API_KEY_ID
    read -p "Issuer ID: " ISSUER_ID
    read -p "Path to .p8 key file: " P8_PATH

    # Expand tilde and validate file exists
    P8_PATH="${P8_PATH/#\~/$HOME}"
    if [ ! -f "$P8_PATH" ]; then
        echo "Error: File not found: $P8_PATH"
        exit 1
    fi

    # Read the key content
    KEY_CONTENT=$(cat "$P8_PATH")

    # Create config directory
    mkdir -p "$(dirname "$ASC_API_KEY_PATH")"

    # Write JSON config (Fastlane format)
    cat > "$ASC_API_KEY_PATH" <<EOF
{
  "key_id": "$API_KEY_ID",
  "issuer_id": "$ISSUER_ID",
  "key": $(echo "$KEY_CONTENT" | jq -Rs .),
  "in_house": false
}
EOF

    # Set restrictive permissions
    chmod 600 "$ASC_API_KEY_PATH"

    echo ""
    echo "API key configuration saved to: $ASC_API_KEY_PATH"
    echo "You can now use --testflight to build and upload to TestFlight."
    exit 0
fi

# Setup Google Play service account key
if [ "$SETUP_GOOGLEPLAY" = true ]; then
    echo "Setting up Google Play service account key for Play Store uploads..."
    echo ""
    echo "You'll need:"
    echo "  - Path to the service account JSON key file"
    echo "    (created in Google Cloud Console, granted access in Play Console)"
    echo ""

    read -p "Path to JSON key file: " JSON_PATH

    JSON_PATH="${JSON_PATH/#\~/$HOME}"
    if [ ! -f "$JSON_PATH" ]; then
        echo "Error: File not found: $JSON_PATH"
        exit 1
    fi

    mkdir -p "$(dirname "$GOOGLE_PLAY_JSON_KEY_PATH")"
    cp "$JSON_PATH" "$GOOGLE_PLAY_JSON_KEY_PATH"
    chmod 600 "$GOOGLE_PLAY_JSON_KEY_PATH"

    echo ""
    echo "Service account key saved to: $GOOGLE_PLAY_JSON_KEY_PATH"
    echo "You can now use --googleplay to build and upload to Google Play."
    exit 0
fi

# Setup Android upload keystore for release signing
if [ "$SETUP_ANDROID_SIGNING" = true ]; then
    KEYSTORE_PATH="$FLUTTER_APP/android/app/upload-keystore.jks"
    KEY_PROPERTIES_PATH="$FLUTTER_APP/android/key.properties"

    if [ -f "$KEYSTORE_PATH" ] || [ -f "$KEY_PROPERTIES_PATH" ]; then
        echo "Keystore or key.properties already exists:"
        [ -f "$KEYSTORE_PATH" ] && echo "  $KEYSTORE_PATH"
        [ -f "$KEY_PROPERTIES_PATH" ] && echo "  $KEY_PROPERTIES_PATH"
        echo ""
        echo "Refusing to overwrite. Move them aside first if you really want a new keystore."
        echo "WARNING: replacing the keystore breaks future Play uploads unless you go through"
        echo "Play Console support to reset the upload key."
        exit 1
    fi

    KEYTOOL=""
    for candidate in \
        "/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin/keytool" \
        "/Applications/Android Studio Preview.app/Contents/jbr/Contents/Home/bin/keytool"; do
        if [ -x "$candidate" ]; then
            KEYTOOL="$candidate"
            break
        fi
    done
    if [ -z "$KEYTOOL" ] && [ -x /usr/libexec/java_home ]; then
        JH="$(/usr/libexec/java_home 2>/dev/null)"
        if [ -n "$JH" ] && [ -x "$JH/bin/keytool" ]; then
            KEYTOOL="$JH/bin/keytool"
        fi
    fi
    if [ -z "$KEYTOOL" ] && command -v keytool &> /dev/null; then
        if keytool -help &> /dev/null; then
            KEYTOOL="$(command -v keytool)"
        fi
    fi
    if [ -z "$KEYTOOL" ]; then
        echo "Error: no working keytool found."
        echo ""
        echo "macOS ships a /usr/bin/keytool stub but no actual JDK. Install one:"
        echo "  brew install --cask temurin       # Eclipse Temurin JDK"
        echo "  brew install openjdk              # or plain OpenJDK"
        echo "Or install Android Studio (bundles a JDK at"
        echo "/Applications/Android Studio.app/Contents/jbr)."
        exit 1
    fi

    echo "Using keytool: $KEYTOOL"
    echo ""
    echo "Generating Android upload keystore..."
    echo ""
    echo "You'll be prompted for:"
    echo "  - A keystore password (remember this — you'll need it for every release)"
    echo "  - A key password (can be the same as the keystore password)"
    echo "  - Distinguished name fields (name, org, locale — values don't matter much)"
    echo ""
    read -p "Press Enter to continue..."

    "$KEYTOOL" -genkey -v \
        -keystore "$KEYSTORE_PATH" \
        -keyalg RSA -keysize 2048 -validity 10000 \
        -alias upload

    if [ ! -f "$KEYSTORE_PATH" ]; then
        echo "Error: keystore was not created."
        exit 1
    fi

    echo ""
    echo "Validating keystore password (so we don't write a bad key.properties)..."
    STORE_PW=""
    for attempt in 1 2 3; do
        read -s -p "Re-enter keystore password: " STORE_PW
        echo
        if "$KEYTOOL" -list -keystore "$KEYSTORE_PATH" -storepass "$STORE_PW" -alias upload &> /dev/null; then
            break
        fi
        echo "Wrong password. Try again ($((3 - attempt)) attempt(s) left)."
        STORE_PW=""
    done
    if [ -z "$STORE_PW" ]; then
        echo "Error: keystore password validation failed 3 times."
        echo "The keystore was created at $KEYSTORE_PATH but key.properties was not written."
        echo "Re-run --setup-android-signing after deleting that keystore, OR write"
        echo "$KEY_PROPERTIES_PATH by hand if you remember the password."
        exit 1
    fi

    KEY_PW=""
    for attempt in 1 2 3; do
        read -s -p "Re-enter key password (blank = same as keystore): " KEY_PW
        echo
        [ -z "$KEY_PW" ] && KEY_PW="$STORE_PW"
        if "$KEYTOOL" -certreq -keystore "$KEYSTORE_PATH" -storepass "$STORE_PW" -keypass "$KEY_PW" -alias upload &> /dev/null; then
            break
        fi
        echo "Wrong key password. Try again ($((3 - attempt)) attempt(s) left)."
        KEY_PW=""
    done
    if [ -z "$KEY_PW" ]; then
        echo "Error: key password validation failed 3 times."
        exit 1
    fi

    cat > "$KEY_PROPERTIES_PATH" <<EOF
storePassword=$STORE_PW
keyPassword=$KEY_PW
keyAlias=upload
storeFile=app/upload-keystore.jks
EOF
    chmod 600 "$KEY_PROPERTIES_PATH"

    echo ""
    echo "Keystore:       $KEYSTORE_PATH"
    echo "key.properties: $KEY_PROPERTIES_PATH"
    echo ""
    echo "Both are gitignored. BACK UP the keystore now — store it somewhere safe"
    echo "(1Password, encrypted backup, etc.). Losing it means future updates require"
    echo "going through Play Console upload-key reset."
    echo ""
    echo "You can now use --googleplay to build and upload signed releases."
    exit 0
fi

# Run FRB codegen if requested
if [ "$RUN_CODEGEN" = true ]; then
    FLUTTER_CMD=$(find_flutter)
    FLUTTER_BIN_DIR="$(dirname "$FLUTTER_CMD")"

    echo "Regenerating flutter_rust_bridge bindings..."

    # Check for flutter_rust_bridge_codegen
    if ! command -v flutter_rust_bridge_codegen &> /dev/null; then
        echo "Installing flutter_rust_bridge_codegen..."
        cargo install flutter_rust_bridge_codegen
    fi

    (cd "$FLUTTER_APP" && PATH="$FLUTTER_BIN_DIR:$PATH" flutter_rust_bridge_codegen generate)
    echo ""
    echo "Codegen complete."
    exit 0
fi

# Clean if requested
if [ "$CLEAN" = true ]; then
    echo "Cleaning build artifacts..."
    cd "$FLUTTER_APP"
    flutter clean

    if [ "$PLATFORM" = "ios" ]; then
        rm -rf ios/Pods ios/Podfile.lock ios/.symlinks
        echo "  -> Cleaned iOS pods"
    elif [ "$PLATFORM" = "macos" ]; then
        rm -rf macos/Pods macos/Podfile.lock macos/.symlinks
        echo "  -> Cleaned macOS pods"
    fi
    echo ""
fi

# Get Flutter dependencies
echo "Getting Flutter dependencies..."
cd "$FLUTTER_APP"
flutter pub get

# CocoaPods setup for iOS/macOS
if [ "$PLATFORM" = "ios" ]; then
    echo ""
    echo "Running pod install for iOS..."
    cd "$FLUTTER_APP/ios"
    pod install
    cd "$FLUTTER_APP"
elif [ "$PLATFORM" = "macos" ]; then
    echo ""
    echo "Running pod install for macOS..."
    cd "$FLUTTER_APP/macos"
    pod install
    cd "$FLUTTER_APP"
fi

# Build Flutter arguments as an array so paths containing spaces (including
# isolated TestFlight dispatch worktrees) remain one argument.
FLUTTER_BUILD_ARGS=()
if [ -n "$RELEASE" ]; then
    FLUTTER_BUILD_ARGS+=("$RELEASE")
fi
prepare_app_build_define_file "$REPO_ROOT" "$FLUTTER_APP"
trap cleanup_app_build_define_file EXIT
FLUTTER_BUILD_ARGS+=("--dart-define-from-file=$RHYTHM_APP_BUILD_DEFINE_FILE")
echo "Using the configured app-build profile"
if [ -n "$BUILD_METADATA_ARGS" ]; then
    FLUTTER_BUILD_ARGS+=("$BUILD_METADATA_ARGS")
fi

# Build/Run
echo ""
if [ "$RUN_APP" = true ]; then
    echo "Building and running on $PLATFORM..."
    if [ "$PLATFORM" = "macos" ]; then
        flutter run -d macos "${FLUTTER_BUILD_ARGS[@]}"
    else
        flutter run "${FLUTTER_BUILD_ARGS[@]}"
    fi
else
    echo "Building for $PLATFORM..."
    if [ "$BUILD_IPA" = true ]; then
        echo "Building IPA for TestFlight..."
        # Never accept an IPA or archive left by an earlier invocation when
        # deciding whether this build or its authenticated fallback succeeded.
        rm -rf "$FLUTTER_APP/build/ios/ipa" \
            "$FLUTTER_APP/build/ios/archive/Runner.xcarchive"
        set +e
        flutter build ipa "${FLUTTER_BUILD_ARGS[@]}"
        FLUTTER_IPA_STATUS=$?
        set -e

        IPA_FILE=""
        if [ -d "$FLUTTER_APP/build/ios/ipa" ]; then
            IPA_FILE=$(find "$FLUTTER_APP/build/ios/ipa" -name "*.ipa" -type f | head -1)
        fi

        # Preserve the normal interactive Xcode path. If Flutter produced an
        # archive but export could not see an Xcode account or distribution
        # certificate, retry only the export with the same App Store Connect
        # API key that TestFlight upload already requires.
        if [ -z "$IPA_FILE" ] && [ -d "$FLUTTER_APP/build/ios/archive/Runner.xcarchive" ]; then
            echo ""
            echo "Flutter produced an archive but no IPA; retrying export with App Store Connect authentication..."
            "$SCRIPT_DIR/export-testflight-ipa.sh" \
                --archive "$FLUTTER_APP/build/ios/archive/Runner.xcarchive" \
                --export-path "$FLUTTER_APP/build/ios/ipa" \
                --api-key "$ASC_API_KEY_PATH" \
                --team-id "$TEAM_ID" \
                --bundle-id "$IOS_APP_IDENTIFIER"
            IPA_FILE=$(find "$FLUTTER_APP/build/ios/ipa" -name "*.ipa" -type f | head -1)
        elif [ "$FLUTTER_IPA_STATUS" -ne 0 ]; then
            exit "$FLUTTER_IPA_STATUS"
        fi

        if [ -z "$IPA_FILE" ]; then
            echo "Error: No IPA file found in build/ios/ipa/"
            exit 1
        fi
        echo ""
        echo "IPA created at: build/ios/ipa/"

        # Upload to TestFlight if requested
        if [ "$UPLOAD_TESTFLIGHT" = true ]; then
            echo ""
            echo "Uploading to TestFlight..."

            # Check for Fastlane
            if ! command -v fastlane &> /dev/null; then
                echo ""
                echo "Fastlane not found. Install with:"
                echo "  brew install fastlane"
                echo ""
                read -p "Install Fastlane now? [y/N] " -n 1 -r
                echo
                if [[ $REPLY =~ ^[Yy]$ ]]; then
                    brew install fastlane
                else
                    echo "Skipping TestFlight upload. Install Fastlane and run again."
                    exit 1
                fi
            fi

            # Check for API key config
            if [ ! -f "$ASC_API_KEY_PATH" ]; then
                echo "Error: App Store Connect API key not configured."
                echo "Run: ./tools/app/scripts/build-mobile.sh --setup-testflight"
                exit 1
            fi

            echo "Uploading: $IPA_FILE"
            TESTFLIGHT_UPLOAD_ARGS=(
                --api_key_path "$ASC_API_KEY_PATH"
                --ipa "$IPA_FILE"
                --skip_waiting_for_build_processing
            )
            if [ -n "$TESTFLIGHT_CHANGELOG" ]; then
                TESTFLIGHT_UPLOAD_ARGS+=(--changelog "$TESTFLIGHT_CHANGELOG")
            fi
            fastlane pilot upload "${TESTFLIGHT_UPLOAD_ARGS[@]}"
            write_store_build_receipt \
                app-store-connect testflight "$IPA_FILE" "$RESOLVED_BUILD_NUMBER"

            echo ""
            echo "Upload complete! Build will appear in TestFlight after processing."
        else
            echo "Upload to TestFlight using:"
            echo "  - Xcode: Open Organizer (Cmd+Shift+O) → Archives → Distribute"
            echo "  - CLI: ./tools/app/scripts/build-mobile.sh --testflight"
            echo "  - Transporter app from Mac App Store"
        fi
    elif [ "$PLATFORM" = "ios" ]; then
        flutter build ios "${FLUTTER_BUILD_ARGS[@]}"
    elif [ "$PLATFORM" = "macos" ]; then
        flutter build macos "${FLUTTER_BUILD_ARGS[@]}"

        # Create DMG if requested
        if [ "$BUILD_DMG" = true ]; then
            echo ""
            echo "Creating DMG..."

            APP_PATH="$FLUTTER_APP/build/macos/Build/Products/Release/$APP_NAME.app"
            DMG_PATH="$FLUTTER_APP/build/$APP_NAME.dmg"
            DMG_STAGING="$FLUTTER_APP/build/dmg_staging"

            if [ ! -d "$APP_PATH" ]; then
                echo "Error: App not found at $APP_PATH"
                exit 1
            fi

            # Sign the app if requested
            if [ "$SIGN_APP" = true ]; then
                echo "Signing app with: $SIGNING_IDENTITY"

                ENTITLEMENTS="$FLUTTER_APP/macos/Runner/Release.entitlements"

                # First, strip ALL existing signatures
                echo "  -> Stripping existing signatures..."
                find "$APP_PATH" -type f \( -name "*.dylib" -o -perm +111 \) | while read -r binary; do
                    codesign --remove-signature "$binary" 2>/dev/null || true
                done
                find "$APP_PATH/Contents/Frameworks" -type d -name "*.framework" | while read -r framework; do
                    codesign --remove-signature "$framework" 2>/dev/null || true
                done
                codesign --remove-signature "$APP_PATH" 2>/dev/null || true

                # Sign all nested components from inside-out
                # 1. Sign all dylibs (with entitlements for JIT support)
                echo "  -> Signing dylibs..."
                find "$APP_PATH/Contents" -type f -name "*.dylib" | while read -r dylib; do
                    codesign --force --sign "$SIGNING_IDENTITY" --options runtime --timestamp --entitlements "$ENTITLEMENTS" "$dylib"
                done

                # 2. Sign all framework binaries first, then the framework bundles
                echo "  -> Signing frameworks..."
                find "$APP_PATH/Contents/Frameworks" -type d -name "*.framework" | sort | while read -r framework; do
                    framework_name=$(basename "$framework" .framework)
                    framework_binary="$framework/Versions/A/$framework_name"
                    if [ ! -f "$framework_binary" ]; then
                        framework_binary="$framework/$framework_name"
                    fi
                    if [ -f "$framework_binary" ]; then
                        codesign --force --sign "$SIGNING_IDENTITY" --options runtime --timestamp --entitlements "$ENTITLEMENTS" "$framework_binary"
                    fi
                    codesign --force --sign "$SIGNING_IDENTITY" --options runtime --timestamp "$framework"
                done

                # 3. Sign the main executable with entitlements
                echo "  -> Signing main executable..."
                codesign --force --sign "$SIGNING_IDENTITY" --options runtime --timestamp --entitlements "$ENTITLEMENTS" "$APP_PATH/Contents/MacOS/$APP_NAME"

                # 4. Sign the entire app bundle with entitlements
                echo "  -> Signing app bundle..."
                codesign --force --sign "$SIGNING_IDENTITY" --options runtime --timestamp --entitlements "$ENTITLEMENTS" "$APP_PATH"

                # Verify
                echo "  -> Verifying signature..."
                if codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>&1; then
                    echo "  -> App signed successfully"
                else
                    echo "  -> Warning: Signature verification had issues, continuing anyway..."
                fi
            fi

            # Create staging directory with Applications symlink
            # IMPORTANT: Use ditto instead of cp to preserve code signatures
            rm -rf "$DMG_STAGING"
            mkdir -p "$DMG_STAGING"
            ditto "$APP_PATH" "$DMG_STAGING/$(basename "$APP_PATH")"
            ln -s /Applications "$DMG_STAGING/Applications"

            # Create DMG
            rm -f "$DMG_PATH"
            hdiutil create -volname "$APP_NAME" \
                -srcfolder "$DMG_STAGING" \
                -ov -format UDZO \
                "$DMG_PATH"

            # Clean up staging
            rm -rf "$DMG_STAGING"

            echo "  -> DMG created: $DMG_PATH"

            # Sign and notarize DMG if requested
            if [ "$SIGN_APP" = true ]; then
                echo ""
                echo "Signing DMG..."
                codesign --force --sign "$SIGNING_IDENTITY" "$DMG_PATH"
                echo "  -> DMG signed"

                echo ""
                echo "Submitting for notarization (this may take a few minutes)..."
                xcrun notarytool submit "$DMG_PATH" \
                    --keychain-profile "$KEYCHAIN_PROFILE" \
                    --wait

                echo ""
                echo "Stapling notarization ticket..."
                xcrun stapler staple "$DMG_PATH"
                echo "  -> Notarization complete"
            fi

            echo ""
            echo "DMG ready: $DMG_PATH"
            if [ "$SIGN_APP" = true ]; then
                echo "The DMG is signed and notarized - it will open without warnings."
            else
                echo "Note: DMG is unsigned. Recipients will need to right-click → Open."
            fi
        fi
    elif [ "$PLATFORM" = "android" ]; then
        if [ "$BUILD_AAB" = true ]; then
            if [ ! -f "$FLUTTER_APP/android/key.properties" ]; then
                echo "Error: android/key.properties not found — release builds would be"
                echo "signed with the debug key and rejected by Play."
                echo "Run: ./tools/app/scripts/build-mobile.sh --setup-android-signing"
                exit 1
            fi
            echo "Building AAB for Google Play..."
            flutter build appbundle "${FLUTTER_BUILD_ARGS[@]}"
            echo ""
            echo "AAB created at: build/app/outputs/bundle/release/"

            if [ "$UPLOAD_GOOGLEPLAY" = true ]; then
                echo ""
                echo "Uploading to Google Play ($GOOGLE_PLAY_TRACK track)..."

                if ! command -v fastlane &> /dev/null; then
                    echo ""
                    echo "Fastlane not found. Install with:"
                    echo "  brew install fastlane"
                    echo ""
                    read -p "Install Fastlane now? [y/N] " -n 1 -r
                    echo
                    if [[ $REPLY =~ ^[Yy]$ ]]; then
                        brew install fastlane
                    else
                        echo "Skipping Google Play upload. Install Fastlane and run again."
                        exit 1
                    fi
                fi

                if [ ! -f "$GOOGLE_PLAY_JSON_KEY_PATH" ]; then
                    echo "Error: Google Play service account key not configured."
                    echo "Run: ./tools/app/scripts/build-mobile.sh --setup-googleplay"
                    exit 1
                fi

                AAB_FILE=$(find "$FLUTTER_APP/build/app/outputs/bundle/release" -name "*.aab" -type f | head -1)
                if [ -z "$AAB_FILE" ]; then
                    echo "Error: No AAB file found in build/app/outputs/bundle/release/"
                    exit 1
                fi

                run_supply() {
                    local release_status="$1"
                    set +e
                    FASTLANE_DISABLE_COLORS=1 FASTLANE_SKIP_UPDATE_CHECK=1 fastlane supply \
                        --package_name "$ANDROID_PACKAGE_NAME" \
                        --json_key "$GOOGLE_PLAY_JSON_KEY_PATH" \
                        --aab "$AAB_FILE" \
                        --track "$GOOGLE_PLAY_TRACK" \
                        --release_status "$release_status" \
                        --skip_upload_metadata \
                        --skip_upload_changelogs \
                        --skip_upload_images \
                        --skip_upload_screenshots 2>&1 | tee /tmp/rhythm_supply_output.log
                    supply_status=${PIPESTATUS[0]}
                    set -e
                }

                echo "Uploading: $AAB_FILE"
                run_supply completed

                if [ $supply_status -ne 0 ] && grep -q "Only releases with status draft may be created on draft app" /tmp/rhythm_supply_output.log; then
                    echo ""
                    echo "App is still in draft state in Play Console — retrying with release_status=draft..."
                    echo ""
                    run_supply draft
                fi

                if [ $supply_status -ne 0 ]; then
                    if grep -q "Package not found" /tmp/rhythm_supply_output.log; then
                        echo ""
                        echo "Google Play has no record of '$ANDROID_PACKAGE_NAME' yet."
                        echo ""
                        echo "Play Console requires the FIRST upload to be made manually through"
                        echo "the web UI. The API only works for subsequent releases."
                        echo ""
                        echo "Next steps:"
                        echo "  1. Go to Play Console (https://play.google.com/console) and create"
                        echo "     the app entry for $ANDROID_PACKAGE_NAME."
                        echo "  2. Testing → Internal testing → Create new release."
                        echo "  3. Upload this AAB by hand:"
                        echo "       $AAB_FILE"
                        echo "  4. Save & roll out."
                        echo "  5. Future releases: ./tools/app/scripts/build-mobile.sh --googleplay"
                    fi
                    rm -f /tmp/rhythm_supply_output.log
                    exit $supply_status
                fi

                if grep -q "release_status=draft" /tmp/rhythm_supply_output.log 2>/dev/null || \
                   grep -q "Only releases with status draft" /tmp/rhythm_supply_output.log 2>/dev/null; then
                    echo ""
                    echo "Uploaded as a DRAFT release because the app is still in draft state."
                    echo "Finish app setup in Play Console (data safety, content rating, target"
                    echo "audience, store listing, countries), then go to Internal testing and"
                    echo "click Review release → Start rollout."
                fi
                rm -f /tmp/rhythm_supply_output.log
                write_store_build_receipt \
                    google-play "$GOOGLE_PLAY_TRACK" "$AAB_FILE" "$RESOLVED_BUILD_NUMBER"

                echo ""
                echo "Upload complete! Build will appear in Play Console after processing."
            else
                echo "Upload to Google Play using:"
                echo "  - CLI: ./tools/app/scripts/build-mobile.sh --googleplay"
                echo "  - Web: Play Console → Testing → Internal testing → Create new release"
            fi
        else
            flutter build apk "${FLUTTER_BUILD_ARGS[@]}"
        fi
    fi
    echo ""
    echo "Build complete."
fi

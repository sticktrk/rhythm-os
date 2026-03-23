#!/bin/bash
# Build and run Flutter app on mobile/desktop (iOS/Android/macOS)
#
# Usage: ./scripts/build-mobile.sh [options]
# Options:
#   --ios              Build for iOS (default if on macOS)
#   --android          Build for Android
#   --macos            Build for macOS desktop
#   --release          Build in release mode
#   --ipa              Build IPA for TestFlight (implies --release --no-run)
#   --testflight       Build IPA and upload to TestFlight (implies --clean --release --no-run)
#   --dmg              Create DMG for macOS distribution (implies --macos --release --no-run)
#   --sign             Sign and notarize the DMG (implies --dmg)
#   --no-run           Build only, don't run on device
#   --codegen          Regenerate FRB bindings and sync FFI code (then exit)
#   --clean            Clean build artifacts before building
#
# First-time setup (iOS):
#   1. Install CocoaPods: brew install cocoapods
#   2. Run: ./scripts/build-mobile.sh --ios
#   3. Trust developer cert on iPhone: Settings → General → VPN & Device Management
#
# First-time setup (macOS signing):
#   Run: ./scripts/build-mobile.sh --setup-signing
#   This stores notarization credentials in your keychain
#
# First-time setup (TestFlight):
#   1. Create API key in App Store Connect:
#      Users and Access → Integrations → Team Keys → Generate API Key
#   2. Download the .p8 file (only downloadable once)
#   3. Run: ./scripts/build-mobile.sh --setup-testflight

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"
# Rust FFI source (may be in a submodule)
if [ -d "$PROJECT_ROOT/rhythm-lighting" ]; then
    RUST_FFI="$PROJECT_ROOT/rhythm-lighting/rust/core/rhythm-core-ffi"
else
    RUST_FFI="$PROJECT_ROOT/rust/core/rhythm-core-ffi"
fi
WRAPPER_RUST="$FLUTTER_APP/rust"

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

# macOS signing configuration
APPLE_ID="${APPLE_ID:-}"
TEAM_ID="${TEAM_ID:-}"
SIGNING_IDENTITY="${SIGNING_IDENTITY:-}"
KEYCHAIN_PROFILE="${KEYCHAIN_PROFILE:-}"
APP_NAME="RhythmLighting"

# TestFlight configuration
ASC_API_KEY_PATH="$HOME/.config/rhythm/asc_api_key.json"

# Defaults
PLATFORM=""
RELEASE=""
BUILD_IPA=false
BUILD_DMG=false
SIGN_APP=false
SETUP_SIGNING=false
UPLOAD_TESTFLIGHT=false
SETUP_TESTFLIGHT=false
RUN_APP=true
RUN_CODEGEN=false
CLEAN=false

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
        --setup-testflight)
            SETUP_TESTFLIGHT=true
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
            head -31 "$0" | tail -29
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

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

    # Sync FFI code from rhythm-core-ffi to wrapper crate
    echo "Syncing FFI code to wrapper crate..."
    rm -rf "$WRAPPER_RUST/src/api"
    cp -r "$RUST_FFI/src/api" "$WRAPPER_RUST/src/"
    cp "$RUST_FFI/src/frb_generated.rs" "$WRAPPER_RUST/src/"
    echo "  -> Copied api/ directory and frb_generated.rs"
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

# Check for .env file with Supabase credentials
DART_DEFINES=""
if [ -f "$FLUTTER_APP/.env" ]; then
    DART_DEFINES="--dart-define-from-file=$FLUTTER_APP/.env"
    echo "Using Supabase config from .env"
fi

# Build/Run
echo ""
if [ "$RUN_APP" = true ]; then
    echo "Building and running on $PLATFORM..."
    if [ "$PLATFORM" = "macos" ]; then
        flutter run -d macos $RELEASE $DART_DEFINES
    else
        flutter run $RELEASE $DART_DEFINES
    fi
else
    echo "Building for $PLATFORM..."
    if [ "$BUILD_IPA" = true ]; then
        echo "Building IPA for TestFlight..."
        flutter build ipa $RELEASE $DART_DEFINES
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
                echo "Run: ./scripts/build-mobile.sh --setup-testflight"
                exit 1
            fi

            # Find the IPA file
            IPA_FILE=$(find "$FLUTTER_APP/build/ios/ipa" -name "*.ipa" -type f | head -1)
            if [ -z "$IPA_FILE" ]; then
                echo "Error: No IPA file found in build/ios/ipa/"
                exit 1
            fi

            echo "Uploading: $IPA_FILE"
            fastlane pilot upload \
                --api_key_path "$ASC_API_KEY_PATH" \
                --ipa "$IPA_FILE" \
                --skip_waiting_for_build_processing

            echo ""
            echo "Upload complete! Build will appear in TestFlight after processing."
        else
            echo "Upload to TestFlight using:"
            echo "  - Xcode: Open Organizer (Cmd+Shift+O) → Archives → Distribute"
            echo "  - CLI: ./scripts/build-mobile.sh --testflight"
            echo "  - Transporter app from Mac App Store"
        fi
    elif [ "$PLATFORM" = "ios" ]; then
        flutter build ios $RELEASE $DART_DEFINES
    elif [ "$PLATFORM" = "macos" ]; then
        flutter build macos $RELEASE $DART_DEFINES

        # Create DMG if requested
        if [ "$BUILD_DMG" = true ]; then
            echo ""
            echo "Creating DMG..."

            APP_PATH="$FLUTTER_APP/build/macos/Build/Products/Release/RhythmLighting.app"
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
                codesign --force --sign "$SIGNING_IDENTITY" --options runtime --timestamp --entitlements "$ENTITLEMENTS" "$APP_PATH/Contents/MacOS/RhythmLighting"

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
    else
        flutter build apk $RELEASE $DART_DEFINES
    fi
    echo ""
    echo "Build complete."
fi

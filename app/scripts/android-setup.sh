#!/bin/bash
# Create and maintain the Flutter Android platform scaffold for Rhythm Lighting.
#
# Usage: ./scripts/android-setup.sh [--pub] [--force]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"

APP_ID="${RHYTHM_ANDROID_APPLICATION_ID:-lighting.rhythm.app}"
APP_LABEL="${RHYTHM_ANDROID_APP_LABEL:-Rhythm Lighting}"
FLUTTER_ORG="${RHYTHM_ANDROID_ORG:-lighting.rhythm}"
PROJECT_NAME="rhythm_app"

RUN_PUB=false
FORCE=false

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    cat <<USAGE
Options:
  --pub      Run flutter pub get as part of flutter create
  --force    Recreate template files with Flutter --overwrite before patching
  -h, --help Show this help

Environment overrides:
  RHYTHM_ANDROID_APPLICATION_ID  Android applicationId/namespace (default: $APP_ID)
  RHYTHM_ANDROID_APP_LABEL       Launcher label (default: $APP_LABEL)
  RHYTHM_ANDROID_ORG             Flutter create org prefix (default: $FLUTTER_ORG)
USAGE
}

find_flutter() {
    if command -v flutter > /dev/null 2>&1; then
        command -v flutter
    elif [ -x "$HOME/Documents/flutter/bin/flutter" ]; then
        echo "$HOME/Documents/flutter/bin/flutter"
    elif [ -x "/opt/flutter/bin/flutter" ]; then
        echo "/opt/flutter/bin/flutter"
    else
        echo "Error: flutter not found. Install Flutter or add it to PATH." >&2
        exit 1
    fi
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pub)
            RUN_PUB=true
            shift
            ;;
        --force)
            FORCE=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            exit 1
            ;;
    esac
done

FLUTTER_CMD="$(find_flutter)"

if [ ! -f "$FLUTTER_APP/pubspec.yaml" ]; then
    echo "Error: Flutter app not found at $FLUTTER_APP" >&2
    exit 1
fi

CREATE_PLATFORMS=(android)
if [ "$FORCE" = false ]; then
    for existing_platform in ios macos web linux windows; do
        if [ -d "$FLUTTER_APP/$existing_platform" ]; then
            CREATE_PLATFORMS+=("$existing_platform")
        fi
    done
fi

CREATE_PLATFORMS_CSV="$(IFS=,; echo "${CREATE_PLATFORMS[*]}")"

CREATE_ARGS=(
    create
    --platforms="$CREATE_PLATFORMS_CSV"
    --project-name "$PROJECT_NAME"
    --org "$FLUTTER_ORG"
)

if [ "$RUN_PUB" = false ]; then
    CREATE_ARGS+=(--no-pub)
fi

if [ "$FORCE" = true ]; then
    CREATE_ARGS+=(--overwrite)
fi

ANDROID_DIR="$FLUTTER_APP/android"
APP_DIR="$ANDROID_DIR/app"

needs_flutter_create() {
    [ ! -d "$ANDROID_DIR" ] && return 0
    [ ! -f "$APP_DIR/build.gradle.kts" ] && return 0
    [ ! -f "$ANDROID_DIR/settings.gradle.kts" ] && return 0
    [ ! -f "$ANDROID_DIR/gradle/wrapper/gradle-wrapper.jar" ] && return 0
    [ ! -f "$ANDROID_DIR/gradlew" ] && return 0
    return 1
}

if [ "$FORCE" = true ] || needs_flutter_create; then
    echo "Creating Flutter Android scaffold..."
    (cd "$FLUTTER_APP" && "$FLUTTER_CMD" "${CREATE_ARGS[@]}" .)
else
    echo "Android scaffold already exists."
fi

MAIN_SRC="$APP_DIR/src/main"
MAIN_ACTIVITY_DIR="$MAIN_SRC/kotlin/lighting/rhythm/app"
MAIN_ACTIVITY="$MAIN_ACTIVITY_DIR/MainActivity.kt"

mkdir -p "$MAIN_ACTIVITY_DIR"

cat > "$MAIN_ACTIVITY" <<'KOTLIN'
package lighting.rhythm.app

import io.flutter.embedding.android.FlutterActivity

class MainActivity : FlutterActivity()
KOTLIN

# Remove the stock Flutter activity if it was generated under the template package.
OLD_ACTIVITY="$APP_DIR/src/main/kotlin/lighting/rhythm/rhythm_app/MainActivity.kt"
if [ -f "$OLD_ACTIVITY" ]; then
    rm "$OLD_ACTIVITY"
fi

cat > "$APP_DIR/build.gradle.kts" <<GRADLE
import java.util.Properties

plugins {
    id("com.android.application")
    id("kotlin-android")
    id("dev.flutter.flutter-gradle-plugin")
}

val keystoreProperties = Properties()
val keystorePropertiesFile = rootProject.file("key.properties")
if (keystorePropertiesFile.exists()) {
    keystorePropertiesFile.inputStream().use { keystoreProperties.load(it) }
}

val hasReleaseKeystore =
    keystorePropertiesFile.exists() &&
        keystoreProperties["storeFile"] != null &&
        keystoreProperties["storePassword"] != null &&
        keystoreProperties["keyAlias"] != null &&
        keystoreProperties["keyPassword"] != null

android {
    namespace = "$APP_ID"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        applicationId = "$APP_ID"
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        if (hasReleaseKeystore) {
            create("release") {
                storeFile = rootProject.file(keystoreProperties["storeFile"] as String)
                storePassword = keystoreProperties["storePassword"] as String
                keyAlias = keystoreProperties["keyAlias"] as String
                keyPassword = keystoreProperties["keyPassword"] as String
            }
        }
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.getByName(if (hasReleaseKeystore) "release" else "debug")
        }
    }
}

flutter {
    source = "../.."
}
GRADLE

cat > "$MAIN_SRC/AndroidManifest.xml" <<MANIFEST
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
    <uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
    <uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />

    <uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION" />
    <uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />

    <uses-permission android:name="android.permission.BLUETOOTH" android:maxSdkVersion="30" />
    <uses-permission android:name="android.permission.BLUETOOTH_ADMIN" android:maxSdkVersion="30" />
    <uses-permission android:name="android.permission.BLUETOOTH_SCAN" />
    <uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

    <uses-permission android:name="android.permission.CAMERA" />

    <uses-feature android:name="android.hardware.bluetooth_le" android:required="false" />
    <uses-feature android:name="android.hardware.camera" android:required="false" />

    <application
        android:label="$APP_LABEL"
        android:name="\${applicationName}"
        android:icon="@mipmap/ic_launcher"
        android:usesCleartextTraffic="true">
        <activity
            android:name="$APP_ID.MainActivity"
            android:exported="true"
            android:launchMode="singleTop"
            android:taskAffinity=""
            android:theme="@style/LaunchTheme"
            android:configChanges="orientation|keyboardHidden|keyboard|screenSize|smallestScreenSize|locale|layoutDirection|fontScale|screenLayout|density|uiMode"
            android:hardwareAccelerated="true"
            android:windowSoftInputMode="adjustResize">
            <meta-data
                android:name="io.flutter.embedding.android.NormalTheme"
                android:resource="@style/NormalTheme" />

            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>

            <intent-filter>
                <action android:name="android.intent.action.VIEW" />
                <category android:name="android.intent.category.DEFAULT" />
                <category android:name="android.intent.category.BROWSABLE" />
                <data android:scheme="rhythmapp" />
            </intent-filter>
        </activity>

        <meta-data
            android:name="flutterEmbedding"
            android:value="2" />
    </application>

    <queries>
        <intent>
            <action android:name="android.intent.action.PROCESS_TEXT" />
            <data android:mimeType="text/plain" />
        </intent>
        <intent>
            <action android:name="android.intent.action.VIEW" />
            <data android:scheme="http" />
        </intent>
        <intent>
            <action android:name="android.intent.action.VIEW" />
            <data android:scheme="https" />
        </intent>
        <intent>
            <action android:name="android.intent.action.SENDTO" />
            <data android:scheme="mailto" />
        </intent>
    </queries>
</manifest>
MANIFEST

echo "Android scaffold is ready at $ANDROID_DIR"
echo "Application ID: $APP_ID"
echo "Launcher label: $APP_LABEL"

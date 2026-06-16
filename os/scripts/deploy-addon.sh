#!/bin/bash
# Deploy the Rhythm OS addon
#
# Usage:
#   ./scripts/deploy-addon.sh [--release] [--no-push] [--skip-build] [--dry-run]
#   ./scripts/deploy-addon.sh --local [--ha-host HOST] [--ha-arch ARCH]
#
# Default mode (beta):
#   1. Builds Rust binaries + Docker image
#   2. Pushes Docker image to Docker Hub with beta tags
#   3. Syncs addon metadata to the beta addon repo
#   4. Commits/pushes the addon repo (skip with --no-push)
#
# Release mode (--release):
#   Same as above but uses production tags and addon repo

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ADDON_DIR="$PROJECT_ROOT/install/addon"

# Docker Hub image name (same for both tracks, distinguished by tags)
IMAGE_NAME="dtconcepts/rhythm-os-addon"

# Defaults
VERSION=""
SKIP_BUILD=false
DRY_RUN=false
LOCAL_DEPLOY=false
PUSH=true
RELEASE=false
HA_HOST="${HA_HOST:-homeassistant.local}"
HA_USER="${HA_USER:-root}"
HA_ARCH="${HA_ARCH:-aarch64}"
HA_ADDONS_PATH="/addons"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --no-push)
            PUSH=false
            shift
            ;;
        --push)
            PUSH=true
            shift
            ;;
        --release)
            RELEASE=true
            shift
            ;;
        --local)
            LOCAL_DEPLOY=true
            shift
            ;;
        --ha-host)
            HA_HOST="$2"
            shift 2
            ;;
        --ha-user)
            HA_USER="$2"
            shift 2
            ;;
        --ha-arch)
            HA_ARCH="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Deploys to the beta track by default. Use --release for production."
            echo ""
            echo "Docker Hub + Addon Repo (default):"
            echo "  --release         Deploy to production (default: beta)"
            echo "  --no-push         Skip committing/pushing addon repo changes"
            echo "  --skip-build      Skip Rust/Docker builds (use existing)"
            echo "  --version X.Y.Z   Version tag (default: auto-bump from config.yaml)"
            echo "  --dry-run         Show what would be done without doing it"
            echo ""
            echo "Local Development (--local):"
            echo "  --local           Deploy directly to HA instance via SSH"
            echo "  --ha-host HOST    HA hostname (default: homeassistant.local, or \$HA_HOST)"
            echo "  --ha-user USER    SSH user (default: root, or \$HA_USER)"
            echo "  --ha-arch ARCH    Target arch: aarch64, amd64, armv7 (default: aarch64)"
            echo ""
            echo "  -h, --help        Show this help"
            echo ""
            echo "Examples:"
            echo "  $0                              # Beta: build + push Docker + commit/push addon repo"
            echo "  $0 --release                    # Production: same but to prod repo with prod tags"
            echo "  $0 --no-push                    # Beta but skip addon repo commit/push"
            echo "  $0 --skip-build                 # Just sync + push addon repo (already built)"
            echo "  $0 --local                      # Build + copy to HA (aarch64)"
            echo "  $0 --local --ha-arch amd64      # Build + copy to HA (x86)"
            echo "  $0 --local --skip-build         # Just copy (already built)"
            echo ""
            echo "Environment:"
            echo "  HA_HOST           Alternative to --ha-host"
            echo "  HA_USER           Alternative to --ha-user"
            echo "  HA_ARCH           Alternative to --ha-arch"
            echo ""
            echo "Beta track:"
            echo "  Docker tags: \$VERSION-beta, beta"
            echo "  Addon repo:  sticktrk/rhythm-os-addon-beta"
            echo ""
            echo "Release track (--release):"
            echo "  Docker tags: \$VERSION, latest"
            echo "  Addon repo:  sticktrk/rhythm-os-addon"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Track-dependent variables
if [ "$RELEASE" = true ]; then
    TRACK="release"
    ADDON_GIT_REPO="git@github.com:sticktrk/rhythm-os-addon.git"
    ADDON_REPO_URL="https://github.com/sticktrk/rhythm-os-addon"
    REPO_NAME="Rhythm OS"
    README_TITLE="Rhythm OS Add-on for Home Assistant"
    README_ADDON_NAME="Rhythm OS"
else
    TRACK="beta"
    ADDON_GIT_REPO="git@github.com:sticktrk/rhythm-os-addon-beta.git"
    ADDON_REPO_URL="https://github.com/sticktrk/rhythm-os-addon-beta"
    REPO_NAME="Rhythm OS BETA"
    README_TITLE="Rhythm OS BETA Add-on for Home Assistant"
    README_ADDON_NAME="Rhythm OS BETA"
fi

# Bump version: increments the build number in 4.2.024-alpha → 4.2.025-alpha
bump_version() {
    local current
    current=$("$SCRIPT_DIR/resolve-version.sh" addon)
    if [ -z "$current" ]; then
        echo "Error: Could not read version from config.yaml"
        exit 1
    fi

    # Split into prefix (4.2.), build (024), and suffix (-alpha)
    local prefix build suffix
    prefix=$(echo "$current" | sed 's/\(.*\.\)[0-9]*\([-].*\)\{0,1\}$/\1/')
    build=$(echo "$current" | sed 's/.*\.\([0-9]*\)\([-].*\)\{0,1\}$/\1/')
    suffix=$(echo "$current" | sed -n 's/.*[0-9]\(-.*\)$/\1/p')

    # Increment build number, preserve zero-padding
    local width=${#build}
    local next=$((10#$build + 1))
    next=$(printf "%0${width}d" "$next")

    local new_version="${prefix}${next}${suffix}"
    sed -i '' "s/^version: .*/version: \"${new_version}\"/" "$ADDON_DIR/config.yaml" 2>/dev/null || \
    sed -i "s/^version: .*/version: \"${new_version}\"/" "$ADDON_DIR/config.yaml"

    echo "Version bumped: $current → $new_version"
}

# Always bump version unless --version was explicitly provided
if [ -z "$VERSION" ]; then
    bump_version
fi

# Get version from config.yaml if not specified
if [ -z "$VERSION" ]; then
    VERSION=$("$SCRIPT_DIR/resolve-version.sh" addon)
    if [ -z "$VERSION" ]; then
        echo "Error: Could not read version from config.yaml"
        exit 1
    fi
fi

# Docker tags depend on track
if [ "$RELEASE" = true ]; then
    DOCKER_VERSION_TAG="$VERSION"
    DOCKER_LATEST_TAG="latest"
else
    DOCKER_VERSION_TAG="$VERSION-beta"
    DOCKER_LATEST_TAG="beta"
fi

# ============================================================
# LOCAL DEPLOYMENT
# ============================================================
if [ "$LOCAL_DEPLOY" = true ]; then
    echo "========================================"
    echo "Local Deploy: Rhythm OS v$VERSION"
    echo "========================================"
    echo ""
    echo "Target: $HA_USER@$HA_HOST:$HA_ADDONS_PATH/rhythm"
    echo "Arch:   $HA_ARCH"
    echo ""

    # Build Rust
    if [ "$SKIP_BUILD" = false ]; then
        echo "=== Building Rust binary ($HA_ARCH) ==="
        "$SCRIPT_DIR/build-rust.sh" --release --target "$HA_ARCH"
    fi

    # Verify dist/ has required files
    echo ""
    echo "=== Verifying build artifacts ==="
    MISSING=false

    if [ ! -f "$PROJECT_ROOT/dist/bin/$HA_ARCH/rhythm-addon" ]; then
        echo "Missing: dist/bin/$HA_ARCH/rhythm-addon"
        MISSING=true
    else
        echo "Found: dist/bin/$HA_ARCH/rhythm-addon"
    fi

    if [ "$MISSING" = true ]; then
        echo ""
        echo "Error: Missing build artifacts. Run without --skip-build"
        exit 1
    fi

    # Prepare addon directory with build artifacts
    echo ""
    echo "=== Preparing addon directory ==="
    rm -rf "$ADDON_DIR/bin"
    mkdir -p "$ADDON_DIR/bin/$HA_ARCH"
    cp "$PROJECT_ROOT/dist/bin/$HA_ARCH/rhythm-addon" "$ADDON_DIR/bin/$HA_ARCH/"
    echo "Copied binary ($HA_ARCH)"

    # Remove image field so HA builds from local Dockerfile
    echo ""
    echo "=== Updating config.yaml (removing image field for local build) ==="
    CONFIG_FILE="$ADDON_DIR/config.yaml"
    grep -v "^image:" "$CONFIG_FILE" > "$CONFIG_FILE.tmp"
    mv "$CONFIG_FILE.tmp" "$CONFIG_FILE"

    echo ""
    echo "=== Deploying to $HA_USER@$HA_HOST ==="

    if [ "$DRY_RUN" = true ]; then
        echo "[DRY RUN] Would run:"
        echo "  ssh $HA_USER@$HA_HOST 'rm -rf $HA_ADDONS_PATH/rhythm'"
        echo "  scp -r $ADDON_DIR $HA_USER@$HA_HOST:$HA_ADDONS_PATH/rhythm"
    else
        # Test SSH connection
        if ! ssh -o ConnectTimeout=5 "$HA_USER@$HA_HOST" "echo 'SSH OK'" 2>/dev/null; then
            echo "Error: Cannot connect to $HA_USER@$HA_HOST"
            echo ""
            echo "Make sure:"
            echo "  1. SSH is enabled on your HA instance"
            echo "  2. You have SSH key authentication set up"
            echo "  3. The host is reachable: ping $HA_HOST"
            exit 1
        fi

        echo "Removing old addon..."
        ssh "$HA_USER@$HA_HOST" "rm -rf $HA_ADDONS_PATH/rhythm" 2>/dev/null || true

        echo "Copying addon files..."
        scp -r "$ADDON_DIR" "$HA_USER@$HA_HOST:$HA_ADDONS_PATH/rhythm"

        echo ""
        echo "========================================"
        echo "Local deployment complete!"
        echo "========================================"
        echo ""
        echo "Deployed to: $HA_USER@$HA_HOST:$HA_ADDONS_PATH/rhythm"
        echo ""
        echo "Next steps in Home Assistant:"
        echo "  1. Go to Settings -> Add-ons -> Add-on Store"
        echo "  2. Click the three dots menu -> Check for updates"
        echo "  3. Find 'Rhythm OS' under 'Local add-ons'"
        echo "  4. Click Install (or Rebuild if already installed)"
    fi
    exit 0
fi

# ============================================================
# DOCKER HUB + ADDON REPO DEPLOYMENT
# ============================================================
echo "========================================"
echo "Deploy Rhythm OS Addon v$VERSION ($TRACK)"
echo "========================================"
echo ""
echo "Docker tags:  $IMAGE_NAME:$DOCKER_VERSION_TAG, $IMAGE_NAME:$DOCKER_LATEST_TAG"
echo "Addon repo:   $ADDON_REPO_URL"
echo ""

# --- Step 1: Build everything ---
if [ "$SKIP_BUILD" = false ]; then
    echo "=== Building Rust binaries (all architectures) ==="
    "$SCRIPT_DIR/build-rust.sh" --release --target all
fi

# Verify build artifacts
echo ""
echo "=== Verifying build artifacts ==="
MISSING=false
for arch in amd64 aarch64 armv7; do
    if [ ! -f "$PROJECT_ROOT/dist/bin/$arch/rhythm-addon" ]; then
        echo "Missing: dist/bin/$arch/rhythm-addon"
        MISSING=true
    else
        echo "Found: dist/bin/$arch/rhythm-addon"
    fi
done

if [ "$MISSING" = true ]; then
    echo ""
    echo "Error: Missing build artifacts. Run without --skip-build"
    exit 1
fi

# --- Step 2: Prepare addon dir and build/push Docker image ---
echo ""
echo "=== Preparing addon directory ==="
rm -rf "$ADDON_DIR/bin"
cp -r "$PROJECT_ROOT/dist/bin" "$ADDON_DIR/bin"
echo "Copied binaries"

# Check Docker prerequisites (credential store or inline auths)
DOCKER_LOGGED_IN=false
if docker info 2>/dev/null | grep -q "Username"; then
    DOCKER_LOGGED_IN=true
elif [ -f "$HOME/.docker/config.json" ]; then
    # Check for credential store (Docker Desktop) or direct auths
    if python3 -c "
import json, sys
with open('$HOME/.docker/config.json') as f:
    c = json.load(f)
if c.get('credsStore') or any('docker.io' in k for k in c.get('auths', {})):
    sys.exit(0)
sys.exit(1)
" 2>/dev/null; then
        DOCKER_LOGGED_IN=true
    fi
fi

if [ "$DOCKER_LOGGED_IN" = false ]; then
    echo ""
    echo "Warning: Not logged into Docker Hub. Run: docker login"
    if [ "$DRY_RUN" = false ]; then
        exit 1
    fi
fi

if ! docker buildx version &>/dev/null; then
    echo "Error: Docker buildx not available. Install Docker Desktop or enable buildx."
    exit 1
fi

# Create/use buildx builder for multi-arch
BUILDER_NAME="rhythm-builder"
if ! docker buildx inspect "$BUILDER_NAME" &>/dev/null; then
    echo "Creating buildx builder: $BUILDER_NAME"
    docker buildx create --name "$BUILDER_NAME" --use
else
    docker buildx use "$BUILDER_NAME"
fi

echo ""
echo "=== Building and pushing Docker image ==="
cd "$ADDON_DIR"

if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would build and push:"
    echo "  $IMAGE_NAME:$DOCKER_VERSION_TAG"
    echo "  $IMAGE_NAME:$DOCKER_LATEST_TAG"
else
    docker buildx build \
        --platform linux/amd64,linux/arm64,linux/arm/v7 \
        --push \
        -t "$IMAGE_NAME:$DOCKER_VERSION_TAG" \
        -t "$IMAGE_NAME:$DOCKER_LATEST_TAG" \
        .
    echo "Pushed: $IMAGE_NAME:$DOCKER_VERSION_TAG, $IMAGE_NAME:$DOCKER_LATEST_TAG"
fi

# --- Step 3: Sync addon metadata to separate repo ---
echo ""
echo "=== Syncing addon repo ==="

# Clone target repo into temp dir
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would clone: $ADDON_GIT_REPO"
    ADDON_REPO_PATH="$TMPDIR/addon-repo"
    mkdir -p "$ADDON_REPO_PATH"
else
    echo "Cloning $ADDON_GIT_REPO..."
    git clone --depth 1 "$ADDON_GIT_REPO" "$TMPDIR/addon-repo"
    ADDON_REPO_PATH="$TMPDIR/addon-repo"
fi

# Remove legacy addon directories so Home Assistant does not see stale slugs.
LEGACY_ADDON_DIRS=("rhythm-lighting")
if [ "$DRY_RUN" = true ]; then
    for legacy_dir in "${LEGACY_ADDON_DIRS[@]}"; do
        echo "[DRY RUN] Would remove legacy addon dir: $legacy_dir/"
    done
else
    for legacy_dir in "${LEGACY_ADDON_DIRS[@]}"; do
        if [ -d "$ADDON_REPO_PATH/$legacy_dir" ]; then
            rm -rf "$ADDON_REPO_PATH/$legacy_dir"
            echo "Removed legacy addon dir: $legacy_dir/"
        fi
    done
fi

# Create repository.yaml (HA addon repo metadata)
if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would create/update: repository.yaml"
else
    echo "Updating repository.yaml..."
    cat > "$ADDON_REPO_PATH/repository.yaml" << EOF
name: $REPO_NAME
url: $ADDON_REPO_URL
maintainer: DT Concepts <hello@dtconcepts.com>
EOF
fi

# Sync addon config and metadata into rhythm-os/ subdirectory
ADDON_DEST="$ADDON_REPO_PATH/rhythm-os"
if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would sync files to: $ADDON_DEST/"
else
    mkdir -p "$ADDON_DEST"

    # config.yaml — ensure image: field is set for Docker Hub
    cp "$ADDON_DIR/config.yaml" "$ADDON_DEST/config.yaml"

    # Beta track: update config so HA pulls the correct Docker tag and avoids slug conflicts
    if [ "$RELEASE" = false ]; then
        sed -i '' "s/^name: .*/name: Rhythm OS BETA/" "$ADDON_DEST/config.yaml" 2>/dev/null || \
        sed -i "s/^name: .*/name: Rhythm OS BETA/" "$ADDON_DEST/config.yaml"
        sed -i '' "s/^slug: .*/slug: rhythm-beta/" "$ADDON_DEST/config.yaml" 2>/dev/null || \
        sed -i "s/^slug: .*/slug: rhythm-beta/" "$ADDON_DEST/config.yaml"
        sed -i '' "s/^version: .*/version: \"${VERSION}-beta\"/" "$ADDON_DEST/config.yaml" 2>/dev/null || \
        sed -i "s/^version: .*/version: \"${VERSION}-beta\"/" "$ADDON_DEST/config.yaml"
        sed -i '' "s/^description: .*/description: \"[BETA] Adaptive lighting that follows the sun\"/" "$ADDON_DEST/config.yaml" 2>/dev/null || \
        sed -i "s/^description: .*/description: \"[BETA] Adaptive lighting that follows the sun\"/" "$ADDON_DEST/config.yaml"
    fi

    # Ensure image: field is present
    if ! grep -q "^image:" "$ADDON_DEST/config.yaml"; then
        # Add image line after version
        awk -v img="image: $IMAGE_NAME" '/^version:/{print; print img; next}1' \
            "$ADDON_DEST/config.yaml" > "$ADDON_DEST/config.yaml.tmp"
        mv "$ADDON_DEST/config.yaml.tmp" "$ADDON_DEST/config.yaml"
    else
        # Update existing image line
        sed -i '' "s|^image:.*|image: $IMAGE_NAME|" "$ADDON_DEST/config.yaml" 2>/dev/null || \
        sed -i "s|^image:.*|image: $IMAGE_NAME|" "$ADDON_DEST/config.yaml"
    fi

    # Metadata files
    for file in CHANGELOG.md DOCS.md icon.png logo.png; do
        if [ -f "$ADDON_DIR/$file" ]; then
            cp "$ADDON_DIR/$file" "$ADDON_DEST/$file"
        fi
    done

    echo "Synced addon metadata to $ADDON_DEST/"
fi

# Update README so repo landing page stays in sync with the deployed track.
README="$ADDON_REPO_PATH/README.md"
if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would create/update: README.md"
else
    echo "Updating README.md..."
    cat > "$README" << EOF
# $README_TITLE

[![HA Add-on](https://img.shields.io/badge/HA-Add--on-41BDF5.svg)](https://www.home-assistant.io/addons/)

Adaptive lighting that follows the sun. Automatically adjusts brightness and color temperature throughout the day.

## Installation

1. Open Home Assistant
2. Go to **Settings** → **Add-ons** → **Add-on Store**
3. Click the three dots menu → **Repositories**
4. Add: \`$ADDON_REPO_URL\`
5. Find **$README_ADDON_NAME** and click **Install**
6. Start the add-on and check **Show in sidebar**

## Features

- Adaptive lighting curves driven by solar position
- Room-based control with per-room settings
- Switch integration via ZHA events
- Works with ZigBee, Z-Wave, WiFi, and Matter lights

## Configuration

The add-on auto-configures from Home Assistant (location, timezone, API token).

## More Info

- [Main repository](https://github.com/sticktrk/rhythm-os)
EOF
fi

# Commit and push if requested
if [ "$PUSH" = true ]; then
    if [ "$DRY_RUN" = true ]; then
        echo ""
        echo "[DRY RUN] Would commit and push to $ADDON_GIT_REPO:"
        echo "  git add -A"
        echo "  git commit -m 'Update to v$VERSION'"
        echo "  git push"
    else
        echo ""
        echo "Committing and pushing addon repo..."
        cd "$ADDON_REPO_PATH"
        git add -A
        if git diff --staged --quiet; then
            echo "No changes to commit"
        else
            git commit -m "Update to v$VERSION"
            git push
            echo "Pushed to $ADDON_REPO_URL"
        fi
    fi
fi

echo ""
echo "========================================"
echo "Deployment complete! ($TRACK)"
echo "========================================"
echo ""
echo "Docker image: $IMAGE_NAME:$DOCKER_VERSION_TAG, $IMAGE_NAME:$DOCKER_LATEST_TAG"
echo "Addon repo:   $ADDON_REPO_URL"
echo ""
if [ "$PUSH" = false ]; then
    echo "To commit/push addon repo changes, run without --no-push"
    echo ""
fi
echo "Users install by adding this repository in HA:"
echo "  $ADDON_REPO_URL"
echo ""

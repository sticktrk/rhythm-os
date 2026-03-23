#!/bin/bash
# Deploy the Rhythm Lighting integration to a separate HACS repo
#
# Usage: ./scripts/deploy-integration.sh [--repo-path PATH] [--push] [--dry-run]
#
# This script:
# 1. Copies custom_components from addon/ to a separate repo
# 2. Adds hacs.json and README if missing
# 3. Optionally commits and pushes

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# Source directory (may be in a submodule)
if [ -d "$PROJECT_ROOT/rhythm-lighting/install/addon" ]; then
    SOURCE_DIR="$PROJECT_ROOT/rhythm-lighting/install/addon/custom_components/rhythmlighting"
else
    SOURCE_DIR="$PROJECT_ROOT/install/addon/custom_components/rhythmlighting"
fi

# Defaults
REPO_PATH="${INTEGRATION_REPO:-$PROJECT_ROOT/../rhythm-lighting-integration}"
PUSH=false
DRY_RUN=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --repo-path)
            REPO_PATH="$2"
            shift 2
            ;;
        --push)
            PUSH=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --repo-path PATH  Path to integration repo (default: ../rhythm-lighting-integration)"
            echo "  --push            Commit and push changes"
            echo "  --dry-run         Show what would be done without doing it"
            echo "  -h, --help        Show this help"
            echo ""
            echo "Environment:"
            echo "  INTEGRATION_REPO  Alternative to --repo-path"
            echo ""
            echo "First time setup:"
            echo "  1. Create repo on GitHub: rhythm-lighting-integration"
            echo "  2. Clone it next to this repo"
            echo "  3. Run this script with --push"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Get version from manifest.json
VERSION=$(grep '"version"' "$SOURCE_DIR/manifest.json" | sed 's/.*: *"\([^"]*\)".*/\1/')
if [ -z "$VERSION" ]; then
    echo "Error: Could not read version from manifest.json"
    exit 1
fi

echo "========================================"
echo "Deploy Rhythm Lighting Integration v$VERSION"
echo "========================================"
echo ""
echo "Source: install/addon/custom_components/rhythmlighting/"
echo "Target: $REPO_PATH"
echo ""

# Check source exists
if [ ! -d "$SOURCE_DIR" ]; then
    echo "Error: Source not found: $SOURCE_DIR"
    exit 1
fi

# Check/create target repo
if [ ! -d "$REPO_PATH" ]; then
    if [ "$DRY_RUN" = true ]; then
        echo "[DRY RUN] Would create: $REPO_PATH"
    else
        echo "Creating integration repo directory..."
        mkdir -p "$REPO_PATH"
        cd "$REPO_PATH"
        git init
        echo "Initialized new git repo at $REPO_PATH"
        echo ""
        echo "Don't forget to add remote:"
        echo "  cd $REPO_PATH"
        echo "  git remote add origin https://github.com/sticktrk/rhythm-lighting-integration"
        echo ""
    fi
fi

if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would sync files:"
    echo "  $SOURCE_DIR/* -> $REPO_PATH/custom_components/rhythmlighting/"
    echo ""
else
    # Sync custom_components
    echo "Syncing custom_components..."
    mkdir -p "$REPO_PATH/custom_components/rhythmlighting"
    rsync -av --delete "$SOURCE_DIR/" "$REPO_PATH/custom_components/rhythmlighting/"
fi

# Create/update hacs.json
HACS_JSON="$REPO_PATH/hacs.json"
if [ "$DRY_RUN" = true ]; then
    echo "[DRY RUN] Would create/update: hacs.json"
else
    echo "Updating hacs.json..."
    cat > "$HACS_JSON" << EOF
{
  "name": "Rhythm Lighting",
  "hacs": "integration",
  "documentation": "https://github.com/sticktrk/rhythm-lighting",
  "issues": "https://github.com/sticktrk/rhythm-lighting/issues",
  "homeassistant": "2024.1.0"
}
EOF
fi

# Create README if missing
README="$REPO_PATH/README.md"
if [ ! -f "$README" ] || [ "$DRY_RUN" = true ]; then
    if [ "$DRY_RUN" = true ]; then
        echo "[DRY RUN] Would create: README.md"
    else
        echo "Creating README.md..."
        cat > "$README" << 'EOF'
# Rhythm Lighting Integration

[![hacs_badge](https://img.shields.io/badge/HACS-Custom-41BDF5.svg)](https://github.com/hacs/integration)

Home Assistant integration for [Rhythm Lighting](https://github.com/sticktrk/rhythm-lighting).

## Installation

### HACS (Recommended)

1. Open HACS in Home Assistant
2. Click the three dots menu → Custom repositories
3. Add `https://github.com/sticktrk/rhythm-lighting-integration` as an Integration
4. Search for "Rhythm Lighting" and install
5. Restart Home Assistant

### Manual

1. Copy `custom_components/rhythmlighting` to your HA `config/custom_components/` folder
2. Restart Home Assistant

## Configuration

1. Go to Settings → Devices & Services
2. Click Add Integration
3. Search for "Rhythm Lighting"

## Requirements

This integration works with the [Rhythm Lighting Add-on](https://github.com/sticktrk/rhythm-lighting). Install the add-on first.

## Services

- `rhythmlighting.rhythm_on` - Turn on with adaptive lighting
- `rhythmlighting.rhythm_off` - Disable rhythm mode
- `rhythmlighting.rhythm_toggle` - Toggle based on current state
- `rhythmlighting.step_up` - Step up brightness/color temp
- `rhythmlighting.step_down` - Step down brightness/color temp
- `rhythmlighting.reset` - Reset to current time position
EOF
    fi
fi

# Commit and push if requested
if [ "$PUSH" = true ]; then
    if [ "$DRY_RUN" = true ]; then
        echo ""
        echo "[DRY RUN] Would commit and push:"
        echo "  git add -A"
        echo "  git commit -m 'Update to v$VERSION'"
        echo "  git push"
    else
        echo ""
        echo "Committing and pushing..."
        cd "$REPO_PATH"
        git add -A
        if git diff --staged --quiet; then
            echo "No changes to commit"
        else
            git commit -m "Update to v$VERSION"
            if git remote get-url origin &>/dev/null; then
                git push
                echo "Pushed to origin"
            else
                echo "Warning: No remote 'origin' configured. Commit created but not pushed."
                echo "Add remote with: git remote add origin <url>"
            fi
        fi
    fi
fi

echo ""
echo "========================================"
echo "Integration deploy complete!"
echo "========================================"
echo ""
echo "Repo: $REPO_PATH"
if [ "$PUSH" = false ]; then
    echo ""
    echo "To push changes, run with --push flag"
fi
echo ""

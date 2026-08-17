#!/bin/bash
# Generate the GitHub release body for a stable appliance promotion.
#
# The important detail is the explicit previous_tag_name: beta releases may
# have been published more recently, but stable notes must cover the complete
# delta since the previous stable release.

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 CURRENT_STABLE_TAG OUTPUT_FILE" >&2
    exit 1
fi

CURRENT_TAG="$1"
OUTPUT_FILE="$2"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
REPOSITORY="${GITHUB_REPOSITORY:-}"

if ! printf '%s' "$CURRENT_TAG" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+-stable$'; then
    echo "Error: current tag must be vX.Y.Z-stable (got '$CURRENT_TAG')" >&2
    exit 1
fi

for command_name in git gh; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "Error: required command not found: $command_name" >&2
        exit 1
    fi
done

if [ -z "$REPOSITORY" ]; then
    REPOSITORY="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
fi

CURRENT_COMMIT="$(git -C "$REPO_ROOT" rev-list -n 1 "$CURRENT_TAG" 2>/dev/null || true)"
if [ -z "$CURRENT_COMMIT" ]; then
    echo "Error: stable tag is not present in this checkout: $CURRENT_TAG" >&2
    exit 1
fi

PREVIOUS_TAG="$(git -C "$REPO_ROOT" describe --tags --abbrev=0 \
    --match 'v[0-9]*-stable' "$CURRENT_COMMIT^" 2>/dev/null || true)"

generate_args=(
    --method POST
    "repos/$REPOSITORY/releases/generate-notes"
    -f "tag_name=$CURRENT_TAG"
)
if [ -n "$PREVIOUS_TAG" ]; then
    generate_args+=(-f "previous_tag_name=$PREVIOUS_TAG")
fi

GENERATED_NOTES="$(gh api "${generate_args[@]}" --jq .body)"
if [ -z "$GENERATED_NOTES" ]; then
    echo "Error: GitHub returned an empty stable release summary" >&2
    exit 1
fi
mkdir -p "$(dirname "$OUTPUT_FILE")"

{
    echo "# Rhythm $CURRENT_TAG"
    echo
    echo "Promoted to the stable appliance channel after beta verification."
    echo
    if [ -n "$PREVIOUS_TAG" ]; then
        echo "This release includes every merged pull request since [$PREVIOUS_TAG](https://github.com/$REPOSITORY/releases/tag/$PREVIOUS_TAG)."
    else
        echo "This is the first stable release found in the tag history."
    fi
    echo
    printf '%s\n' "$GENERATED_NOTES"
} > "$OUTPUT_FILE"

echo "Stable release notes: $CURRENT_TAG (previous: ${PREVIOUS_TAG:-none}) -> $OUTPUT_FILE"

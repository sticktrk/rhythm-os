# Optional publisher integration for the portable release commands.
# The executable receives the published tag and its exact source commit.
# It is supplied by the caller's environment, never evaluated as shell text.

validate_release_publish_hook() {
    if [ -n "${RHYTHM_RELEASE_PUBLISH_HOOK:-}" ]; then
        if [[ "$RHYTHM_RELEASE_PUBLISH_HOOK" != /* ]] || [ ! -x "$RHYTHM_RELEASE_PUBLISH_HOOK" ]; then
            echo "Error: RHYTHM_RELEASE_PUBLISH_HOOK must name an absolute executable path." >&2
            return 1
        fi
    fi
}

preview_release_publisher() {
    if [ -n "${RHYTHM_RELEASE_PUBLISH_HOOK:-}" ]; then
        printf '[dry-run] Would invoke the configured publisher for %s after pushing its tag.\n' "$1"
    fi
}

publish_release_tag() {
    local tag="$1"
    if [ -n "${RHYTHM_RELEASE_PUBLISH_HOOK:-}" ]; then
        local commit
        commit="$(git -C "$REPO_ROOT" rev-parse "refs/tags/$tag^{commit}")"
        "$RHYTHM_RELEASE_PUBLISH_HOOK" "$tag" "$commit"
    fi
}

#!/bin/bash

# Resolve one Flutter-compatible app-build file without placing credentials in
# the repository or an isolated worktree. Call cleanup_app_build_define_file
# through an EXIT trap after successful preparation.

RHYTHM_APP_BUILD_DEFINE_FILE=""
RHYTHM_APP_BUILD_DEFINE_TEMP=""
RHYTHM_APP_BUILD_PROFILE_SOURCE=""

prepare_app_build_define_file() {
    local repo_root="$1"
    local flutter_app="$2"
    local configured_file
    local materialized_file

    configured_file="${RHYTHM_APP_BUILD_ENV_FILE:-${RHYTHM_CONFIG_DIR:-$HOME/.config/rhythm}/app-build.env}"
    if [ ! -f "$configured_file" ] && [ -z "${RHYTHM_APP_BUILD_ENV_FILE:-}" ] && \
        [ -z "${RHYTHM_CONFIG_DIR:-}" ] && [ -f "$flutter_app/.env" ]; then
        RHYTHM_APP_BUILD_DEFINE_FILE="$flutter_app/.env"
        RHYTHM_APP_BUILD_PROFILE_SOURCE="legacy"
        return 0
    fi

    if ! materialized_file="$(
        RHYTHM_APP_BUILD_ENV_FILE="$configured_file" \
            python3 "$repo_root/tools/config/materialize_app_build.py"
    )"; then
        echo "Error: the external app-build profile is not ready." >&2
        return 1
    fi
    if [ -z "$materialized_file" ] || [ ! -f "$materialized_file" ]; then
        if [ -n "$materialized_file" ]; then
            rm -f -- "$materialized_file"
        fi
        echo "Error: the validated app-build profile was not materialized." >&2
        return 1
    fi

    RHYTHM_APP_BUILD_DEFINE_FILE="$materialized_file"
    RHYTHM_APP_BUILD_DEFINE_TEMP="$materialized_file"
    RHYTHM_APP_BUILD_PROFILE_SOURCE="external"
}

cleanup_app_build_define_file() {
    if [ -n "$RHYTHM_APP_BUILD_DEFINE_TEMP" ]; then
        rm -f -- "$RHYTHM_APP_BUILD_DEFINE_TEMP"
    fi
    RHYTHM_APP_BUILD_DEFINE_FILE=""
    RHYTHM_APP_BUILD_DEFINE_TEMP=""
    RHYTHM_APP_BUILD_PROFILE_SOURCE=""
}

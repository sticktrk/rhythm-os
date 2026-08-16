#!/bin/bash

set -euo pipefail

[ "${1:-}" = "api" ] || {
    echo "fake gh only supports gh api" >&2
    exit 2
}
shift

METHOD="GET"
ENDPOINT=""
INPUT=""
JQ_FILTER=""
PAGINATE=false
while [ $# -gt 0 ]; do
    case "$1" in
        --method)
            METHOD="$2"
            shift 2
            ;;
        --input)
            INPUT="$2"
            shift 2
            ;;
        --jq)
            JQ_FILTER="$2"
            shift 2
            ;;
        --paginate)
            PAGINATE=true
            shift
            ;;
        *)
            if [ -z "$ENDPOINT" ]; then
                ENDPOINT="$1"
                shift
            else
                echo "unexpected fake gh argument: $1" >&2
                exit 2
            fi
            ;;
    esac
done

printf '%s %s paginate=%s\n' "$METHOD" "$ENDPOINT" "$PAGINATE" >> "$FAKE_GH_LOG"

emit_json() {
    local value="$1"
    if [ -n "$JQ_FILTER" ]; then
        printf '%s\n' "$value" | jq -r "$JQ_FILTER"
    else
        printf '%s\n' "$value"
    fi
}

case "$METHOD $ENDPOINT" in
    "GET repos/test/repo/pulls/7")
        emit_json "$(jq -n --arg head "$FAKE_HEAD_SHA" --arg base "$FAKE_BASE_SHA" '{state: "open", head: {sha: $head}, base: {sha: $base, ref: "master"}, labels: []}')"
        ;;
    "GET repos/test/repo/issues/7/comments?per_page=100")
        if [ "${FAKE_EXISTING_MARKER:-false}" = true ]; then
            emit_json "$(jq -n --arg body "<!-- codex-cross-flutter-ui pr=7 head=$FAKE_HEAD_SHA -->" '[{body: $body}]')"
        else
            emit_json '[]'
        fi
        ;;
    "GET repos/test/repo")
        emit_json '{"default_branch":"master"}'
        ;;
    "GET repos/test/repo/git/ref/heads/master")
        emit_json "$(jq -n --arg sha "$FAKE_BASE_SHA" '{object: {sha: $sha}}')"
        ;;
    "GET repos/test/repo/git/ref/heads/codex-ui-evidence-pr-7")
        exit 1
        ;;
    "GET repos/test/repo/git/commits/$FAKE_BASE_SHA")
        emit_json '{"tree":{"sha":"dddddddddddddddddddddddddddddddddddddddd"}}'
        ;;
    "POST repos/test/repo/git/refs")
        emit_json "$(jq -n --arg sha "$FAKE_BASE_SHA" '{object: {sha: $sha}}')"
        ;;
    "POST repos/test/repo/git/blobs")
        emit_json '{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}'
        ;;
    "POST repos/test/repo/git/trees")
        cp "$INPUT" "$FAKE_TREE_JSON"
        emit_json '{"sha":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}'
        ;;
    "POST repos/test/repo/git/commits")
        emit_json '{"sha":"ffffffffffffffffffffffffffffffffffffffff"}'
        ;;
    "PATCH repos/test/repo/git/refs/heads/codex-ui-evidence-pr-7")
        emit_json '{}'
        ;;
    "POST repos/test/repo/issues/7/comments")
        cp "$INPUT" "$FAKE_COMMENT_JSON"
        emit_json '{}'
        ;;
    *)
        echo "unsupported fake gh request: $METHOD $ENDPOINT" >&2
        exit 2
        ;;
esac

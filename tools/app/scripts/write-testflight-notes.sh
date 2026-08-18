#!/bin/bash
# Render Apple's per-build "What to Test" text from a GitHub PR.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: write-testflight-notes.sh --pr NUMBER --title TITLE --body-file FILE --output FILE

Uses the PR's "TestFlight — What to Test" section when present, then falls
back to "Summary", and finally to the PR title. Output is capped at Apple's
4000-character per-build changelog limit.
EOF
}

PR_NUMBER=""
PR_TITLE=""
BODY_FILE=""
OUTPUT_FILE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --pr)
            PR_NUMBER="${2:?--pr requires a PR number}"
            shift 2
            ;;
        --title)
            PR_TITLE="${2:?--title requires text}"
            shift 2
            ;;
        --body-file)
            BODY_FILE="${2:?--body-file requires a path}"
            shift 2
            ;;
        --output)
            OUTPUT_FILE="${2:?--output requires a path}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if ! [[ "$PR_NUMBER" =~ ^[0-9]+$ ]] || [ -z "$PR_TITLE" ] || \
    [ -z "$BODY_FILE" ] || [ -z "$OUTPUT_FILE" ]; then
    usage >&2
    exit 1
fi
if [ ! -f "$BODY_FILE" ]; then
    echo "Error: PR body file not found: $BODY_FILE" >&2
    exit 1
fi

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-testflight-notes.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
CLEAN_BODY="$TEMP_ROOT/body.md"
SECTION_FILE="$TEMP_ROOT/section.md"
UNCAPPED_FILE="$TEMP_ROOT/uncapped.txt"

# PR templates contain guidance comments that must never become tester copy.
awk '
    BEGIN { in_comment = 0 }
    {
        line = $0
        while (1) {
            if (in_comment) {
                end = index(line, "-->")
                if (!end) {
                    line = ""
                    break
                }
                line = substr(line, end + 3)
                in_comment = 0
            }

            start = index(line, "<!--")
            if (!start) {
                break
            }

            before = substr(line, 1, start - 1)
            rest = substr(line, start + 4)
            end = index(rest, "-->")
            if (end) {
                line = before substr(rest, end + 3)
            } else {
                line = before
                in_comment = 1
                break
            }
        }
        print line
    }
' "$BODY_FILE" > "$CLEAN_BODY"

extract_section() {
    local requested="$1"
    awk -v requested="$requested" '
        function normalized_heading(line, value) {
            value = line
            sub(/^##[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return tolower(value)
        }
        /^##[[:space:]]+/ {
            heading = normalized_heading($0)
            if (active) {
                exit
            }
            if (requested == "testflight" &&
                (heading == "testflight" || heading == "what to test" ||
                 (heading ~ /^testflight/ && heading ~ /what to test$/))) {
                active = 1
                next
            }
            if (requested == "summary" && heading == "summary") {
                active = 1
                next
            }
        }
        active { print }
    ' "$CLEAN_BODY"
}

clean_section() {
    awk '
        {
            sub(/[[:space:]]+$/, "")
            if ($0 ~ /^[[:space:]]*[-*][[:space:]]*$/) {
                next
            }
            lines[++count] = $0
            if ($0 !~ /^[[:space:]]*$/) {
                last_nonblank = count
            }
        }
        END {
            first = 1
            while (first <= last_nonblank && lines[first] ~ /^[[:space:]]*$/) {
                first++
            }
            for (i = first; i <= last_nonblank; i++) {
                print lines[i]
            }
        }
    '
}

extract_section testflight | clean_section > "$SECTION_FILE"
if [ ! -s "$SECTION_FILE" ]; then
    extract_section summary | clean_section > "$SECTION_FILE"
fi

{
    printf 'PR #%s — %s\n' "$PR_NUMBER" "$PR_TITLE"
    if [ -s "$SECTION_FILE" ]; then
        printf '\n'
        cat "$SECTION_FILE"
    fi
} > "$UNCAPPED_FILE"

# Keep complete UTF-8 characters in normal macOS/Linux locales and leave room
# for an ASCII truncation marker when unusually long PR copy exceeds the store
# limit.
awk -v max_chars=4000 '
    {
        text = text (NR == 1 ? "" : "\n") $0
    }
    END {
        if (length(text) > max_chars) {
            print substr(text, 1, max_chars - 3) "..."
        } else {
            print text
        }
    }
' "$UNCAPPED_FILE" > "$OUTPUT_FILE"

if [ ! -s "$OUTPUT_FILE" ]; then
    echo "Error: rendered TestFlight notes are empty." >&2
    exit 1
fi

#!/usr/bin/env bash
# Regression test for the AWS CLI bootstrap archive extractor.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EXTRACTOR="$REPO_ROOT/.github/actions/configure-updates-r2/extract_zip_preserving_permissions.py"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-r2-extract-test.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

python3 - "$TEMP_DIR/awscliv2.zip" <<'PY'
import stat
import sys
import zipfile


def add(archive, name, content, mode):
    member = zipfile.ZipInfo(name)
    member.create_system = 3
    member.external_attr = mode << 16
    archive.writestr(member, content)


with zipfile.ZipFile(sys.argv[1], "w") as archive:
    add(archive, "aws/", b"", stat.S_IFDIR | 0o755)
    add(archive, "aws/install", b"#!/usr/bin/env bash\n", stat.S_IFREG | 0o755)
    add(archive, "aws/README.md", b"fixture\n", stat.S_IFREG | 0o644)
PY

python3 "$EXTRACTOR" "$TEMP_DIR/awscliv2.zip" "$TEMP_DIR/extracted"

test -x "$TEMP_DIR/extracted/aws/install"
test ! -x "$TEMP_DIR/extracted/aws/README.md"

echo "AWS CLI ZIP permissions preserved"

#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
python3 - <<'CHECK'
from pathlib import Path
import re
import subprocess
root = Path('.')
versions = set()
for file in (root / 'tools/app/supabase/migrations').glob('*.sql'):
    match = re.fullmatch(r'(\d{14})_[a-z0-9_]+\.sql', file.name)
    assert match, 'Invalid migration filename: ' + file.name
    assert match[1] not in versions, 'Duplicate migration version: ' + match[1]
    versions.add(match[1])
# Inspect tracked paths; ignored local tool state is allowed in a developer clone.
if (root / '.git').exists():
    tracked = subprocess.check_output(['git', 'ls-files', '-z']).decode().split('\0')
else:
    tracked = [str(p) for p in root.rglob('*') if p.is_file()]
private_prefixes = ['.triage/', '.codex/', '.claude/', 'rhythm-marketing/', 'deploy/avo.rhythm.lighting/', 'tools/app/supabase/functions/blog-post-intake/']
for name in tracked:
    assert not any(name.startswith(prefix) for prefix in private_prefixes), 'Private file tracked: ' + name
    assert not name.endswith('/flutter_log.txt'), 'Runtime log tracked: ' + name
    basename = Path(name).name
    assert not (basename == '.env' or (basename.startswith('.env.') and basename != '.env.example')), 'Live environment file tracked: ' + name
assert 'Apache License' in (root / 'LICENSE').read_text()
assert 'Apache License' in (root / 'app/LICENSE').read_text()
print('Public source boundaries, licenses and migration versions passed.')
CHECK
python3 -m unittest tools/tests/test_prepare_supabase_workdir.py
bash tools/app/scripts/tests/deploy-supabase-functions-test.sh
bash tools/app/scripts/tests/deploy-supabase-test.sh
python3 -m unittest tools/tests/test_release_publication.py
python3 -m unittest tools/tests/test_testflight_build.py
python3 tools/config/test_rhythm_env.py
for fixture in \
    tools/ci/test-detect-changed-surfaces.sh \
    tools/ci/test-extract-zip-preserving-permissions.sh \
    tools/ci/test-post-flutter-ui-evidence.sh \
    tools/ci/test-render-flutter-ui-evidence.sh \
    tools/app/scripts/tests/app-build-profile-test.sh \
    tools/app/scripts/tests/export-testflight-ipa-test.sh \
    tools/app/scripts/tests/testflight-notes-test.sh \
    tools/app/scripts/tests/write-app-build-receipt-test.sh \
    tools/os/scripts/tests/r2-publishing-sim.sh \
    tools/os/scripts/tests/stable-release-notes-sim.sh; do
    bash "$fixture"
done

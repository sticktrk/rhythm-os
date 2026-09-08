#!/usr/bin/env python3
"""Interactively upload the shared Monster account secrets; never store them in the repo."""
import argparse
import getpass
import json
import os
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project-ref', required=True)
    args = parser.parse_args()
    # One vendor application credential plus the shared account email/password.
    # Tickets derive their key from the account; there are no per-device secrets.
    names = ('MONSTER_EMAIL', 'MONSTER_PASSWORD', 'MONSTER_AYLA_APP_SECRET', 'MONSTER_OWNER_USER_ID')
    values = {}
    for name in names:
        hint = ' (blank keeps the account shared)' if name == 'MONSTER_OWNER_USER_ID' else ''
        values[name] = getpass.getpass(f'{name}{hint}: ')
    if not values['MONSTER_OWNER_USER_ID'].strip():
        del values['MONSTER_OWNER_USER_ID']
    if any(not isinstance(value, str) or not value or '\n' in value or '\r' in value for value in values.values()):
        parser.error('Every value must be nonempty and single-line')
    # NamedTemporaryFile is owner-only and deleted even when upload fails.
    with tempfile.NamedTemporaryFile(mode='w', prefix='rhythm-monster-', suffix='.env') as f:
        os.fchmod(f.fileno(), 0o600)
        for name, value in values.items():
            f.write(f'{name}={json.dumps(value, ensure_ascii=False).replace(chr(36), chr(92) + chr(36))}\n')
        f.flush()
        result = subprocess.run(['supabase', 'secrets', 'set', '--project-ref',
                                 args.project_ref, '--env-file', f.name],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if result.returncode:
        raise SystemExit('Secret upload failed; check Supabase CLI authentication/project access.')
    print('Monster account secrets uploaded. Temporary file removed. Function deployment is separate.')


if __name__ == '__main__':
    main()

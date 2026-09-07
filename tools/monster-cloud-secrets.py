#!/usr/bin/env python3
"""Interactively upload owner-scoped Monster secrets; never store them in the repo."""
import argparse
import getpass
import json
import os
import secrets
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project-ref', required=True)
    parser.add_argument('--ayla-config', help='Private app configuration containing appId and appSecret')
    args = parser.parse_args()
    names = ('MONSTER_OWNER_USER_ID', 'MONSTER_EMAIL', 'MONSTER_PASSWORD',
             'MONSTER_APP_ID', 'MONSTER_APP_SECRET')
    values = {}
    if args.ayla_config:
        try:
            with open(args.ayla_config) as config_file:
                config = json.load(config_file)
            values = {'MONSTER_APP_ID': config['appId'], 'MONSTER_APP_SECRET': config['appSecret']}
        except (OSError, ValueError, KeyError, TypeError):
            parser.error('Cannot read Ayla application configuration')
    for name in names:
        if name not in values:
            values[name] = getpass.getpass(f'{name}: ')
    if any(not isinstance(value, str) or not value or '\n' in value or '\r' in value for value in values.values()):
        parser.error('Every value must be nonempty and single-line')
    values['MONSTER_TICKET_SECRET'] = secrets.token_hex(32)
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
    print('Monster secrets uploaded. Temporary file removed. Function deployment is separate.')


if __name__ == '__main__':
    main()

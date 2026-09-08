#!/usr/bin/env python3
"""Compose unchanged core/marketing migrations for an existing shared project.

The destination is a temporary CLI workdir, never a second source of schema.
Function sources and secrets stay with their owning repositories.
"""
import argparse
from pathlib import Path
import re
import shutil


def prepare(core: Path, marketing: Path, destination: Path) -> None:
    roots = (core, marketing / 'supabase')
    migrations = {}
    for root in roots:
        if not (root / 'config.toml').is_file():
            raise ValueError(f'Missing Supabase config in {root}')
        files = sorted((root / 'migrations').glob('*.sql'))
        if not files:
            raise ValueError(f'Missing migrations in {root}')
        for source in files:
            match = re.fullmatch(r'(\d{14})_.+\.sql', source.name)
            if not match:
                raise ValueError(f'Invalid migration filename: {source.name}')
            version = match.group(1)
            if version in migrations:
                raise ValueError(f'Duplicate migration version: {version}')
            migrations[version] = source
    project = destination / 'supabase'
    project.mkdir(parents=True, exist_ok=False)
    (project / 'migrations').mkdir()
    shutil.copy2(core / 'config.toml', project / 'config.toml')
    if (core / '.temp').is_dir():
        shutil.copytree(core / '.temp', project / '.temp')
    for source in migrations.values():
        shutil.copy2(source, project / 'migrations' / source.name)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path, required=True)
    parser.add_argument('--marketing-repo', type=Path, required=True)
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    try:
        prepare(args.core, args.marketing_repo, args.destination)
    except (ValueError, OSError) as error:
        parser.exit(1, f'Cannot compose Supabase histories: {error}\n')


if __name__ == '__main__':
    main()

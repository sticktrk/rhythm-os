#!/usr/bin/env python3
"""Generate the storage lifecycle manifest inventory from Rust.

The source of truth is `STORE_MANIFEST` in `rhythm-os/src/store_manifest.rs`.
This script turns that typed table into a stable JSON artifact for PRD review,
CI drift checks, and backup/reset/restore migration work.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path


INVENTORY_VERSION = 1
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("store_manifest.json")
SOURCE_FILE = Path("rust/core/rhythm-os/src/store_manifest.rs")

STORE_ENTRY_RE = re.compile(
    r"StoreManifestEntry\s*\{\s*"
    r'key:\s*"(?P<key>[^"]+)",\s*'
    r"path_kind:\s*StorePathKind::(?P<path_kind>[A-Za-z]+),\s*"
    r'path:\s*(?:Some\("(?P<path>[^"]+)"\)|None),\s*'
    r'purpose:\s*"(?P<purpose>[^"]+)",\s*'
    r"introduced_by:\s*StoreIntroducedBy::(?P<introduced_by>[A-Za-z0-9]+),\s*"
    r"factory_reset:\s*FactoryResetBehavior::(?P<factory_reset>[A-Za-z]+),\s*"
    r"backup:\s*BackupBehavior::(?P<backup>[A-Za-z]+),\s*"
    r"restore:\s*RestoreBehavior::(?P<restore>[A-Za-z]+),\s*"
    r"\}",
    re.MULTILINE | re.DOTALL,
)


def snake_case(name: str) -> str:
    out: list[str] = []
    for index, char in enumerate(name):
        if char.isdigit() and index > 0 and not name[index - 1].isdigit():
            out.append("_")
        if char.isupper() and index > 0:
            previous = name[index - 1]
            next_char = name[index + 1] if index + 1 < len(name) else ""
            if not previous.isupper() or (next_char and next_char.islower()):
                out.append("_")
        out.append(char.lower())
    return "".join(out)


def inventory(root: Path) -> dict:
    source_path = root / SOURCE_FILE
    source = source_path.read_text(encoding="utf-8")

    rows = []
    seen_keys: set[str] = set()
    for match in STORE_ENTRY_RE.finditer(source):
        key = match.group("key")
        if key in seen_keys:
            raise RuntimeError(f"duplicate StoreManifestEntry for {key} in {source_path}")
        seen_keys.add(key)
        rows.append(
            {
                "key": key,
                "path_kind": snake_case(match.group("path_kind")),
                "path": match.group("path"),
                "purpose": match.group("purpose"),
                "introduced_by": snake_case(match.group("introduced_by")),
                "factory_reset": snake_case(match.group("factory_reset")),
                "backup": snake_case(match.group("backup")),
                "restore": snake_case(match.group("restore")),
            }
        )

    if not rows:
        raise RuntimeError(f"no StoreManifestEntry entries found in {source_path}")

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_store_manifest.py",
        "source": SOURCE_FILE.as_posix(),
        "summary": summarize(rows),
        "stores": rows,
    }


def count_by(rows: list[dict], key: str) -> dict:
    counts: dict[str, int] = {}
    for row in rows:
        counts[row[key]] = counts.get(row[key], 0) + 1
    return dict(sorted(counts.items()))


def summarize(rows: list[dict]) -> dict:
    return {
        "store_count": len(rows),
        "reset_file_count": sum(
            1
            for row in rows
            if row["factory_reset"] == "clear" and row["path_kind"] == "file"
        ),
        "backup_included_count": sum(
            1
            for row in rows
            if row["backup"] in {"include", "include_redacted", "secret_only"}
        ),
        "factory_reset": count_by(rows, "factory_reset"),
        "backup": count_by(rows, "backup"),
        "restore": count_by(rows, "restore"),
        "introduced_by": count_by(rows, "introduced_by"),
    }


def dumps(data: dict) -> str:
    return json.dumps(data, indent=2, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true", help="fail if output is stale")
    parser.add_argument("--write", action="store_true", help="write the inventory file")
    args = parser.parse_args()

    root = args.root.resolve()
    output = args.output
    if not output.is_absolute():
        output = root / output

    rendered = dumps(inventory(root))

    if args.write:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered, encoding="utf-8")
        return 0

    if args.check:
        if not output.exists():
            print(f"missing inventory: {output}", file=sys.stderr)
            return 1
        current = output.read_text(encoding="utf-8")
        if current != rendered:
            diff = difflib.unified_diff(
                current.splitlines(keepends=True),
                rendered.splitlines(keepends=True),
                fromfile=str(output),
                tofile="generated",
            )
            sys.stderr.writelines(diff)
            return 1
        return 0

    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

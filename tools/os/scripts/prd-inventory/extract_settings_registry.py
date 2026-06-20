#!/usr/bin/env python3
"""Generate the settings registry inventory from Rust.

The source of truth is `SETTING_SPECS` in `rhythm-core/src/settings.rs`.
This script turns that typed table into a stable JSON artifact for PRD review,
CI drift checks, and settings migration work.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path


INVENTORY_VERSION = 1
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("settings_registry.json")
SOURCE_FILE = Path("rust/core/rhythm-core/src/settings.rs")

SETTING_SPEC_RE = re.compile(
    r"SettingSpec\s*\{\s*"
    r'key:\s*"(?P<key>[^"]+)",\s*'
    r"key_match:\s*SettingKeyMatch::(?P<key_match>[A-Za-z]+),\s*"
    r'default_value:\s*"(?P<default>[^"]*)",\s*'
    r"value_kind:\s*SettingValueKind::(?P<value_kind>[A-Za-z]+),\s*"
    r'consumed_by:\s*"(?P<consumed_by>[^"]+)",\s*'
    r'python_source:\s*"(?P<python_source>[^"]+)",\s*'
    r"\}",
    re.MULTILINE | re.DOTALL,
)
STORED_VALUE_SPEC_RE = re.compile(
    r"StoredValueSpec\s*\{\s*"
    r'key:\s*"(?P<key>[^"]+)",\s*'
    r'store:\s*"(?P<store>[^"]+)",\s*'
    r'purpose:\s*"(?P<purpose>[^"]+)",\s*'
    r"\}",
    re.MULTILINE | re.DOTALL,
)


def snake_case(name: str) -> str:
    out: list[str] = []
    for index, char in enumerate(name):
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

    settings = []
    seen_keys: set[str] = set()
    for match in SETTING_SPEC_RE.finditer(source):
        key = match.group("key")
        if key in seen_keys:
            raise RuntimeError(f"duplicate SettingSpec for {key} in {source_path}")
        seen_keys.add(key)
        settings.append(
            {
                "key": key,
                "match": snake_case(match.group("key_match")),
                "default": match.group("default"),
                "value_kind": snake_case(match.group("value_kind")),
                "consumed_by": match.group("consumed_by"),
                "python_source": match.group("python_source"),
            }
        )

    stored_non_settings = [
        {
            "key": match.group("key"),
            "store": match.group("store"),
            "purpose": match.group("purpose"),
        }
        for match in STORED_VALUE_SPEC_RE.finditer(source)
    ]

    if not settings:
        raise RuntimeError(f"no SettingSpec entries found in {source_path}")

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_settings_registry.py",
        "source": SOURCE_FILE.as_posix(),
        "summary": summarize(settings, stored_non_settings),
        "settings": settings,
        "stored_non_settings": stored_non_settings,
    }


def summarize(settings: list[dict], stored_non_settings: list[dict]) -> dict:
    value_kinds: dict[str, int] = {}
    match_kinds: dict[str, int] = {}
    for row in settings:
        value_kinds[row["value_kind"]] = value_kinds.get(row["value_kind"], 0) + 1
        match_kinds[row["match"]] = match_kinds.get(row["match"], 0) + 1

    return {
        "setting_count": len(settings),
        "stored_non_setting_count": len(stored_non_settings),
        "wildcard_settings": sum(1 for row in settings if row["match"] != "exact"),
        "match_kinds": dict(sorted(match_kinds.items())),
        "value_kinds": dict(sorted(value_kinds.items())),
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

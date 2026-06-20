#!/usr/bin/env python3
"""Generate the activity source classification inventory from Rust.

The source of truth is `ACTIVITY_SOURCE_RULES` in `rhythm-core/src/activity.rs`.
This script turns that typed table into a stable JSON artifact for PRD review,
CI drift checks, and importer work.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path


INVENTORY_VERSION = 1
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("activity_sources.json")
SOURCE_FILE = Path("rust/core/rhythm-core/src/activity.rs")

RULE_RE = re.compile(
    r"ActivitySourceRule\s*\{\s*"
    r'raw:\s*"(?P<raw>[^"]+)",\s*'
    r"kind:\s*ActivitySourceKnownKind::(?P<kind>[A-Za-z]+),\s*"
    r"marks_touched:\s*(?P<marks>true|false),\s*"
    r"\}",
    re.MULTILINE,
)


def snake_case(name: str) -> str:
    out: list[str] = []
    for index, char in enumerate(name):
        if char.isupper() and index > 0:
            out.append("_")
        out.append(char.lower())
    return "".join(out)


def inventory(root: Path) -> dict:
    source_path = root / SOURCE_FILE
    source = source_path.read_text(encoding="utf-8")
    rules = []
    for match in RULE_RE.finditer(source):
        rules.append(
            {
                "raw": match.group("raw"),
                "kind": snake_case(match.group("kind")),
                "marks_touched": match.group("marks") == "true",
            }
        )

    if not rules:
        raise RuntimeError(f"no ActivitySourceRule entries found in {source_path}")

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_activity_sources.py",
        "source": SOURCE_FILE.as_posix(),
        "dynamic_prefixes": [
            {
                "raw_prefix": "switch:",
                "kind": "switch",
                "marks_touched": True,
            }
        ],
        "unknown_default": {
            "kind": "unknown",
            "marks_touched": False,
            "preserve_raw": True,
        },
        "summary": {
            "exact_rules": len(rules),
            "touching_rules": sum(1 for rule in rules if rule["marks_touched"]),
            "non_touching_rules": sum(1 for rule in rules if not rule["marks_touched"]),
        },
        "rules": sorted(rules, key=lambda rule: rule["raw"]),
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

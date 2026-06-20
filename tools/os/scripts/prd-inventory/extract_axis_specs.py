#!/usr/bin/env python3
"""Generate the AxisSpec inventory from Rust.

The source of truth is `AXIS_SPECS` in `rhythm-core/src/axis.rs`.
This script turns that typed table into a stable JSON artifact for PRD review,
CI drift checks, and scope-tree work.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path


INVENTORY_VERSION = 1
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("axis_specs.json")
SOURCE_FILE = Path("rust/core/rhythm-core/src/axis.rs")

KIND_CONST_RE = re.compile(
    r"const\s+(?P<name>[A-Z_]+):\s*&\[ScopeNodeKind\]\s*=\s*&\[(?P<body>.*?)\];",
    re.MULTILINE | re.DOTALL,
)
KIND_RE = re.compile(r"ScopeNodeKind::(?P<kind>[A-Za-z]+)")
AXIS_SPEC_RE = re.compile(
    r"AxisSpec\s*\{\s*"
    r"axis:\s*StateAxis::(?P<axis>[A-Za-z]+),\s*"
    r"kinds:\s*(?P<kinds>[A-Z_]+|&\[[^\]]+\]),\s*"
    r"combine:\s*AxisCombine::(?P<combine>[A-Za-z]+),\s*"
    r"promotion:\s*AxisPromotion::(?P<promotion>[A-Za-z]+),\s*"
    r"reset:\s*AxisResetBehavior::(?P<reset>[A-Za-z]+),\s*"
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


def parse_kind_expr(expr: str, kind_consts: dict[str, list[str]]) -> list[str]:
    expr = expr.strip()
    if expr in kind_consts:
        return kind_consts[expr]

    inline = [snake_case(match.group("kind")) for match in KIND_RE.finditer(expr)]
    if inline:
        return inline

    raise RuntimeError(f"unresolved ScopeNodeKind expression: {expr}")


def parse_kind_consts(source: str) -> dict[str, list[str]]:
    return {
        match.group("name"): [
            snake_case(kind_match.group("kind"))
            for kind_match in KIND_RE.finditer(match.group("body"))
        ]
        for match in KIND_CONST_RE.finditer(source)
    }


def inventory(root: Path) -> dict:
    source_path = root / SOURCE_FILE
    source = source_path.read_text(encoding="utf-8")
    kind_consts = parse_kind_consts(source)

    rows = []
    seen_axes: set[str] = set()
    for match in AXIS_SPEC_RE.finditer(source):
        axis = snake_case(match.group("axis"))
        if axis in seen_axes:
            raise RuntimeError(f"duplicate AxisSpec for {axis} in {source_path}")
        seen_axes.add(axis)

        rows.append(
            {
                "axis": axis,
                "kinds": parse_kind_expr(match.group("kinds"), kind_consts),
                "combine": snake_case(match.group("combine")),
                "promotion": snake_case(match.group("promotion")),
                "reset_behavior": snake_case(match.group("reset")),
            }
        )

    if not rows:
        raise RuntimeError(f"no AxisSpec entries found in {source_path}")

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_axis_specs.py",
        "source": SOURCE_FILE.as_posix(),
        "summary": summarize(rows),
        "axes": rows,
    }


def summarize(rows: list[dict]) -> dict:
    combines: dict[str, int] = {}
    promotions: dict[str, int] = {}
    reset_behaviors: dict[str, int] = {}
    for row in rows:
        combines[row["combine"]] = combines.get(row["combine"], 0) + 1
        promotions[row["promotion"]] = promotions.get(row["promotion"], 0) + 1
        reset_behaviors[row["reset_behavior"]] = reset_behaviors.get(row["reset_behavior"], 0) + 1

    return {
        "axis_count": len(rows),
        "promotable_axes": sum(1 for row in rows if row["promotion"] != "none"),
        "combines": dict(sorted(combines.items())),
        "promotions": dict(sorted(promotions.items())),
        "reset_behaviors": dict(sorted(reset_behaviors.items())),
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

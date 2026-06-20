#!/usr/bin/env python3
"""Inventory Rust `as u8` conversion sites for the Circadian pipeline work.

This is intentionally lexical rather than a Rust parser. The PRD gate needs a
cheap, deterministic inventory that fails when load-bearing conversion sites are
added, removed, or reshaped. The output classifies all `as u8` casts under
`rust/core/rhythm-{core,os}/src` and marks the runtime brightness sites that the
Circadian value-pipeline strangler must absorb first.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


INVENTORY_VERSION = 2
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("quantization_sites.json")
SCAN_ROOTS = (
    Path("rust/core/rhythm-core/src"),
    Path("rust/core/rhythm-os/src"),
)
RUNTIME_BRIGHTNESS_FILES = {
    "rust/core/rhythm-core/src/actions.rs",
    "rust/core/rhythm-core/src/primitives.rs",
    "rust/core/rhythm-os/src/commands.rs",
}


FN_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b"
)
MOD_TESTS_RE = re.compile(r"\bmod\s+tests\b")


@dataclass(frozen=True)
class CastSite:
    path: str
    line: int
    function: str | None
    category: str
    pipeline_candidate: bool
    test_context: bool
    statement: str

    def to_json(self) -> dict:
        return {
            "path": self.path,
            "function": self.function,
            "category": self.category,
            "pipeline_candidate": self.pipeline_candidate,
            "test_context": self.test_context,
            "statement": self.statement,
        }


def normalize_statement(lines: Iterable[str]) -> str:
    normalized = " ".join(" ".join(lines).split())
    return normalized.removesuffix(" }")


def collect_statement(lines: list[str], index: int) -> str:
    start = index
    while start > 0 and index - start < 8:
        previous = lines[start - 1].strip()
        if not previous:
            break
        if previous.endswith(";") or previous.endswith("{"):
            break
        start -= 1

    end = index
    while end + 1 < len(lines) and end - index < 8:
        current = lines[end].strip()
        if ";" in current or current.endswith(",") or current == "}" or current.startswith("}"):
            break
        end += 1

    return normalize_statement(
        line.strip() for line in lines[start : end + 1] if line.strip() != "}"
    )


def categorize(path: str, function: str | None, statement: str, test_context: bool) -> str:
    if test_context:
        return "test_only"

    lower = statement.lower()
    filename = Path(path).name

    if filename == "quantization.rs":
        return "quantization_helper"
    if path.endswith("canonical/identity.rs"):
        return "identity_byte"
    if "control_id" in lower:
        return "button_wire_id"
    if filename in {"solar.rs", "room.rs", "periodic.rs"} and (
        "hour" in lower or "minute" in lower or "minutes_since_midnight" in lower
    ):
        return "time_component"
    if filename == "controller_helpers.rs" and ("hue" in lower or "saturation" in lower):
        return "color_hs_wire"
    if filename == "handlers.rs":
        return "api_parse_or_clamp"
    if path.endswith("light_profile/profile.rs") and (
        "brightness" in lower or re.search(r"\bbri\b", lower)
    ):
        return "lighting_profile_brightness"
    if "warning_dim_factor" in lower or "brightness" in lower or re.search(r"\bbri\b", lower):
        if path in RUNTIME_BRIGHTNESS_FILES:
            return "light_runtime_brightness"
        return "lighting_brightness_other"
    if function and function.startswith("calculate_"):
        return "profile_or_curve_output"

    return "other"


def iter_rust_files(root: Path) -> Iterable[Path]:
    for scan_root in SCAN_ROOTS:
        full = root / scan_root
        if not full.exists():
            continue
        yield from sorted(full.rglob("*.rs"))


def inventory(root: Path) -> dict:
    sites: list[CastSite] = []

    for file_path in iter_rust_files(root):
        rel_path = file_path.relative_to(root).as_posix()
        lines = file_path.read_text(encoding="utf-8").splitlines()
        depth = 0
        test_depths: list[int] = []
        fn_stack: list[tuple[int, str]] = []
        pending_fn: str | None = None

        for index, line in enumerate(lines):
            stripped = line.strip()
            fn_match = FN_RE.match(line)
            if fn_match:
                fn_name = fn_match.group(1)
                if "{" in line:
                    fn_depth = depth + line.count("{") - line.count("}")
                    if fn_depth <= depth:
                        fn_depth = depth + 1
                    fn_stack.append((fn_depth, fn_name))
                else:
                    pending_fn = fn_name
            elif pending_fn and "{" in line:
                fn_depth = depth + line.count("{") - line.count("}")
                if fn_depth <= depth:
                    fn_depth = depth + 1
                fn_stack.append((fn_depth, pending_fn))
                pending_fn = None

            starts_test_mod = bool(MOD_TESTS_RE.search(line)) and "{" in line
            if starts_test_mod:
                test_depth = depth + line.count("{") - line.count("}")
                if test_depth <= depth:
                    test_depth = depth + 1
                test_depths.append(test_depth)

            in_test = bool(test_depths)
            current_fn = fn_stack[-1][1] if fn_stack else None

            if "as u8" in line:
                statement = collect_statement(lines, index)
                category = categorize(rel_path, current_fn, statement, in_test)
                pipeline_candidate = (
                    category == "light_runtime_brightness"
                    and rel_path in RUNTIME_BRIGHTNESS_FILES
                )
                sites.append(
                    CastSite(
                        path=rel_path,
                        line=index + 1,
                        function=current_fn,
                        category=category,
                        pipeline_candidate=pipeline_candidate,
                        test_context=in_test,
                        statement=statement,
                    )
                )

            depth += line.count("{") - line.count("}")
            while fn_stack and depth < fn_stack[-1][0]:
                fn_stack.pop()
            while test_depths and depth < test_depths[-1]:
                test_depths.pop()

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_quantization.py",
        "scan_roots": [path.as_posix() for path in SCAN_ROOTS],
        "summary": summarize(sites),
        "sites": [
            site.to_json()
            for site in sorted(
                sites,
                key=lambda s: (
                    s.path,
                    s.function or "",
                    s.category,
                    s.statement,
                    s.pipeline_candidate,
                    s.test_context,
                ),
            )
        ],
    }


def summarize(sites: list[CastSite]) -> dict:
    categories: dict[str, int] = {}
    for site in sites:
        categories[site.category] = categories.get(site.category, 0) + 1
    return {
        "total_as_u8_sites": len(sites),
        "pipeline_candidate_sites": sum(1 for site in sites if site.pipeline_candidate),
        "test_only_sites": sum(1 for site in sites if site.test_context),
        "categories": dict(sorted(categories.items())),
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

    data = inventory(root)
    rendered = dumps(data)

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

#!/usr/bin/env python3
"""Generate the automation action catalog inventory from Rust.

The source of truth is `ACTION_CATALOG` in `rhythm-core/src/automation.rs`.
This script turns that typed table into a stable JSON artifact for PRD review,
CI drift checks, and importer/API work.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path


INVENTORY_VERSION = 1
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("action_catalog.json")
SOURCE_FILE = Path("rust/core/rhythm-core/src/automation.rs")

TARGET_CONST_RE = re.compile(
    r"const\s+(?P<name>[A-Z_]+):\s*&\[ActionTargetGrain\]\s*=\s*&\[(?P<body>.*?)\];",
    re.MULTILINE | re.DOTALL,
)
WHEN_OFF_CONST_RE = re.compile(
    r"const\s+(?P<name>[A-Z_]+):\s*&\[WhenOffAction\]\s*=\s*&\[(?P<body>.*?)\];",
    re.MULTILINE | re.DOTALL,
)
PARAM_CONST_RE = re.compile(
    r"const\s+(?P<name>[A-Z0-9_]+):\s*&\[ActionParam\]\s*=\s*&\[(?P<body>.*?)\];",
    re.MULTILINE | re.DOTALL,
)
ENTRY_RE = re.compile(r"ActionCatalogEntry\s*\{(?P<body>.*?)\n\s*\}", re.DOTALL)
DYNAMIC_RE = re.compile(r"DynamicActionPattern\s*\{(?P<body>.*?)\n\s*\}", re.DOTALL)

TARGET_RE = re.compile(r"ActionTargetGrain::(?P<value>[A-Za-z]+)")
WHEN_OFF_RE = re.compile(r"WhenOffAction::(?P<value>[A-Za-z]+)")
PARAM_RE = re.compile(
    r"ActionParam\s*\{\s*key:\s*\"(?P<key>[^\"]+)\",\s*value:\s*\"(?P<value>[^\"]+)\",\s*\}",
    re.MULTILINE | re.DOTALL,
)
FIELD_RE = re.compile(r"(?P<name>[A-Za-z_]+):\s*(?P<value>.*?)(?:,\n|,\s*$)", re.DOTALL)


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


def fields(body: str) -> dict[str, str]:
    return {match.group("name"): match.group("value").strip() for match in FIELD_RE.finditer(body)}


def string_value(value: str) -> str:
    match = re.fullmatch(r'"([^"]*)"', value.strip())
    if not match:
        raise RuntimeError(f"expected string literal, got {value!r}")
    return match.group(1)


def option_string(value: str) -> str | None:
    value = value.strip()
    if value == "None":
        return None
    match = re.fullmatch(r'Some\("([^"]+)"\)', value)
    if not match:
        raise RuntimeError(f"expected Option string, got {value!r}")
    return match.group(1)


def enum_value(value: str, prefix: str) -> str:
    match = re.fullmatch(prefix + r"::([A-Za-z]+)", value.strip())
    if not match:
        raise RuntimeError(f"expected {prefix} enum, got {value!r}")
    return snake_case(match.group(1))


def parse_const_enum_arrays(source: str, regex: re.Pattern, enum_regex: re.Pattern) -> dict[str, list[str]]:
    return {
        match.group("name"): [
            snake_case(value.group("value")) for value in enum_regex.finditer(match.group("body"))
        ]
        for match in regex.finditer(source)
    }


def parse_param_consts(source: str) -> dict[str, list[dict]]:
    return {
        match.group("name"): [
            {"key": param.group("key"), "value": param.group("value")}
            for param in PARAM_RE.finditer(match.group("body"))
        ]
        for match in PARAM_CONST_RE.finditer(source)
    }


def parse_array_ref(value: str, consts: dict[str, list], label: str) -> list:
    value = value.strip()
    if value == "&[]":
        return []
    if value in consts:
        return consts[value]
    raise RuntimeError(f"unresolved {label} reference: {value}")


def const_array_block(source: str, marker: str, source_path: Path) -> str:
    try:
        const_start = source.index(marker)
        array_start = source.index("&[", const_start)
        array_end = source.index("];", array_start) + 1
    except ValueError as exc:
        raise RuntimeError(f"could not find {marker} in {source_path}") from exc
    return source[array_start:array_end]


def inventory(root: Path) -> dict:
    source_path = root / SOURCE_FILE
    source = source_path.read_text(encoding="utf-8")
    target_consts = parse_const_enum_arrays(source, TARGET_CONST_RE, TARGET_RE)
    when_off_consts = parse_const_enum_arrays(source, WHEN_OFF_CONST_RE, WHEN_OFF_RE)
    param_consts = parse_param_consts(source)
    action_block = const_array_block(source, "pub const ACTION_CATALOG", source_path)
    dynamic_block = const_array_block(source, "pub const DYNAMIC_ACTION_PATTERNS", source_path)

    entries = []
    seen_ids: set[str | None] = set()
    for match in ENTRY_RE.finditer(action_block):
        item = fields(match.group("body"))
        if "python_id" not in item:
            continue
        python_id = option_string(item["python_id"])
        if python_id in seen_ids:
            raise RuntimeError(f"duplicate action catalog id {python_id!r} in {source_path}")
        seen_ids.add(python_id)

        entries.append(
            {
                "python_action_id": python_id,
                "migrated_id": option_string(item["migrated_id"]),
                "category": string_value(item["category"]),
                "label": string_value(item["label"]),
                "rust_action": enum_value(item["action"], "AutomationActionKind"),
                "parameters": parse_array_ref(item["params"], param_consts, "params"),
                "allowed_target_grains": parse_array_ref(
                    item["allowed_target_grains"], target_consts, "target grains"
                ),
                "supports_when_off": item["supports_when_off"] == "true",
                "allowed_when_off_values": parse_array_ref(
                    item["allowed_when_off"], when_off_consts, "when_off values"
                ),
                "dispatch_precondition": enum_value(
                    item["dispatch_precondition"], "DispatchPrecondition"
                ),
                "import_behavior": enum_value(item["import_behavior"], "ActionImportBehavior"),
            }
        )

    dynamic_patterns = []
    for match in DYNAMIC_RE.finditer(dynamic_block):
        item = fields(match.group("body"))
        if "pattern" not in item:
            continue
        dynamic_patterns.append(
            {
                "pattern": string_value(item["pattern"]),
                "rust_action": enum_value(item["action"], "AutomationActionKind"),
                "param_template": string_value(item["param_template"]),
                "allowed_target_grains": parse_array_ref(
                    item["allowed_target_grains"], target_consts, "target grains"
                ),
                "dispatch_precondition": enum_value(
                    item["dispatch_precondition"], "DispatchPrecondition"
                ),
                "import_behavior": enum_value(item["import_behavior"], "ActionImportBehavior"),
            }
        )

    if not entries:
        raise RuntimeError(f"no ActionCatalogEntry entries found in {source_path}")

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_action_catalog.py",
        "source": SOURCE_FILE.as_posix(),
        "summary": summarize(entries, dynamic_patterns),
        "actions": entries,
        "dynamic_patterns": dynamic_patterns,
    }


def summarize(entries: list[dict], dynamic_patterns: list[dict]) -> dict:
    import_behaviors: dict[str, int] = {}
    rust_actions: dict[str, int] = {}
    for entry in entries:
        import_behaviors[entry["import_behavior"]] = import_behaviors.get(entry["import_behavior"], 0) + 1
        rust_actions[entry["rust_action"]] = rust_actions.get(entry["rust_action"], 0) + 1

    return {
        "static_action_count": len(entries),
        "dynamic_pattern_count": len(dynamic_patterns),
        "when_off_action_count": sum(1 for entry in entries if entry["supports_when_off"]),
        "alias_count": sum(1 for entry in entries if entry["migrated_id"] is not None),
        "import_behaviors": dict(sorted(import_behaviors.items())),
        "rust_actions": dict(sorted(rust_actions.items())),
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

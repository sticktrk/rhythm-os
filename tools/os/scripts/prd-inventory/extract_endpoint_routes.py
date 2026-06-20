#!/usr/bin/env python3
"""Generate the shared endpoint route inventory from Rust.

The source of truth is `SHARED_API_ROUTES` in `rhythm-os/src/routes.rs`.
This script turns that typed table into a stable JSON artifact for PRD review,
CI drift checks, and platform parity work.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path


INVENTORY_VERSION = 1
DEFAULT_OUTPUT = Path(__file__).resolve().with_name("endpoint_routes.json")
SOURCE_FILE = Path("rust/core/rhythm-os/src/routes.rs")

ROUTE_RE = re.compile(
    r"SharedRoute\s*\{\s*"
    r'path:\s*"(?P<path>[^"]+)",\s*'
    r"methods:\s*&\[(?P<methods>[^\]]*)\],\s*"
    r"\}",
    re.MULTILINE,
)
METHOD_RE = re.compile(r'"(?P<method>[A-Z]+)"')


def shared_routes_block(source: str, source_path: Path) -> str:
    marker = "pub const SHARED_API_ROUTES"
    try:
        const_start = source.index(marker)
        array_start = source.index("&[", const_start)
        array_end = source.index("\n];", array_start)
    except ValueError as exc:
        raise RuntimeError(f"could not find SHARED_API_ROUTES in {source_path}") from exc

    return source[array_start:array_end]


def excluded_route_notes(source: str) -> list[str]:
    notes: list[str] = []
    collecting = False

    for line in source.splitlines():
        stripped = line.strip()
        if stripped == "/// Does NOT include:":
            collecting = True
            continue
        if not collecting:
            continue
        if not stripped.startswith("///"):
            break

        content = stripped.removeprefix("///").strip()
        if not content:
            continue
        if content.startswith("- "):
            notes.append(content.removeprefix("- ").strip())
            continue
        if notes:
            notes[-1] = f"{notes[-1]} {content}"

    return notes


def parse_routes(source: str, source_path: Path) -> list[dict]:
    block = shared_routes_block(source, source_path)
    routes: list[dict] = []
    seen_bindings: set[tuple[str, str]] = set()

    for match in ROUTE_RE.finditer(block):
        path = match.group("path")
        methods = [method.group("method") for method in METHOD_RE.finditer(match.group("methods"))]
        if not methods:
            raise RuntimeError(f"route {path!r} has no methods in {source_path}")

        for method in methods:
            binding = (path, method)
            if binding in seen_bindings:
                raise RuntimeError(f"duplicate route binding {method} {path} in {source_path}")
            seen_bindings.add(binding)

        routes.append(
            {
                "path": path,
                "methods": methods,
            }
        )

    if not routes:
        raise RuntimeError(f"no SharedRoute entries found in {source_path}")

    return routes


def summarize(routes: list[dict]) -> dict:
    method_counts: dict[str, int] = {}
    for route in routes:
        for method in route["methods"]:
            method_counts[method] = method_counts.get(method, 0) + 1

    return {
        "route_entries": len(routes),
        "method_bindings": sum(len(route["methods"]) for route in routes),
        "parameterized_routes": sum(1 for route in routes if ":" in route["path"]),
        "methods": dict(sorted(method_counts.items())),
    }


def inventory(root: Path) -> dict:
    source_path = root / SOURCE_FILE
    source = source_path.read_text(encoding="utf-8")
    routes = parse_routes(source, source_path)

    return {
        "version": INVENTORY_VERSION,
        "generated_by": "tools/os/scripts/prd-inventory/extract_endpoint_routes.py",
        "source": SOURCE_FILE.as_posix(),
        "excluded_transport_or_platform_routes": excluded_route_notes(source),
        "summary": summarize(routes),
        "routes": routes,
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

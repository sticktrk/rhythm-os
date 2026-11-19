#!/usr/bin/env python3
"""Check the assistant coverage ledger against the actual Rust HTTP routers."""
import csv
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "docs/architecture/assistant-route-coverage.tsv"
VERBS = {"get", "put", "patch", "post", "delete", "head", "options"}
TOKEN = re.compile(r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|\w+|[^\s]')
PAIRS = {"(": ")", "{": "}", "[": "]"}


def routes(root=ROOT):
    constants = {}
    files = list((root / "os/rust").rglob("src/**/*.rs"))
    for file in files:
        constants.update(re.findall(r'const\s+(\w+):\s*&str\s*=\s*"(/[^"]*)"', file.read_text()))
    found = set()
    for file in files:
        if file.name == "tests.rs" or "tests" in file.parts:
            continue
        source = file.read_text().split("#[cfg(test)]\nmod tests")[0]
        tokens = [t for t in TOKEN.findall(source) if not t.startswith(("//", "/*"))]
        for start in range(len(tokens) - 3):
            if (tokens[start] == "." and tokens[start + 2] == "("
                and tokens[start + 1] in {"nest", "nest_service", "route_service", "fallback", "fallback_service", "method_not_allowed_fallback"}):
                raise ValueError(f"Classify the router registration {tokens[start + 1]} in {file}")
            if tokens[start:start+3] != [".", "route", "("]:
                continue
            scoped = next(i for i in range(start+3, len(tokens)) if tokens[i] == ",")
            name = tokens[scoped-1]
            path = name.strip('"') if name.startswith('"') else constants.get(name)
            if not path or not path.startswith("/"):
                raise ValueError(f"Unresolved route at {file}: {name}")
            stack = []
            verbs = set()
            for i in range(scoped + 1, len(tokens)):
                t = tokens[i]
                if not stack and t == ")":
                    break
                if not stack and t in VERBS and tokens[i+1] == "(":
                    verbs.add(t.upper())
                if t in PAIRS:
                    stack.append(PAIRS[t])
                elif stack and t == stack[-1]:
                    stack.pop()
            if not verbs:
                raise ValueError(f"Unresolved method for {path} in {file}")
            surface = "shared" if "rhythm-os" in file.relative_to(root).parts else file.relative_to(root).parts[3]
            found.update((surface, verb, path) for verb in verbs)
    return found


def check_new_routes():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        source = root / "os/rust/bins/new-hub/src/router.rs"
        source.parent.mkdir(parents=True)
        source.write_text('Router::new().\nroute("/example", get(read).post(write))')
        if routes(root) != {("new-hub", "GET", "/example"), ("new-hub", "POST", "/example")}:
            raise ValueError("Coverage scanner did not detect a new route and both methods")
        source.write_text('Router::new().nest_service("/example", service)')
        try:
            routes(root)
        except ValueError as error:
            if "nest_service" in str(error):
                return
            raise
        raise ValueError("Coverage scanner silently skipped a nested service")


def main():
    check_new_routes()
    actual = routes()
    with LEDGER.open() as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    expected = set()
    for row in rows:
        key = (row["surface"], row["method"], row["path"])
        if key in expected or row["classification"] not in {"operation", "handoff", "exclusion"} or not row["reason_or_operation"].strip():
            sys.exit(f"Invalid or duplicate assistant coverage row: {key}")
        expected.add(key)
    if actual != expected:
        sys.exit(f"Unclassified routes: {sorted(actual-expected)}\nObsolete rows: {sorted(expected-actual)}")
    print(f"{len(expected)} hub HTTP routes classified.")


if __name__ == "__main__":
    main()

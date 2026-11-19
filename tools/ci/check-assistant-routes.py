#!/usr/bin/env python3
"""Check the assistant coverage ledger against the actual Rust HTTP routers."""
import csv
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "docs/architecture/assistant-route-coverage.tsv"
VERBS = {"get", "put", "patch", "post", "delete", "head", "options"}
TOKEN = re.compile(r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|\w+|[^\s]')
PAIRS = {"(": ")", "{": "}", "[": "]"}


def routes():
    constants = {}
    files = list((ROOT / "os/rust").rglob("src/**/*.rs"))
    for file in files:
        constants.update(re.findall(r'const\s+(\w+):\s*&str\s*=\s*"(/[^"]*)"', file.read_text()))
    found = set()
    for file in files:
        if file.name == "tests.rs" or "tests" in file.parts:
            continue
        source = file.read_text().split("#[cfg(test)]\nmod tests")[0]
        if ".route" not in source:
            continue
        tokens = [t for t in TOKEN.findall(source) if not t.startswith(("//", "/*"))]
        for start in range(len(tokens) - 4):
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
            surface = "shared" if "rhythm-os" in file.relative_to(ROOT).parts else file.relative_to(ROOT).parts[3]
            found.update((surface, verb, path) for verb in verbs)
    return found


def main():
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

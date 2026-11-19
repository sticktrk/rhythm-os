#!/usr/bin/env python3
"""Compare cloud assistant classifications with the executable cloud surfaces."""

import csv
import re
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "docs/architecture/cloud-assistant-coverage.tsv"
VERBS = {"get", "post", "put", "patch", "delete", "head", "options", "all"}
TOKEN = re.compile(
    r"""//[^\n]*|/\*[\s\S]*?\*/|'''[\s\S]*?'''|\"\"\"[\s\S]*?\"\"\"|"""
    r"""'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|\.\.|\w+|[^\s]"""
)


def tokens(source):
    return [t for t in TOKEN.findall(source) if not t.startswith(("//", "/*"))]


def literal(token):
    if token.startswith(("'", '"')) and "$" not in token and "\\" not in token:
        return token[1:-1]
    return None


def staff_routes(source):
    ts = tokens(source)
    # Recognize both a constructor cascade and later references to the named
    # router. Keep outgoing HTTP client calls separate from route declarations.
    routers = {
        ts[i - 2]
        for i in range(2, len(ts) - 2)
        if ts[i - 1 : i + 3] == ["=", "Router", "(", ")"]
    }
    routers.update(
        ts[i + 1]
        for i, t in enumerate(ts[:-1])
        if t == "Router" and ts[i + 1].isidentifier()
    )
    found = set()
    for i, t in enumerate(ts):
        if ts[i : i + 2] == ["@", "Route"]:
            raise ValueError("Extend the coverage scanner for an annotated staff route")
        if t != "Router" and t not in routers:
            continue
        j = i + (3 if ts[i : i + 3] == ["Router", "(", ")"] else 1)
        while ts[j : j + 1] in (["."], [".."]):
            method = ts[j + 1]
            if ts[j + 2 : j + 3] != ["("]:
                break
            pos = j + 3
            if method == "add":
                verb = literal(ts[pos])
                if not verb or ts[pos + 1] != "," or verb.lower() not in VERBS:
                    raise ValueError("Unresolved staff HTTP verb")
                method, pos = verb.lower(), pos + 2
            if method in VERBS:
                path = literal(ts[pos])
                if not path or not path.startswith("/") or ts[pos + 1] != ",":
                    raise ValueError(f"Unresolved staff route following {method}")
                found.add(("staff", method.upper(), path))
            elif method != "call":
                raise ValueError(f"Extend the coverage scanner for Router.{method}")
            depth = 1
            j += 3
            while depth and j < len(ts):
                depth += (ts[j] == "(") - (ts[j] == ")")
                j += 1
    return found


def app_rpcs(source):
    ts = tokens(source)
    found = set()
    constants = {}
    for i, t in enumerate(ts[:-3]):
        if t != "const":
            continue
        j = i + 2 if ts[i + 1] == "String" else i + 1
        if ts[j].isidentifier() and ts[j + 1] == "=" and literal(ts[j + 2]):
            constants.setdefault(ts[j], set()).add(literal(ts[j + 2]))
    for i, t in enumerate(ts):
        if t.startswith(("'", '"')) and "/rpc/" in t:
            name = re.search(r"/rpc/([a-z][a-z0-9_]*)(?:[?'\"/]|$)", t)
            if not name:
                raise ValueError("Unresolved PostgREST RPC path in app source")
            found.add(("app_rpc", "CALL", name[1]))
        if t != "rpc" or ts[i - 1] not in {".", ".."}:
            continue
        j = i + 1
        if ts[j] == "<":  # Supabase supports an optional explicit result type.
            depth = 1
            j += 1
            while depth and j < len(ts):
                depth += (ts[j] == "<") - (ts[j] == ">")
                j += 1
        if ts[j : j + 1] != ["("]:
            raise ValueError("Unresolved reference to app RPC method")
        arg = ts[j + 1]
        candidates = constants.get(arg, set())
        name = literal(arg) or (
            next(iter(candidates)) if len(candidates) == 1 else None
        )
        if (
            not name
            or not re.fullmatch(r"[a-z][a-z0-9_]*", name)
            or ts[j + 2] not in {")", ","}
        ):
            raise ValueError(
                "Use a statically discoverable name for an app database function"
            )
        found.add(("app_rpc", "CALL", name))
    return found


def inventory(root):
    actual = set()
    for file in (root / "admin-api").rglob("*.dart"):
        if (
            "test" not in file.parts
            and "build" not in file.parts
            and ".dart_tool" not in file.parts
        ):
            try:
                actual.update(staff_routes(file.read_text()))
            except ValueError as error:
                raise ValueError(f"{file}: {error}") from error
    supabase = root / "tools/app/supabase"
    functions = set(
        tomllib.loads((supabase / "config.toml").read_text()).get("functions", {})
    )
    # The deploy command accepts each public function directory. Configured
    # custom entrypoints are also functions, even outside the normal tree.
    functions.update(
        path.name
        for path in (supabase / "functions").iterdir()
        if path.is_dir() and not path.name.startswith(("_", "."))
    )
    actual.update(("edge", "INVOKE", name) for name in functions)
    roots = (*sorted((root / "app/flutter").glob("*/lib")), root / "sdk/lib")
    for path in roots:
        for file in path.rglob("*.dart"):
            try:
                actual.update(app_rpcs(file.read_text()))
            except ValueError as error:
                raise ValueError(f"{file}: {error}") from error
    return actual


def operation_ids(root):
    resident = root / "tools/app/supabase/functions/customer-assistant/contract.ts"
    staff = root / "admin-api/lib/src/staff_assistant.dart"
    return {
        match[1]
        for path, constructor in [(resident, "descriptor"), (staff, "_Tool")]
        for match in re.finditer(
            r"\b" + constructor + r"\(\s*['\"]([a-z_]+)['\"]", path.read_text()
        )
    }


def verify(actual, ledger, operations):
    expected = set()
    with ledger.open() as file:
        for row in csv.DictReader(file, delimiter="\t"):
            key = row["surface"], row["method"], row["path"]
            kind, reason = row["classification"], row["reason_or_operation"].strip()
            if (
                key in expected
                or kind not in {"operation", "handoff", "exclusion"}
                or not reason
            ):
                raise ValueError(f"Duplicate or invalid cloud coverage row: {key}")
            if kind == "operation" and reason.split()[0] not in operations:
                raise ValueError(f"Unknown cloud operation in {key}: {reason}")
            expected.add(key)
    if actual != expected:
        raise ValueError(
            f"Unclassified cloud surfaces: {sorted(actual - expected)}\nObsolete classifications: {sorted(expected - actual)}"
        )


def verify_new_surfaces_fail():
    # Exercise discovery and comparison together. Each added surface has to
    # fail the same comparison as a newly deployed production capability.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        staff = root / "admin-api/lib"
        app = root / "app/flutter/rhythm_app/lib"
        supabase = root / "tools/app/supabase"
        (supabase / "functions/_shared").mkdir(parents=True)
        staff.mkdir(parents=True)
        app.mkdir(parents=True)
        (supabase / "config.toml").write_text("")
        ledger = root / "ledger.tsv"
        ledger.write_text(
            "surface\tmethod\tpath\tclassification\treason_or_operation\n"
        )
        verify(inventory(root), ledger, set())
        (staff / "server.dart").write_text(
            "final routes = Router()..get('/new', handler); routes.add('PATCH','/new', handler);"
        )
        (supabase / "functions/new-edge").mkdir()
        (supabase / "config.toml").write_text(
            '[functions.custom_entry]\nentrypoint = "./nested.ts"'
        )
        (app / "database.dart").write_text(
            "await client.rpc<List<Map<String,dynamic>>>('new_rpc');"
        )
        core = root / "app/flutter/rhythm_core/lib"
        core.mkdir(parents=True)
        (core / "account.dart").write_text("await client.rpc('shared_account_rpc');")
        actual = inventory(root)
        wanted = {
            ("staff", "GET", "/new"),
            ("staff", "PATCH", "/new"),
            ("edge", "INVOKE", "new-edge"),
            ("edge", "INVOKE", "custom_entry"),
            ("app_rpc", "CALL", "new_rpc"),
            ("app_rpc", "CALL", "shared_account_rpc"),
        }
        if actual != wanted:
            raise ValueError(f"Scanner missed fixture surfaces: {wanted - actual}")
        for entry in actual:
            try:
                verify({entry}, ledger, set())
            except ValueError as error:
                if "Unclassified cloud surfaces" not in str(error):
                    raise
            else:
                raise ValueError(f"Unclassified fixture surface passed: {entry}")


def main():
    actual = inventory(ROOT)
    verify(actual, LEDGER, operation_ids(ROOT))
    verify_new_surfaces_fail()
    print(f"{len(actual)} cloud surfaces classified.")


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        sys.exit(str(error))

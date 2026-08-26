#!/usr/bin/env python3
"""Materialize the validated app-build profile for one local Flutter process."""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

from rhythm_env import (
    ConfigError,
    load_profile,
    load_registry,
    resolve_profile_path,
)


REPO_ROOT = Path(__file__).resolve().parents[2]


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def main() -> int:
    temporary: Path | None = None
    descriptor = -1
    try:
        registry = load_registry()
        profile = registry.profiles["app-build"]
        source = resolve_profile_path(registry, profile)
        if _is_within(source, REPO_ROOT):
            raise ConfigError(
                "External app-build profile must be outside the repository."
            )
        values = load_profile("app-build", registry=registry)
        descriptor, temporary_name = tempfile.mkstemp(
            prefix=f".{source.name}.build.",
            suffix=".json",
            dir=source.parent,
        )
        temporary = Path(temporary_name)
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            descriptor = -1
            json.dump(dict(sorted(values.items())), handle, ensure_ascii=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
    except ConfigError as error:
        for message in error.messages:
            print(f"app-build materialization failed: {message}", file=sys.stderr)
        return 1
    except OSError:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
        print(
            "app-build materialization failed: private temporary file could not be created",
            file=sys.stderr,
        )
        return 1
    finally:
        if descriptor >= 0:
            os.close(descriptor)

    print(temporary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

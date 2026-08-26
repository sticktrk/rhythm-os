#!/usr/bin/env python3
"""Validate Rhythm credential profiles without printing paths or values."""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from rhythm_env import (
    ConfigError,
    load_registry,
    profiles_for_consumer,
    validate_profile,
    validate_registry_examples,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        help="validate one named profile; repeat to validate more than one",
    )
    parser.add_argument(
        "--consumer",
        action="append",
        default=[],
        help="validate every profile assigned to a named consumer",
    )
    parser.add_argument(
        "--schema-only",
        action="store_true",
        help="validate the registry and checked-in examples without reading live files",
    )
    parser.add_argument(
        "--config-dir",
        type=Path,
        help="override the configuration directory for this validation",
    )
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path(__file__).with_name("profiles.toml"),
        help=argparse.SUPPRESS,
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        registry = load_registry(args.registry)
        validate_registry_examples(registry, registry_dir=args.registry.parent)
    except ConfigError as error:
        for message in error.messages:
            print(f"credential profile schema: invalid: {message}", file=sys.stderr)
        return 1

    if args.schema_only:
        print(f"credential profile schema: valid ({len(registry.profiles)} profiles)")
        return 0

    selected: list[str] = []
    try:
        for consumer in args.consumer:
            selected.extend(profiles_for_consumer(consumer, registry=registry))
    except ConfigError as error:
        for message in error.messages:
            print(f"credential profile selection: invalid: {message}", file=sys.stderr)
        return 1
    selected.extend(args.profile)
    if not selected:
        selected.extend(registry.profiles)
    selected = list(dict.fromkeys(selected))

    environment = dict(os.environ)
    if args.config_dir is not None:
        environment[registry.config_dir_env] = str(args.config_dir)

    failed = False
    for name in selected:
        result = validate_profile(name, registry=registry, environment=environment)
        if result.ok:
            print(f"{name}: ready")
            continue
        failed = True
        for message in result.errors:
            print(f"{name}: invalid: {message}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

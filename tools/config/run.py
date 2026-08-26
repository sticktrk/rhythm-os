#!/usr/bin/env python3
"""Run a command with one or more validated Rhythm credential profiles."""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from rhythm_env import (
    ConfigError,
    load_profiles,
    load_registry,
    profiles_for_consumer,
    validate_registry_examples,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        help="load one named profile; repeat to load more than one",
    )
    parser.add_argument(
        "--consumer",
        action="append",
        default=[],
        help="load every profile assigned to a named consumer",
    )
    parser.add_argument(
        "--config-dir",
        type=Path,
        help="override the configuration directory for this command",
    )
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path(__file__).with_name("profiles.toml"),
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="command and arguments to execute",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        print("credential loading failed: no command was provided", file=sys.stderr)
        return 2

    try:
        registry = load_registry(args.registry)
        validate_registry_examples(registry, registry_dir=args.registry.parent)
        selected: list[str] = []
        for consumer in args.consumer:
            selected.extend(profiles_for_consumer(consumer, registry=registry))
        selected.extend(args.profile)
        selected = list(dict.fromkeys(selected))
        if not selected:
            raise ConfigError("At least one credential profile or consumer is required.")

        environment = dict(os.environ)
        if args.config_dir is not None:
            environment[registry.config_dir_env] = str(args.config_dir)
        environment.update(
            load_profiles(selected, registry=registry, environment=environment)
        )
    except ConfigError as error:
        for message in error.messages:
            print(f"credential loading failed: {message}", file=sys.stderr)
        return 1

    try:
        os.execvpe(command[0], command, environment)
    except OSError:
        print("credential loading failed: command could not be started", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

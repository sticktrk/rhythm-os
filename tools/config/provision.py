#!/usr/bin/env python3
"""Provision external Rhythm credential profiles from legacy local files."""

from __future__ import annotations

import argparse
import os
import stat
import sys
import tempfile
from pathlib import Path

from rhythm_env import (
    ConfigError,
    Profile,
    load_profile,
    load_registry,
    read_legacy_env_file,
    resolve_config_dir,
    validate_profile_values,
    validate_registry_examples,
)


LEGACY_SOURCES = {
    "staff": (
        "app/flutter/rhythm_app/.env",
        "admin-api/.env",
    ),
    "analytics": (
        "app/flutter/rhythm_app/.env",
        "admin-api/.env",
    ),
    "admin-api": (
        "app/flutter/rhythm_app/.env",
        "admin-api/.env",
    ),
    "app-build": ("app/flutter/rhythm_app/.env",),
    # Listed from lowest to highest precedence to preserve release.sh's
    # current os/.env -> root .env -> admin-api/.env search order.
    "release": (
        "admin-api/.env",
        ".env",
        "os/.env",
    ),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        help="provision one profile; repeat to provision more than one",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="provision every registered profile",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="report whether profiles can be provisioned without writing files",
    )
    parser.add_argument(
        "--legacy-root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--config-dir",
        type=Path,
        help="override the external configuration directory",
    )
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path(__file__).with_name("profiles.toml"),
        help=argparse.SUPPRESS,
    )
    return parser.parse_args()


def _validate_directory(path: Path, *, create: bool) -> None:
    if create:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        directory_stat = path.lstat()
    except OSError as error:
        raise ConfigError("External credential directory is unavailable.") from error
    if stat.S_ISLNK(directory_stat.st_mode) or not stat.S_ISDIR(
        directory_stat.st_mode
    ):
        raise ConfigError("External credential directory must be a real directory.")
    if hasattr(os, "getuid") and directory_stat.st_uid != os.getuid():
        raise ConfigError("External credential directory must be owned by the current user.")
    if create:
        path.chmod(0o700)


def _load_legacy_values(
    profile: Profile, legacy_root: Path
) -> tuple[dict[str, str], bool]:
    values: dict[str, str] = {}
    unsafe_source = False
    for relative_source in LEGACY_SOURCES.get(profile.name, ()):
        source = legacy_root / relative_source
        if not os.path.lexists(source):
            continue
        try:
            source_values = read_legacy_env_file(source)
        except ConfigError:
            unsafe_source = True
            continue
        for key in profile.allowed_keys:
            value = source_values.get(key, "").strip()
            if value:
                values[key] = value

    if profile.name == "staff" and all(
        values.get(key, "").strip()
        for key in ("RHYTHM_ADMIN_EMAIL", "RHYTHM_ADMIN_PASSWORD")
    ):
        values.pop("RHYTHM_ADMIN_ACCESS_TOKEN", None)
    return values, unsafe_source


def _serialize(profile: Profile, values: dict[str, str]) -> str:
    ordered = (
        *profile.required,
        *(key for group in profile.one_of for key in group),
        *profile.optional,
    )
    lines: list[str] = []
    for key in ordered:
        if key not in values:
            continue
        value = values[key]
        if "\n" in value or "\r" in value or "\x00" in value:
            raise ConfigError(f"Credential value for {key} cannot be serialized safely.")
        lines.append(f"{key}='{value}'")
    return "\n".join(lines) + "\n"


def _write_new_profile(path: Path, content: str) -> None:
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            descriptor = -1
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary, path)
    except FileExistsError as error:
        raise ConfigError("External credential profile already exists.") from error
    except OSError as error:
        raise ConfigError("External credential profile could not be created safely.") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    args = parse_args()
    try:
        registry = load_registry(args.registry)
        validate_registry_examples(registry, registry_dir=args.registry.parent)
        selected = list(registry.profiles) if args.all else list(args.profile)
        selected = list(dict.fromkeys(selected))
        if not selected:
            raise ConfigError("At least one credential profile is required.")
        unknown = sorted(set(selected) - set(registry.profiles))
        if unknown:
            raise ConfigError("Unknown credential profiles: " + ", ".join(unknown) + ".")

        config_environment: dict[str, str] = {}
        if args.config_dir is not None:
            config_environment[registry.config_dir_env] = str(args.config_dir)
        config_dir = resolve_config_dir(registry, config_environment)
        legacy_root = args.legacy_root.expanduser()
        if not legacy_root.is_absolute():
            raise ConfigError("Legacy configuration root must be an absolute path.")
        if config_dir.exists():
            _validate_directory(config_dir, create=not args.check)
        elif not args.check:
            _validate_directory(config_dir, create=True)
    except ConfigError as error:
        for message in error.messages:
            print(f"credential provisioning failed: {message}", file=sys.stderr)
        return 1

    failed = False
    for name in selected:
        profile = registry.profiles[name]
        target = config_dir / profile.filename
        if target.exists():
            result_environment = {registry.config_dir_env: str(config_dir)}
            try:
                load_profile(name, registry=registry, environment=result_environment)
                print(f"{name}: already ready")
            except ConfigError as error:
                failed = True
                for message in error.messages:
                    print(f"{name}: existing profile invalid: {message}", file=sys.stderr)
            continue

        values, unsafe_source = _load_legacy_values(profile, legacy_root)
        errors = validate_profile_values(profile, values)
        if unsafe_source:
            errors = (*errors, "a legacy source could not be read safely")
        if errors:
            failed = True
            for message in errors:
                print(f"{name}: not provisioned: {message}", file=sys.stderr)
            continue
        if args.check:
            print(f"{name}: ready to provision")
            continue
        created = False
        try:
            _validate_directory(config_dir, create=True)
            _write_new_profile(target, _serialize(profile, values))
            created = True
            result_environment = {registry.config_dir_env: str(config_dir)}
            load_profile(name, registry=registry, environment=result_environment)
        except ConfigError as error:
            if created:
                try:
                    target.unlink()
                except FileNotFoundError:
                    pass
            failed = True
            for message in error.messages:
                print(f"{name}: provisioning failed: {message}", file=sys.stderr)
            continue
        print(f"{name}: provisioned")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Load explicit Rhythm credential profiles without exposing their values."""

from __future__ import annotations

import os
import re
import stat
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path


REGISTRY_PATH = Path(__file__).with_name("profiles.toml")
SUPPORTED_SCHEMA_VERSION = 1
ENV_NAME_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")
PROFILE_NAME_RE = re.compile(r"^[a-z][a-z0-9-]*$")


class ConfigError(RuntimeError):
    """A privacy-safe local credential configuration failure."""

    def __init__(self, *messages: str):
        cleaned = tuple(message.strip() for message in messages if message.strip())
        self.messages = cleaned or ("Credential configuration is invalid.",)
        super().__init__("; ".join(self.messages))


@dataclass(frozen=True)
class Profile:
    name: str
    description: str
    filename: str
    file_env: str
    example: str
    required: tuple[str, ...]
    one_of: tuple[tuple[str, ...], ...]
    optional: tuple[str, ...]

    @property
    def allowed_keys(self) -> frozenset[str]:
        alternatives = (name for group in self.one_of for name in group)
        return frozenset((*self.required, *alternatives, *self.optional))


@dataclass(frozen=True)
class Registry:
    config_dir_env: str
    default_config_dir: str
    profiles: Mapping[str, Profile]
    consumers: Mapping[str, tuple[str, ...]]


@dataclass(frozen=True)
class ValidationResult:
    profile: str
    errors: tuple[str, ...]

    @property
    def ok(self) -> bool:
        return not self.errors


def _string(value: object, *, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ConfigError(f"Credential profile field {field} must be a string.")
    return value.strip()


def _env_names(value: object, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ConfigError(f"Credential profile field {field} must be a string list.")
    names = tuple(item.strip() for item in value)
    if any(not ENV_NAME_RE.fullmatch(name) for name in names):
        raise ConfigError(f"Credential profile field {field} contains an invalid key name.")
    if len(names) != len(set(names)):
        raise ConfigError(f"Credential profile field {field} contains duplicate key names.")
    return names


def _alternatives(value: object, *, field: str) -> tuple[tuple[str, ...], ...]:
    if not isinstance(value, list):
        raise ConfigError(f"Credential profile field {field} must be a list.")
    groups = tuple(_env_names(group, field=field) for group in value)
    if any(not group for group in groups):
        raise ConfigError(f"Credential profile field {field} contains an empty alternative.")
    return groups


def _simple_filename(value: object, *, field: str) -> str:
    filename = _string(value, field=field)
    if Path(filename).name != filename or filename in {".", ".."}:
        raise ConfigError(f"Credential profile field {field} must be a simple filename.")
    return filename


def load_registry(path: Path = REGISTRY_PATH) -> Registry:
    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except OSError as error:
        raise ConfigError("Credential profile registry is unavailable.") from error
    except tomllib.TOMLDecodeError as error:
        raise ConfigError("Credential profile registry is invalid TOML.") from error

    allowed_top_level = {
        "schema_version",
        "config_dir_env",
        "default_config_dir",
        "profiles",
        "consumers",
    }
    unknown_top_level = sorted(set(document) - allowed_top_level)
    if unknown_top_level:
        raise ConfigError(
            "Credential profile registry contains unsupported fields: "
            + ", ".join(unknown_top_level)
        )
    schema_version = document.get("schema_version")
    if (
        not isinstance(schema_version, int)
        or isinstance(schema_version, bool)
        or schema_version != SUPPORTED_SCHEMA_VERSION
    ):
        raise ConfigError("Credential profile registry has an unsupported schema version.")

    config_dir_env = _string(document.get("config_dir_env"), field="config_dir_env")
    if not ENV_NAME_RE.fullmatch(config_dir_env):
        raise ConfigError("Credential profile config_dir_env is invalid.")
    default_config_dir = _string(
        document.get("default_config_dir"), field="default_config_dir"
    )
    expanded_default = Path(default_config_dir).expanduser()
    if not expanded_default.is_absolute():
        raise ConfigError("Credential profile default_config_dir must be absolute.")

    raw_profiles = document.get("profiles")
    if not isinstance(raw_profiles, dict) or not raw_profiles:
        raise ConfigError("Credential profile registry must define profiles.")
    profiles: dict[str, Profile] = {}
    profile_fields = {
        "description",
        "file",
        "file_env",
        "example",
        "required",
        "one_of",
        "optional",
    }
    for name, raw_profile in raw_profiles.items():
        if not isinstance(name, str) or not PROFILE_NAME_RE.fullmatch(name):
            raise ConfigError("Credential profile registry contains an invalid profile name.")
        if not isinstance(raw_profile, dict):
            raise ConfigError(f"Credential profile {name} must be a table.")
        unknown_fields = sorted(set(raw_profile) - profile_fields)
        if unknown_fields:
            raise ConfigError(
                f"Credential profile {name} contains unsupported fields: "
                + ", ".join(unknown_fields)
            )
        required = _env_names(raw_profile.get("required"), field=f"{name}.required")
        one_of = _alternatives(raw_profile.get("one_of"), field=f"{name}.one_of")
        optional = _env_names(raw_profile.get("optional"), field=f"{name}.optional")
        all_names = (*required, *(key for group in one_of for key in group), *optional)
        if len(all_names) != len(set(all_names)):
            raise ConfigError(f"Credential profile {name} assigns a key more than once.")
        file_env = _string(raw_profile.get("file_env"), field=f"{name}.file_env")
        if not ENV_NAME_RE.fullmatch(file_env):
            raise ConfigError(f"Credential profile {name} has an invalid file override key.")
        profiles[name] = Profile(
            name=name,
            description=_string(
                raw_profile.get("description"), field=f"{name}.description"
            ),
            filename=_simple_filename(raw_profile.get("file"), field=f"{name}.file"),
            file_env=file_env,
            example=_simple_filename(
                raw_profile.get("example"), field=f"{name}.example"
            ),
            required=required,
            one_of=one_of,
            optional=optional,
        )

    raw_consumers = document.get("consumers")
    if not isinstance(raw_consumers, dict):
        raise ConfigError("Credential profile registry must define consumers.")
    consumers: dict[str, tuple[str, ...]] = {}
    for name, raw_profile_names in raw_consumers.items():
        if not isinstance(name, str) or not PROFILE_NAME_RE.fullmatch(name):
            raise ConfigError("Credential profile registry contains an invalid consumer name.")
        if not isinstance(raw_profile_names, list) or any(
            not isinstance(item, str) for item in raw_profile_names
        ):
            raise ConfigError(f"Credential consumer {name} must contain a profile list.")
        profile_names = tuple(item.strip() for item in raw_profile_names)
        if not profile_names or len(profile_names) != len(set(profile_names)):
            raise ConfigError(f"Credential consumer {name} has an invalid profile list.")
        missing_profiles = sorted(set(profile_names) - set(profiles))
        if missing_profiles:
            raise ConfigError(
                f"Credential consumer {name} references unknown profiles: "
                + ", ".join(missing_profiles)
            )
        consumers[name] = profile_names

    return Registry(
        config_dir_env=config_dir_env,
        default_config_dir=default_config_dir,
        profiles=profiles,
        consumers=consumers,
    )


def _absolute_path(value: str, *, label: str) -> Path:
    path = Path(value).expanduser()
    if not path.is_absolute():
        raise ConfigError(f"{label} must be an absolute path.")
    return path


def resolve_config_dir(
    registry: Registry, environment: Mapping[str, str] | None = None
) -> Path:
    environment = os.environ if environment is None else environment
    configured = environment.get(registry.config_dir_env, "").strip()
    return _absolute_path(
        configured or registry.default_config_dir,
        label="Credential configuration directory",
    )


def resolve_profile_path(
    registry: Registry,
    profile: Profile,
    environment: Mapping[str, str] | None = None,
) -> Path:
    environment = os.environ if environment is None else environment
    override = environment.get(profile.file_env, "").strip()
    if override:
        return _absolute_path(override, label=f"Credential profile {profile.name} file")
    return resolve_config_dir(registry, environment) / profile.filename


def _parse_env_text(text: str) -> dict[str, str]:
    if "\x00" in text:
        raise ConfigError("Credential file contains an invalid null byte.")
    values: dict[str, str] = {}
    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[len("export ") :].strip()
        key, separator, raw_value = line.partition("=")
        key = key.strip()
        if not separator:
            raise ConfigError(f"Credential file line {line_number} is malformed.")
        if not ENV_NAME_RE.fullmatch(key):
            raise ConfigError(f"Credential file line {line_number} has an invalid key name.")
        if key in values:
            raise ConfigError(f"Credential file defines {key} more than once.")
        value = raw_value.strip()
        if value[:1] in {"\"", "'"}:
            if len(value) < 2 or value[-1] != value[0]:
                raise ConfigError(f"Credential file value for {key} has unmatched quotes.")
            value = value[1:-1]
        values[key] = value
    return values


def _read_profile_file(
    path: Path, *, require_private_permissions: bool = True
) -> dict[str, str]:
    try:
        file_stat = path.lstat()
    except OSError as error:
        raise ConfigError("Credential profile file is unavailable.") from error
    if stat.S_ISLNK(file_stat.st_mode):
        raise ConfigError("Credential profile file must not be a symbolic link.")
    if not stat.S_ISREG(file_stat.st_mode):
        raise ConfigError("Credential profile file must be a regular file.")
    if hasattr(os, "getuid") and file_stat.st_uid != os.getuid():
        raise ConfigError("Credential profile file must be owned by the current user.")
    if require_private_permissions and stat.S_IMODE(file_stat.st_mode) & 0o177:
        raise ConfigError("Credential profile file permissions must be 0600 or stricter.")

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor: int | None = None
    try:
        descriptor = os.open(path, flags)
        opened_stat = os.fstat(descriptor)
        if (opened_stat.st_dev, opened_stat.st_ino) != (
            file_stat.st_dev,
            file_stat.st_ino,
        ):
            raise ConfigError("Credential profile file changed while it was opened.")
        with os.fdopen(descriptor, encoding="utf-8") as handle:
            descriptor = None
            return _parse_env_text(handle.read())
    except ConfigError:
        raise
    except (OSError, UnicodeError) as error:
        raise ConfigError("Credential profile file could not be read safely.") from error
    finally:
        if descriptor is not None:
            os.close(descriptor)


def read_legacy_env_file(path: Path) -> dict[str, str]:
    """Read an owner-controlled legacy file for one-way external migration."""

    return _read_profile_file(path, require_private_permissions=False)


def validate_profile_values(
    profile: Profile, values: Mapping[str, str]
) -> tuple[str, ...]:
    errors: list[str] = []
    unknown = sorted(set(values) - profile.allowed_keys)
    if unknown:
        errors.append("contains undeclared keys: " + ", ".join(unknown))
    missing = [name for name in profile.required if not values.get(name, "").strip()]
    if missing:
        errors.append("missing required keys: " + ", ".join(missing))
    if profile.one_of and not any(
        all(values.get(name, "").strip() for name in group) for group in profile.one_of
    ):
        choices = " or ".join(" + ".join(group) for group in profile.one_of)
        errors.append("requires one complete credential set: " + choices)
    return tuple(errors)


def load_profile(
    name: str,
    *,
    registry: Registry | None = None,
    environment: Mapping[str, str] | None = None,
) -> dict[str, str]:
    registry = load_registry() if registry is None else registry
    environment = os.environ if environment is None else environment
    profile = registry.profiles.get(name)
    if profile is None:
        raise ConfigError(f"Unknown credential profile: {name}.")
    path = resolve_profile_path(registry, profile, environment)
    values = _read_profile_file(path)
    for key in profile.allowed_keys:
        override = environment.get(key)
        if override is not None and override.strip():
            values[key] = override.strip()
    errors = validate_profile_values(profile, values)
    if errors:
        raise ConfigError(*(f"Credential profile {name} {error}." for error in errors))
    return {key: values[key] for key in profile.allowed_keys if key in values}


def load_profiles(
    names: Sequence[str],
    *,
    registry: Registry | None = None,
    environment: Mapping[str, str] | None = None,
) -> dict[str, str]:
    registry = load_registry() if registry is None else registry
    merged: dict[str, str] = {}
    for name in names:
        values = load_profile(name, registry=registry, environment=environment)
        for key, value in values.items():
            if key in merged and merged[key] != value:
                raise ConfigError(f"Credential profiles assign conflicting values for {key}.")
            merged[key] = value
    return merged


def profiles_for_consumer(
    name: str, *, registry: Registry | None = None
) -> tuple[str, ...]:
    registry = load_registry() if registry is None else registry
    profiles = registry.consumers.get(name)
    if profiles is None:
        raise ConfigError(f"Unknown credential consumer: {name}.")
    return profiles


def load_consumer(
    name: str,
    *,
    registry: Registry | None = None,
    environment: Mapping[str, str] | None = None,
) -> dict[str, str]:
    registry = load_registry() if registry is None else registry
    return load_profiles(
        profiles_for_consumer(name, registry=registry),
        registry=registry,
        environment=environment,
    )


def validate_profile(
    name: str,
    *,
    registry: Registry | None = None,
    environment: Mapping[str, str] | None = None,
) -> ValidationResult:
    try:
        load_profile(name, registry=registry, environment=environment)
    except ConfigError as error:
        return ValidationResult(profile=name, errors=error.messages)
    return ValidationResult(profile=name, errors=())


def validate_registry_examples(
    registry: Registry | None = None, *, registry_dir: Path | None = None
) -> None:
    registry = load_registry() if registry is None else registry
    registry_dir = REGISTRY_PATH.parent if registry_dir is None else registry_dir
    errors: list[str] = []
    for profile in registry.profiles.values():
        try:
            values = _parse_env_text(
                (registry_dir / profile.example).read_text(encoding="utf-8")
            )
        except (OSError, UnicodeError, ConfigError) as error:
            if isinstance(error, ConfigError):
                detail = "; ".join(error.messages)
            else:
                detail = "example file is unavailable"
            errors.append(f"Credential profile {profile.name} {detail}.")
            continue
        errors.extend(
            f"Credential profile {profile.name} example {detail}."
            for detail in validate_profile_values(profile, values)
        )
    if errors:
        raise ConfigError(*errors)

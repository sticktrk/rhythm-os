#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


CONFIG_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(CONFIG_DIR))

import rhythm_env  # noqa: E402


class RhythmEnvironmentTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.config_dir = Path(self.temporary.name)
        self.registry = rhythm_env.load_registry()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_profile(
        self, filename: str, content: str, *, mode: int = 0o600
    ) -> Path:
        path = self.config_dir / filename
        path.write_text(content, encoding="utf-8")
        path.chmod(mode)
        return path

    def environment(self, **overrides: str) -> dict[str, str]:
        environment = {"RHYTHM_CONFIG_DIR": str(self.config_dir)}
        environment.update(overrides)
        return environment

    def staff_password_content(self, *, marker: str = "password-value") -> str:
        return "\n".join(
            (
                "SUPABASE_URL=https://example.invalid",
                "SUPABASE_ANON_KEY=anon-value",
                "RHYTHM_ADMIN_EMAIL=staff@example.invalid",
                f"RHYTHM_ADMIN_PASSWORD={marker}",
                "",
            )
        )

    def app_build_content(self) -> str:
        return "\n".join(
            (
                "SUPABASE_URL=https://file.example.invalid",
                "SUPABASE_ANON_KEY=file-anon",
                "LOGIN_ENABLED=true",
                "",
            )
        )

    def test_checked_in_registry_and_examples_are_valid(self) -> None:
        rhythm_env.validate_registry_examples(self.registry)
        self.assertEqual(
            set(self.registry.profiles),
            {"staff", "analytics", "admin-api", "app-build", "release"},
        )
        self.assertEqual(
            rhythm_env.profiles_for_consumer("fleet-scout", registry=self.registry),
            ("staff", "analytics"),
        )

    def test_profile_uses_external_config_directory(self) -> None:
        self.write_profile("staff.env", self.staff_password_content())

        values = rhythm_env.load_profile(
            "staff", registry=self.registry, environment=self.environment()
        )

        self.assertEqual(values["SUPABASE_URL"], "https://example.invalid")
        self.assertEqual(values["RHYTHM_ADMIN_PASSWORD"], "password-value")

    def test_absolute_profile_override_wins(self) -> None:
        override_dir = self.config_dir / "override"
        override_dir.mkdir()
        override_path = override_dir / "custom.env"
        override_path.write_text(self.staff_password_content(marker="override"))
        override_path.chmod(0o600)

        values = rhythm_env.load_profile(
            "staff",
            registry=self.registry,
            environment=self.environment(RHYTHM_STAFF_ENV_FILE=str(override_path)),
        )

        self.assertEqual(values["RHYTHM_ADMIN_PASSWORD"], "override")

    def test_relative_paths_fail_closed(self) -> None:
        with self.assertRaisesRegex(
            rhythm_env.ConfigError, "configuration directory must be an absolute path"
        ):
            rhythm_env.load_profile(
                "staff",
                registry=self.registry,
                environment={"RHYTHM_CONFIG_DIR": "relative/config"},
            )

        with self.assertRaisesRegex(
            rhythm_env.ConfigError, "staff file must be an absolute path"
        ):
            rhythm_env.load_profile(
                "staff",
                registry=self.registry,
                environment=self.environment(RHYTHM_STAFF_ENV_FILE="staff.env"),
            )

    def test_nonempty_environment_overrides_but_empty_does_not_erase(self) -> None:
        self.write_profile("staff.env", self.staff_password_content())

        values = rhythm_env.load_profile(
            "staff",
            registry=self.registry,
            environment=self.environment(
                SUPABASE_URL="",
                SUPABASE_ANON_KEY="override-anon",
            ),
        )

        self.assertEqual(values["SUPABASE_URL"], "https://example.invalid")
        self.assertEqual(values["SUPABASE_ANON_KEY"], "override-anon")

    def test_environment_can_fill_blank_file_value(self) -> None:
        self.write_profile(
            "staff.env",
            self.staff_password_content().replace(
                "SUPABASE_ANON_KEY=anon-value", "SUPABASE_ANON_KEY="
            ),
        )

        values = rhythm_env.load_profile(
            "staff",
            registry=self.registry,
            environment=self.environment(SUPABASE_ANON_KEY="environment-anon"),
        )

        self.assertEqual(values["SUPABASE_ANON_KEY"], "environment-anon")

    def test_staff_profile_accepts_access_token_alternative(self) -> None:
        self.write_profile(
            "staff.env",
            "\n".join(
                (
                    "SUPABASE_URL=https://example.invalid",
                    "SUPABASE_ANON_KEY=anon-value",
                    "RHYTHM_ADMIN_ACCESS_TOKEN=token-value",
                    "",
                )
            ),
        )

        values = rhythm_env.load_profile(
            "staff", registry=self.registry, environment=self.environment()
        )

        self.assertEqual(values["RHYTHM_ADMIN_ACCESS_TOKEN"], "token-value")

    def test_incomplete_staff_alternative_reports_names_only(self) -> None:
        marker = "never-print-this-password"
        self.write_profile(
            "staff.env",
            "\n".join(
                (
                    "SUPABASE_URL=https://example.invalid",
                    "SUPABASE_ANON_KEY=anon-value",
                    "RHYTHM_ADMIN_EMAIL=staff@example.invalid",
                    f"# {marker}",
                    "",
                )
            ),
        )

        result = rhythm_env.validate_profile(
            "staff", registry=self.registry, environment=self.environment()
        )

        self.assertFalse(result.ok)
        combined = "\n".join(result.errors)
        self.assertIn("RHYTHM_ADMIN_EMAIL + RHYTHM_ADMIN_PASSWORD", combined)
        self.assertNotIn(marker, combined)

    def test_permissions_and_symlinks_fail_closed(self) -> None:
        profile = self.write_profile(
            "staff.env", self.staff_password_content(), mode=0o644
        )
        result = rhythm_env.validate_profile(
            "staff", registry=self.registry, environment=self.environment()
        )
        self.assertFalse(result.ok)
        self.assertIn("permissions must be 0600 or stricter", "\n".join(result.errors))

        profile.unlink()
        target = self.write_profile("staff-target.env", self.staff_password_content())
        profile.symlink_to(target)
        result = rhythm_env.validate_profile(
            "staff", registry=self.registry, environment=self.environment()
        )
        self.assertFalse(result.ok)
        self.assertIn("must not be a symbolic link", "\n".join(result.errors))

    def test_owner_execute_permission_fails_closed(self) -> None:
        self.write_profile("staff.env", self.staff_password_content(), mode=0o700)

        result = rhythm_env.validate_profile(
            "staff", registry=self.registry, environment=self.environment()
        )

        self.assertFalse(result.ok)
        self.assertIn("permissions must be 0600 or stricter", "\n".join(result.errors))

    def test_undeclared_and_duplicate_keys_fail_closed(self) -> None:
        self.write_profile(
            "staff.env", self.staff_password_content() + "SERVICE_ROLE_SECRET=nope\n"
        )
        result = rhythm_env.validate_profile(
            "staff", registry=self.registry, environment=self.environment()
        )
        self.assertFalse(result.ok)
        self.assertIn("SERVICE_ROLE_SECRET", "\n".join(result.errors))

        self.write_profile(
            "staff.env",
            self.staff_password_content() + "SUPABASE_URL=https://duplicate.invalid\n",
        )
        result = rhythm_env.validate_profile(
            "staff", registry=self.registry, environment=self.environment()
        )
        self.assertFalse(result.ok)
        self.assertIn("defines SUPABASE_URL more than once", "\n".join(result.errors))

    def test_consumer_loads_only_its_declared_profiles(self) -> None:
        self.write_profile("staff.env", self.staff_password_content())
        self.write_profile(
            "analytics.env",
            "\n".join(
                (
                    "POSTHOG_PERSONAL_API_KEY=posthog-secret",
                    "POSTHOG_PROJECT_ID=12345",
                    "",
                )
            ),
        )

        values = rhythm_env.load_consumer(
            "fleet-scout", registry=self.registry, environment=self.environment()
        )

        self.assertIn("RHYTHM_ADMIN_PASSWORD", values)
        self.assertIn("POSTHOG_PERSONAL_API_KEY", values)
        self.assertNotIn("SUPABASE_SERVICE_ROLE_KEY", values)

    def test_validator_never_prints_values_or_paths(self) -> None:
        marker = "never-print-this-secret"
        self.write_profile("staff.env", self.staff_password_content(marker=marker))
        environment = os.environ.copy()
        for name in self.registry.profiles["staff"].allowed_keys:
            environment.pop(name, None)
        environment["RHYTHM_CONFIG_DIR"] = str(self.config_dir)

        result = subprocess.run(
            [
                sys.executable,
                str(CONFIG_DIR / "validate.py"),
                "--profile",
                "staff",
            ],
            check=False,
            capture_output=True,
            text=True,
            env=environment,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        output = result.stdout + result.stderr
        self.assertIn("staff: ready", output)
        self.assertNotIn(marker, output)
        self.assertNotIn(str(self.config_dir), output)

    def test_command_loader_passes_values_without_printing_them(self) -> None:
        marker = "never-print-this-runner-secret"
        self.write_profile("staff.env", self.staff_password_content(marker=marker))
        environment = os.environ.copy()
        for name in self.registry.profiles["staff"].allowed_keys:
            environment.pop(name, None)
        environment["RHYTHM_CONFIG_DIR"] = str(self.config_dir)

        result = subprocess.run(
            [
                sys.executable,
                str(CONFIG_DIR / "run.py"),
                "--profile",
                "staff",
                "--",
                sys.executable,
                "-c",
                (
                    "import os,sys; "
                    f"sys.exit(0 if os.environ.get('RHYTHM_ADMIN_PASSWORD') == {marker!r} "
                    "else 9)"
                ),
            ],
            check=False,
            capture_output=True,
            text=True,
            env=environment,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        output = result.stdout + result.stderr
        self.assertNotIn(marker, output)
        self.assertNotIn(str(self.config_dir), output)

    def test_app_build_materializer_applies_overrides_and_is_private(self) -> None:
        source = self.write_profile("app-build.env", self.app_build_content())
        environment = os.environ.copy()
        for name in self.registry.profiles["app-build"].allowed_keys:
            environment.pop(name, None)
        environment.update(
            {
                "RHYTHM_CONFIG_DIR": str(self.config_dir),
                "SUPABASE_URL": "https://override.example.invalid",
            }
        )

        result = subprocess.run(
            [sys.executable, str(CONFIG_DIR / "materialize_app_build.py")],
            check=False,
            capture_output=True,
            text=True,
            env=environment,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        materialized = Path(result.stdout.strip())
        try:
            self.assertEqual(materialized.parent, self.config_dir)
            self.assertNotEqual(materialized, source)
            self.assertEqual(stat.S_IMODE(materialized.stat().st_mode), 0o600)
            values = json.loads(materialized.read_text(encoding="utf-8"))
            self.assertEqual(
                values["SUPABASE_URL"], "https://override.example.invalid"
            )
            self.assertEqual(values["SUPABASE_ANON_KEY"], "file-anon")
            self.assertEqual(
                source.read_text(encoding="utf-8"), self.app_build_content()
            )
        finally:
            materialized.unlink(missing_ok=True)

    def test_app_build_materializer_fails_closed_when_profile_is_missing(self) -> None:
        environment = os.environ.copy()
        for name in self.registry.profiles["app-build"].allowed_keys:
            environment.pop(name, None)
        environment["RHYTHM_CONFIG_DIR"] = str(self.config_dir)

        result = subprocess.run(
            [sys.executable, str(CONFIG_DIR / "materialize_app_build.py")],
            check=False,
            capture_output=True,
            text=True,
            env=environment,
        )

        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertNotIn(str(self.config_dir), result.stderr)

    def test_provisioner_splits_legacy_files_and_hardens_targets(self) -> None:
        legacy_root = self.config_dir / "legacy"
        external_dir = self.config_dir / "external"
        app_dir = legacy_root / "app/flutter/rhythm_app"
        admin_dir = legacy_root / "admin-api"
        app_dir.mkdir(parents=True)
        admin_dir.mkdir(parents=True)
        (app_dir / ".env").write_text(
            "\n".join(
                (
                    "SUPABASE_URL=https://app.example.invalid",
                    "SUPABASE_ANON_KEY=app-anon",
                    "GOOGLE_SIGN_IN_SERVER_CLIENT_ID=app-client",
                    "POSTHOG_API_KEY=app-posthog",
                    "",
                )
            ),
            encoding="utf-8",
        )
        (app_dir / ".env").chmod(0o644)
        (admin_dir / ".env").write_text(
            "\n".join(
                (
                    "SUPABASE_URL=https://admin.example.invalid",
                    "SUPABASE_ANON_KEY=admin-anon",
                    "SUPABASE_SERVICE_ROLE_KEY=service-secret",
                    "RHYTHM_ADMIN_EMAIL=staff@example.invalid",
                    "RHYTHM_ADMIN_PASSWORD=staff-secret",
                    "POSTHOG_PERSONAL_API_KEY=analytics-secret",
                    "POSTHOG_PROJECT_ID=12345",
                    "",
                )
            ),
            encoding="utf-8",
        )
        (admin_dir / ".env").chmod(0o600)

        result = subprocess.run(
            [
                sys.executable,
                str(CONFIG_DIR / "provision.py"),
                "--legacy-root",
                str(legacy_root),
                "--config-dir",
                str(external_dir),
                "--profile",
                "staff",
                "--profile",
                "analytics",
                "--profile",
                "admin-api",
                "--profile",
                "app-build",
            ],
            check=False,
            capture_output=True,
            text=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(stat.S_IMODE(external_dir.stat().st_mode), 0o700)
        for filename in (
            "staff.env",
            "analytics.env",
            "admin-api.env",
            "app-build.env",
        ):
            self.assertEqual(stat.S_IMODE((external_dir / filename).stat().st_mode), 0o600)
        environment = {"RHYTHM_CONFIG_DIR": str(external_dir)}
        staff = rhythm_env.load_profile(
            "staff", registry=self.registry, environment=environment
        )
        admin = rhythm_env.load_profile(
            "admin-api", registry=self.registry, environment=environment
        )
        app = rhythm_env.load_profile(
            "app-build", registry=self.registry, environment=environment
        )
        self.assertEqual(staff["SUPABASE_URL"], "https://admin.example.invalid")
        self.assertNotIn("SUPABASE_SERVICE_ROLE_KEY", staff)
        self.assertNotIn("POSTHOG_PERSONAL_API_KEY", admin)
        self.assertEqual(app["SUPABASE_URL"], "https://app.example.invalid")

    def test_provisioner_does_not_create_incomplete_or_overwrite_existing(self) -> None:
        legacy_root = self.config_dir / "legacy"
        external_dir = self.config_dir / "external"
        (legacy_root / "admin-api").mkdir(parents=True)
        (legacy_root / "admin-api/.env").write_text(
            self.staff_password_content(marker="original-secret"), encoding="utf-8"
        )
        (legacy_root / "admin-api/.env").chmod(0o600)
        command = [
            sys.executable,
            str(CONFIG_DIR / "provision.py"),
            "--legacy-root",
            str(legacy_root),
            "--config-dir",
            str(external_dir),
            "--profile",
            "staff",
        ]
        first = subprocess.run(command, check=False, capture_output=True, text=True)
        self.assertEqual(first.returncode, 0, first.stderr)

        (legacy_root / "admin-api/.env").write_text(
            self.staff_password_content(marker="replacement-secret"), encoding="utf-8"
        )
        second = subprocess.run(command, check=False, capture_output=True, text=True)
        self.assertEqual(second.returncode, 0, second.stderr)
        values = rhythm_env.load_profile(
            "staff",
            registry=self.registry,
            environment={"RHYTHM_CONFIG_DIR": str(external_dir)},
        )
        self.assertEqual(values["RHYTHM_ADMIN_PASSWORD"], "original-secret")

        release = subprocess.run(
            [
                sys.executable,
                str(CONFIG_DIR / "provision.py"),
                "--legacy-root",
                str(legacy_root),
                "--config-dir",
                str(external_dir),
                "--profile",
                "release",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(release.returncode, 1)
        self.assertFalse((external_dir / "release.env").exists())
        output = release.stdout + release.stderr
        self.assertNotIn("original-secret", output)
        self.assertNotIn(str(legacy_root), output)


if __name__ == "__main__":
    unittest.main()

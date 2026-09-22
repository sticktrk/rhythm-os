#!/usr/bin/env python3
"""Exercise version resolution through the build entrypoint without build/store writes."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]


class TestFlightVersionTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        scripts = root / 'tools/app/scripts'
        scripts.mkdir(parents=True)
        shutil.copy(SCRIPTS / 'build-mobile.sh', scripts)
        shutil.copytree(SCRIPTS / 'lib', scripts / 'lib')
        self.script = scripts / 'build-mobile.sh'
        app = root / 'app/flutter/rhythm_app'
        app.mkdir(parents=True)
        self.pubspec = app / 'pubspec.yaml'
        self.pubspec.write_text('version: 0.0.0+1\n')
        key = root / 'key.json'
        key.write_text('{}')
        self.log = root / 'calls'
        bin_dir = root / 'bin'
        bin_dir.mkdir()
        fastlane = bin_dir / 'fastlane'
        fastlane.write_text('''#!/bin/bash
printf '%s\\n' "$*" >> "$CALL_LOG"
if [ "$1" = app_build_testflight_version ]; then
    echo "RHYTHM_TESTFLIGHT_VERSION=${TEST_VERSION:-1.2.3}"
    exit "${TEST_STATUS:-0}"
fi
# Stop at the subsequent scoped build-number request, before any build.
exit 73
''')
        fastlane.chmod(0o755)
        self.env = {
            'PATH': f'{bin_dir}:/usr/bin:/bin', 'HOME': str(root),
            'RHYTHM_ASC_API_KEY_PATH': str(key), 'TEAM_ID': 'ABCDEFGHIJ',
            'CALL_LOG': str(self.log),
        }

    def run_build(self, *args):
        result = subprocess.run(['bash', str(self.script), '--testflight', *args],
                                env=self.env, text=True, capture_output=True)
        calls = self.log.read_text() if self.log.exists() else ''
        return result, calls

    def test_discovers_version_before_scoped_build_number(self):
        result, calls = self.run_build()
        self.assertEqual(result.returncode, 73, result.stdout + result.stderr)
        self.assertIn('app_build_testflight_version', calls)
        self.assertIn('version:1.2.3', calls)
        self.assertIn('Using app version: 1.2.3', result.stdout)

    def test_explicit_versions_skip_discovery(self):
        for env_version, args, expected in [
            ('2.3.4', [], '2.3.4'),
            ('2.3.4', ['--build-name', '3.4.5'], '3.4.5'),
        ]:
            with self.subTest(expected=expected):
                self.env['RHYTHM_APP_VERSION'] = env_version
                self.log.unlink(missing_ok=True)
                result, calls = self.run_build(*args)
                self.assertEqual(result.returncode, 73)
                self.assertNotIn('app_build_testflight_version', calls)
                self.assertIn(f'version:{expected}', calls)

    def test_real_pubspec_version_skips_discovery(self):
        self.pubspec.write_text('version: 5.6.7+1\n')
        result, calls = self.run_build()
        self.assertEqual(result.returncode, 73)
        self.assertNotIn('app_build_testflight_version', calls)
        self.assertIn('version:5.6.7', calls)

    def test_unusable_versions_stop_before_number_lookup(self):
        for version in ['0.0.0', 'garbage', '1.2', '1.2.3suffix']:
            with self.subTest(version=version):
                self.log.unlink(missing_ok=True)
                self.env['TEST_VERSION'] = version
                result, calls = self.run_build()
                self.assertEqual(result.returncode, 1)
                self.assertNotIn('run latest_testflight_build_number', calls)
                self.assertIn('Pass --build-name', result.stdout)

    def test_failed_lookup_cannot_use_output(self):
        self.env['TEST_STATUS'] = '1'
        result, calls = self.run_build()
        self.assertEqual(result.returncode, 1)
        self.assertNotIn('run latest_testflight_build_number', calls)
        self.assertIn('could not discover', result.stdout)

    def test_release_all_rejects_placeholder_before_either_upload(self):
        result, calls = self.run_build('--release-all')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(calls, '')
        self.assertIn('--release-all requires', result.stdout)

    def test_explicit_placeholder_is_not_silently_replaced(self):
        result, calls = self.run_build('--build-name', '0.0.0')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(calls, '')


if __name__ == '__main__':
    unittest.main()

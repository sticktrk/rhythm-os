"""Exercise the real mobile entrypoint without Xcode or store side effects."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class TestFlightBuildTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='testflight fixture ')
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name)
        self.repo = self.path/'source'
        self.app = self.repo/'app/flutter/rhythm_app'
        (self.app/'ios/Runner.xcworkspace').mkdir(parents=True)
        (self.app/'pubspec.yaml').write_text('version: 1.2.3+4\n')
        source = Path(__file__).resolve().parents[2]
        scripts = self.repo/'tools/app/scripts'
        scripts.mkdir(parents=True)
        for name in ['build-mobile.sh', 'export-testflight-ipa.sh', 'write-app-build-receipt.sh']:
            shutil.copy2(source/'tools/app/scripts'/name, scripts/name)
        shutil.copytree(source/'tools/app/scripts/lib', scripts/'lib')
        shutil.copytree(source/'tools/config', self.repo/'tools/config',
                        ignore=shutil.ignore_patterns('__pycache__'))
        self.profile = self.path/'app-build.env'
        self.profile.write_text('SUPABASE_URL=https://example.invalid\nSUPABASE_ANON_KEY=fixture\n')
        self.profile.chmod(0o600)
        self.key = self.path/'key.json'
        self.key.write_text(json.dumps(dict(key_id='KEY123', issuer_id='fixture-issuer', key='fixture-key')))
        self.key.chmod(0o600)
        self.log = self.path/'commands.jsonl'
        self.env = dict(os.environ)
        for name in subprocess.check_output(['git', 'rev-parse', '--local-env-vars'], text=True).splitlines():
            self.env.pop(name, None)
        for name in list(self.env):
            if name.startswith(('RHYTHM_', 'CROSS_', 'SUPABASE_')) or name == 'TEAM_ID':
                self.env.pop(name)
        self.env.update(TEAM_ID='TEAM123456', RHYTHM_ASC_API_KEY_PATH=str(self.key),
                        RHYTHM_APP_BUILD_ENV_FILE=str(self.profile),
                        RHYTHM_APP_BUILD_EVIDENCE_ROOT=str(self.path/'evidence'),
                        FIXTURE_LOG=str(self.log), GIT_AUTHOR_NAME='Fixture',
                        GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_NAME='Fixture',
                        GIT_COMMITTER_EMAIL='fixture@example.invalid')
        binary = self.path/'bin'
        binary.mkdir()
        fake = '''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['FIXTURE_LOG'], 'a') as output:
    output.write(json.dumps([name, args]) + '\\n')
def value(flag): return args[args.index(flag) + 1]
if name == 'fastlane' and args[0] == 'run':
    print('Result: 15')
elif name == 'flutter' and args[:2] == ['build', 'ios']:
    assert '--config-only' in args and '--no-codesign' in args
    assert '--release' in args and '--build-number=16' in args
    defines = next(a.split('=', 1)[1] for a in args if a.startswith('--dart-define-from-file='))
    assert 'TEAM_ID' not in json.loads(pathlib.Path(defines).read_text())
    sys.exit(int(os.environ.get('FIXTURE_CONFIG_STATUS', '0')))
elif name == 'xcodebuild' and '-workspace' in args:
    assert 'DEVELOPMENT_TEAM=TEAM123456' in args
    assert '-authenticationKeyPath' in args and '-allowProvisioningUpdates' in args
    status = int(os.environ.get('FIXTURE_ARCHIVE_STATUS', '0'))
    if status: sys.exit(status)
    pathlib.Path(value('-archivePath')).mkdir(parents=True)
elif name == 'xcodebuild' and '-exportArchive' in args:
    target = pathlib.Path(value('-exportPath'))
    target.mkdir(parents=True, exist_ok=True)
    (target/'Fixture.ipa').write_bytes(b'fixture ipa')
elif name == 'flutter' and args[:2] == ['build', 'ipa']:
    target = pathlib.Path('build/ios/ipa')
    target.mkdir(parents=True, exist_ok=True)
    (target/'Fixture.ipa').write_bytes(b'fixture ipa')
'''
        for name in ['flutter', 'pod', 'xcodebuild', 'fastlane']:
            executable = binary/name
            executable.write_text(fake)
            executable.chmod(0o755)
        self.env['PATH'] = str(binary) + os.pathsep + self.env['PATH']
        for args in [('init', '-b', 'master'), ('add', '.'), ('commit', '-m', 'Fixture')]:
            subprocess.run(['git', '-C', str(self.repo), *args], env=self.env,
                           capture_output=True, check=True)

    def build(self, *args, success=True):
        result = subprocess.run([str(self.repo/'tools/app/scripts/build-mobile.sh'), *args],
                                cwd=self.repo, env=self.env, capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_authenticated_archive_export_and_one_upload(self):
        self.build('--testflight')
        calls = self.calls()
        self.assertEqual([args[0] for name, args in calls if name == 'xcodebuild'],
                         ['-workspace', '-exportArchive'])
        self.assertEqual(sum(name == 'fastlane' and args[:2] == ['pilot', 'upload']
                             for name, args in calls), 1)
        self.assertEqual(len(list((self.path/'evidence').glob('*.json'))), 1)
        self.assertFalse(list(self.path.glob('.app-build.env.build.*')))
        self.assertFalse((self.repo/'.release-evidence').exists())

    def test_missing_team_stops_before_clean_or_remote_calls(self):
        self.env.pop('TEAM_ID')
        result = self.build('--testflight', success=False)
        self.assertIn('set TEAM_ID', result.stdout)
        self.assertEqual(self.calls(), [])

    def test_failed_archive_never_exports_or_uploads(self):
        self.env['FIXTURE_ARCHIVE_STATUS'] = '65'
        self.build('--testflight', success=False)
        self.assertFalse(any('-exportArchive' in args or args[:2] == ['pilot', 'upload']
                             for _, args in self.calls()))
        self.assertFalse((self.path/'evidence').exists())

    def test_failed_flutter_configuration_never_archives(self):
        self.env['FIXTURE_CONFIG_STATUS'] = '1'
        self.build('--testflight', success=False)
        self.assertFalse(any(name == 'xcodebuild' or args[:2] == ['pilot', 'upload']
                             for name, args in self.calls()))

    def test_ipa_without_api_key_keeps_interactive_flutter_path(self):
        self.env.pop('TEAM_ID')
        self.env['RHYTHM_ASC_API_KEY_PATH'] = str(self.path/'missing.json')
        self.build('--ipa', '--build-number', '16')
        self.assertTrue(any(name == 'flutter' and args[:2] == ['build', 'ipa']
                            for name, args in self.calls()))
        self.assertFalse(any(name in ['xcodebuild', 'fastlane'] for name, _ in self.calls()))


if __name__ == '__main__':
    unittest.main()

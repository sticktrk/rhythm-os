"""Release contract fixtures use local Git remotes and a synthetic publisher."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch


class ReleaseFixture(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix='release fixture ')
        self.addCleanup(tmp.cleanup)
        self.path = Path(tmp.name)
        self.repo = self.path / 'source'
        self.repo.mkdir()
        self.remote = self.path / 'upstream.git'
        self.log = self.path / 'publisher.log'
        self.hook = self.path / 'publish fixture'
        self.hook.write_text('''#!/bin/bash
set -euo pipefail
test "$2" = "$(git --git-dir "$TEST_UPSTREAM" rev-parse "refs/tags/$1^{commit}")"
printf '%s %s\\n' "$1" "$2" >> "$TEST_PUBLISH_LOG"
exit "${TEST_PUBLISH_EXIT:-0}"
''')
        self.hook.chmod(0o755)
        env = dict(os.environ)
        for name in subprocess.check_output(['git', 'rev-parse', '--local-env-vars'], text=True).splitlines():
            env.pop(name, None)
        for name in list(env):
            if name.startswith(('RHYTHM_', 'CROSS_')):
                env.pop(name)
        env.update(GIT_ALLOW_PROTOCOL='file', GIT_AUTHOR_NAME='Fixture',
                   GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_NAME='Fixture',
                   GIT_COMMITTER_EMAIL='fixture@example.invalid',
                   RHYTHM_RELEASE_PUBLISH_HOOK=str(self.hook), TEST_UPSTREAM=str(self.remote),
                   TEST_PUBLISH_LOG=str(self.log))
        environ = patch.dict(os.environ, env, clear=True)
        environ.start()
        self.addCleanup(environ.stop)
        source = Path(__file__).resolve().parents[1] / 'os/scripts'
        scripts = self.repo / 'tools/os/scripts'
        (scripts / 'lib').mkdir(parents=True)
        for name in ['release.sh', 'promote-stable.sh', 'verify-beta-release.sh',
                     'lib/version.sh', 'lib/publication.sh']:
            shutil.copy2(source / name, scripts / name)
        (self.repo / 'os/install/rpiz').mkdir(parents=True)
        (self.repo / 'os/install/rpiz/builder-image.lock').write_text('fixture\n')
        (self.repo / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0-beta"\n')
        (self.repo / 'Cargo.lock').write_text('version = 4\n[[package]]\nname = "rhythm-fixture"\nversion = "0.1.0-beta"\n')
        self.git('init', '-b', 'master')
        self.git('add', '.')
        self.git('commit', '-m', 'Initial source')
        self.git('tag', '-a', 'v0.1.0-beta', '-m', 'Initial beta')
        self.git('clone', '--bare', str(self.repo), str(self.remote))
        self.git('remote', 'add', 'origin', str(self.remote))

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args],
                                       text=True, stderr=subprocess.DEVNULL).strip()

    def run_tool(self, name, *args, success=True):
        result = subprocess.run([str(self.repo / 'tools/os/scripts' / name), *args],
                                cwd=self.path, text=True, capture_output=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def release(self, *args, success=True):
        return self.run_tool('release.sh', '--skip-builder-refresh', '--version', '0.1.1',
                             *args, success=success)


class ReleasePublicationTests(ReleaseFixture):
    def test_publisher_receives_exact_published_beta_and_stable(self):
        self.release()
        commit = self.git('rev-parse', 'v0.1.1-beta^{commit}')
        self.assertEqual(self.log.read_text(), f'v0.1.1-beta {commit}\n')
        self.run_tool('release.sh', '--promote-stable', '0.1.1',
                      '--skip-beta-verification', '--reason', 'Synthetic fixture')
        self.assertEqual(self.log.read_text().splitlines(),
                         [f'v0.1.1-beta {commit}', f'v0.1.1-stable {commit}'])

    def test_dry_run_does_not_mutate_or_publish(self):
        before = self.git('rev-parse', 'HEAD')
        result = self.release('--dry-run')
        self.assertIn('Would invoke the configured publisher', result.stdout)
        self.assertEqual(self.git('rev-parse', 'HEAD'), before)
        self.assertFalse(self.log.exists())
        self.assertEqual(self.git('tag', '--list', 'v0.1.1-beta'), '')

    def test_local_tag_never_invokes_publisher(self):
        self.release('--no-push')
        self.assertEqual(self.git('tag', '--list', 'v0.1.1-beta'), 'v0.1.1-beta')
        self.assertFalse(self.log.exists())

    def test_failed_tag_push_never_invokes_publisher(self):
        reject = self.remote / 'hooks/pre-receive'
        reject.write_text('#!/bin/bash\nwhile read -r old new ref; do\ncase "$ref" in refs/tags/*) exit 1;; esac\ndone\n')
        reject.chmod(0o755)
        self.release(success=False)
        self.assertFalse(self.log.exists())

    def test_invalid_publisher_fails_before_release_mutations(self):
        before = self.git('rev-parse', 'HEAD')
        os.environ['RHYTHM_RELEASE_PUBLISH_HOOK'] = str(self.path / 'missing')
        self.release(success=False)
        self.assertEqual(self.git('rev-parse', 'HEAD'), before)
        self.assertEqual(self.git('tag', '--list', 'v0.1.1-beta'), '')

    def test_publisher_failure_is_reported(self):
        os.environ['TEST_PUBLISH_EXIT'] = '7'
        self.release(success=False)
        self.assertTrue(self.log.exists())

    def test_standalone_release_does_not_claim_an_upload(self):
        os.environ.pop('RHYTHM_RELEASE_PUBLISH_HOOK')
        result = self.release()
        self.assertIn('no OTA upload was requested', result.stdout)
        self.assertFalse(self.log.exists())

    def test_verification_evidence_can_live_outside_source(self):
        evidence = self.path / 'private evidence'
        os.environ['RHYTHM_RELEASE_EVIDENCE_ROOT'] = str(evidence)
        result = self.run_tool('verify-beta-release.sh', '--version', '0.1.0', '--dry-run')
        self.assertIn(str(evidence / 'v0.1.0-beta.json'), result.stdout)
        self.assertFalse((self.repo / '.release-evidence').exists())


if __name__ == '__main__':
    unittest.main()

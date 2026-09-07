import importlib.util
from pathlib import Path
import stat
import subprocess
import unittest
from unittest.mock import patch

MODULE_PATH = Path(__file__).resolve().parents[1] / 'monster-cloud-secrets.py'
spec = importlib.util.spec_from_file_location('monster_cloud_secrets', MODULE_PATH)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SecretUploadTests(unittest.TestCase):
    def run_upload(self, fail=False):
        seen = []
        values = ['owner', 'email', 'p$word"é\\end', 'app', 'app-secret']

        def upload(args, **kwargs):
            filename = Path(args[-1])
            seen.append(filename)
            self.assertEqual(stat.S_IMODE(filename.stat().st_mode), 0o600)
            contents = filename.read_text()
            self.assertIn('MONSTER_PASSWORD="p\\$word\\"é\\\\end"', contents)
            self.assertIn('MONSTER_TICKET_SECRET=', contents)
            self.assertNotIn(values[2], args)
            self.assertEqual(kwargs['stdout'], subprocess.DEVNULL)
            self.assertEqual(kwargs['stderr'], subprocess.DEVNULL)
            if fail:
                raise OSError('fixture CLI unavailable')
            return subprocess.CompletedProcess(args, 0)

        with patch('sys.argv', ['helper', '--project-ref', 'fixture-project']), \
                patch.object(module.getpass, 'getpass', side_effect=values), \
                patch.object(module.subprocess, 'run', side_effect=upload), \
                patch('builtins.print'):
            if fail:
                with self.assertRaises(OSError):
                    module.main()
            else:
                module.main()
        self.assertEqual(len(seen), 1)
        self.assertFalse(seen[0].exists())

    def test_upload_uses_private_file_and_removes_it(self):
        self.run_upload()

    def test_cli_failure_still_removes_private_file(self):
        self.run_upload(fail=True)


if __name__ == '__main__':
    unittest.main()

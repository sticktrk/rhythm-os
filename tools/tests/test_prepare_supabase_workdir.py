import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    'prepare_supabase', Path(__file__).resolve().parents[1] / 'prepare-supabase-workdir.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SharedMigrationHistoryTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.core = self.root / 'core'
        self.marketing = self.root / 'marketing'
        self.destination = self.root / 'composed'
        for root in (self.core, self.marketing / 'supabase'):
            (root / 'migrations').mkdir(parents=True)
            (root / 'config.toml').write_text('# fixture config\n')
            (root / '.env').write_text('SECRET=must-not-copy\n')
        (self.core / 'migrations/20260101000000_product.sql').write_bytes(b'SELECT 1;\r\n')
        (self.marketing / 'supabase/migrations/20260102000000_marketing.sql').write_bytes(b'SELECT 2;\n')
        (self.core / '.temp').mkdir()
        (self.core / '.temp/project-ref').write_text('fixture-project')

    def test_preserves_full_history_bytes_and_link_without_copying_secrets(self):
        module.prepare(self.core, self.marketing, self.destination)
        project = self.destination / 'supabase'
        self.assertEqual(len(list((project / 'migrations').glob('*.sql'))), 2)
        for root in (self.core, self.marketing / 'supabase'):
            for file in (root / 'migrations').glob('*.sql'):
                self.assertEqual(file.read_bytes(), (project / 'migrations' / file.name).read_bytes())
        self.assertEqual((project / '.temp/project-ref').read_text(), 'fixture-project')
        self.assertFalse((project / '.env').exists())
        self.assertFalse((project / 'functions').exists())

    def test_duplicate_versions_fail_before_creating_workdir(self):
        (self.marketing / 'supabase/migrations/20260101000000_collision.sql').write_text('SELECT 3;')
        with self.assertRaisesRegex(ValueError, 'Duplicate migration version'):
            module.prepare(self.core, self.marketing, self.destination)
        self.assertFalse(self.destination.exists())

    def test_missing_marketing_history_fails_closed(self):
        (self.marketing / 'supabase/migrations/20260102000000_marketing.sql').unlink()
        with self.assertRaisesRegex(ValueError, 'Missing migrations'):
            module.prepare(self.core, self.marketing, self.destination)
        self.assertFalse(self.destination.exists())


if __name__ == '__main__':
    unittest.main()

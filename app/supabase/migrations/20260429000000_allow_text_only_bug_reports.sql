-- Allow text-only bug reports (no debug bundle, no paired server hub).

ALTER TABLE support_debug_bundle_submissions
  ALTER COLUMN bundle_storage_path DROP NOT NULL,
  ALTER COLUMN bundle_file_name DROP NOT NULL,
  ALTER COLUMN bundle_content_type DROP NOT NULL;

ALTER TABLE support_debug_bundle_submissions
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_size_bytes_check;

ALTER TABLE support_debug_bundle_submissions
  ADD CONSTRAINT support_debug_bundle_submissions_bundle_size_bytes_check
  CHECK (bundle_size_bytes IS NULL OR bundle_size_bytes > 0);

ALTER TABLE support_debug_bundle_submissions
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_storage_path_key;

CREATE UNIQUE INDEX IF NOT EXISTS support_debug_bundle_submissions_bundle_storage_path_idx
  ON support_debug_bundle_submissions (bundle_storage_path)
  WHERE bundle_storage_path IS NOT NULL;

COMMENT ON COLUMN support_debug_bundle_submissions.bundle_storage_path IS
  'Private Supabase Storage path for the uploaded tar.gz support archive (null for text-only reports)';

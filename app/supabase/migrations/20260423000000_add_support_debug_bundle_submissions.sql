-- Private support submissions for uploaded server debug bundles.

INSERT INTO storage.buckets (
  id,
  name,
  public,
  file_size_limit,
  allowed_mime_types
)
VALUES (
  'support-debug-bundles',
  'support-debug-bundles',
  FALSE,
  26214400,
  ARRAY['application/gzip']
)
ON CONFLICT (id) DO UPDATE
SET
  public = EXCLUDED.public,
  file_size_limit = EXCLUDED.file_size_limit,
  allowed_mime_types = EXCLUDED.allowed_mime_types;

CREATE TABLE IF NOT EXISTS support_debug_bundle_submissions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  user_email TEXT,
  is_anonymous BOOLEAN NOT NULL DEFAULT TRUE,
  reference_code TEXT NOT NULL UNIQUE DEFAULT (
    'DBG-' || UPPER(SUBSTRING(REPLACE(gen_random_uuid()::TEXT, '-', '') FROM 1 FOR 8))
  ),
  status TEXT NOT NULL DEFAULT 'received'
    CHECK (status IN ('received', 'reviewing', 'resolved', 'error')),
  summary TEXT,
  app_version TEXT,
  app_build TEXT,
  app_platform TEXT,
  server_hub_id TEXT,
  server_name TEXT,
  server_host TEXT,
  server_port INTEGER,
  server_version TEXT,
  server_platform_context TEXT,
  bundle_storage_path TEXT NOT NULL UNIQUE,
  bundle_file_name TEXT NOT NULL,
  bundle_content_type TEXT NOT NULL,
  bundle_size_bytes BIGINT NOT NULL CHECK (bundle_size_bytes > 0),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS support_debug_bundle_submissions_user_id_idx
  ON support_debug_bundle_submissions (user_id);

CREATE INDEX IF NOT EXISTS support_debug_bundle_submissions_created_at_idx
  ON support_debug_bundle_submissions (created_at DESC);

ALTER TABLE support_debug_bundle_submissions ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Users can view their debug bundle submissions"
  ON support_debug_bundle_submissions;
CREATE POLICY "Users can view their debug bundle submissions"
  ON support_debug_bundle_submissions FOR SELECT
  USING (auth.uid() = user_id);

DROP POLICY IF EXISTS "Users can create their debug bundle submissions"
  ON support_debug_bundle_submissions;
CREATE POLICY "Users can create their debug bundle submissions"
  ON support_debug_bundle_submissions FOR INSERT
  WITH CHECK (auth.uid() = user_id);

DROP TRIGGER IF EXISTS support_debug_bundle_submissions_updated_at
  ON support_debug_bundle_submissions;
CREATE TRIGGER support_debug_bundle_submissions_updated_at
  BEFORE UPDATE ON support_debug_bundle_submissions
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

DROP POLICY IF EXISTS "Users can upload their debug bundles"
  ON storage.objects;
CREATE POLICY "Users can upload their debug bundles"
  ON storage.objects FOR INSERT TO authenticated
  WITH CHECK (
    bucket_id = 'support-debug-bundles'
    AND auth.uid()::TEXT = (storage.foldername(name))[1]
  );

DROP POLICY IF EXISTS "Users can view their debug bundles"
  ON storage.objects;
CREATE POLICY "Users can view their debug bundles"
  ON storage.objects FOR SELECT TO authenticated
  USING (
    bucket_id = 'support-debug-bundles'
    AND auth.uid()::TEXT = (storage.foldername(name))[1]
  );

DROP POLICY IF EXISTS "Users can delete their debug bundles"
  ON storage.objects;
CREATE POLICY "Users can delete their debug bundles"
  ON storage.objects FOR DELETE TO authenticated
  USING (
    bucket_id = 'support-debug-bundles'
    AND auth.uid()::TEXT = (storage.foldername(name))[1]
  );

COMMENT ON TABLE support_debug_bundle_submissions IS
  'Support-facing metadata for uploaded RhythmServer debug bundle archives';
COMMENT ON COLUMN support_debug_bundle_submissions.summary IS
  'Optional user-entered problem summary attached to the debug bundle';
COMMENT ON COLUMN support_debug_bundle_submissions.bundle_storage_path IS
  'Private Supabase Storage path for the uploaded tar.gz support archive';

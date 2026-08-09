-- A queued support report exists before its asynchronous debug bundle has
-- been built or uploaded. Keep pending/failed rows nullable while preserving
-- the positive-size contract for uploaded bundles.

BEGIN;

ALTER TABLE public.support_debug_bundle_submissions
  ALTER COLUMN bundle_size_bytes DROP NOT NULL;

ALTER TABLE public.support_debug_bundle_submissions
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_size_bytes_check,
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_uploaded_bundle_size_check;

ALTER TABLE public.support_debug_bundle_submissions
  ADD CONSTRAINT support_debug_bundle_submissions_bundle_size_bytes_check
    CHECK (bundle_size_bytes IS NULL OR bundle_size_bytes > 0),
  ADD CONSTRAINT support_debug_bundle_submissions_uploaded_bundle_size_check
    CHECK (bundle_status <> 'uploaded' OR bundle_size_bytes > 0);

COMMENT ON COLUMN public.support_debug_bundle_submissions.bundle_size_bytes IS
  'Positive uploaded archive size; null while no bundle exists or collection is pending/failed';

COMMIT;

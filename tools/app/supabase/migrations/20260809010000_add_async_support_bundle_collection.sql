-- Track server-owned asynchronous support-bundle collection and its
-- privacy-bounded completion summary.

ALTER TABLE public.support_debug_bundle_submissions
  ADD COLUMN IF NOT EXISTS bundle_status TEXT NOT NULL DEFAULT 'none',
  ADD COLUMN IF NOT EXISTS bundle_completion_token_hash TEXT,
  ADD COLUMN IF NOT EXISTS bundle_collection_started_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS bundle_collection_completed_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS bundle_collection_duration_ms BIGINT,
  ADD COLUMN IF NOT EXISTS bundle_schema_version INTEGER,
  ADD COLUMN IF NOT EXISTS bundle_captured_log_count INTEGER,
  ADD COLUMN IF NOT EXISTS bundle_captured_persisted_count INTEGER,
  ADD COLUMN IF NOT EXISTS bundle_generated_file_count INTEGER,
  ADD COLUMN IF NOT EXISTS bundle_missing_persisted_count INTEGER,
  ADD COLUMN IF NOT EXISTS bundle_file_error_count INTEGER,
  ADD COLUMN IF NOT EXISTS bundle_app_log_included BOOLEAN,
  ADD COLUMN IF NOT EXISTS bundle_failure_stage TEXT;

UPDATE public.support_debug_bundle_submissions
SET bundle_status = CASE
  WHEN bundle_storage_path IS NULL THEN 'none'
  ELSE 'uploaded'
END
WHERE bundle_status = 'none';

ALTER TABLE public.support_debug_bundle_submissions
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_status_check,
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_completion_token_hash_check,
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_collection_duration_check,
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_schema_version_check,
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_summary_counts_check,
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_bundle_failure_stage_check;

ALTER TABLE public.support_debug_bundle_submissions
  ADD CONSTRAINT support_debug_bundle_submissions_bundle_status_check
    CHECK (bundle_status IN ('none', 'collecting', 'uploaded', 'failed')),
  ADD CONSTRAINT support_debug_bundle_submissions_completion_token_hash_check
    CHECK (
      bundle_completion_token_hash IS NULL
      OR bundle_completion_token_hash ~ '^[0-9a-f]{64}$'
    ),
  ADD CONSTRAINT support_debug_bundle_submissions_collection_duration_check
    CHECK (
      bundle_collection_duration_ms IS NULL
      OR bundle_collection_duration_ms >= 0
    ),
  ADD CONSTRAINT support_debug_bundle_submissions_bundle_schema_version_check
    CHECK (bundle_schema_version IS NULL OR bundle_schema_version > 0),
  ADD CONSTRAINT support_debug_bundle_submissions_bundle_summary_counts_check
    CHECK (
      COALESCE(bundle_captured_log_count, 0) >= 0
      AND COALESCE(bundle_captured_persisted_count, 0) >= 0
      AND COALESCE(bundle_generated_file_count, 0) >= 0
      AND COALESCE(bundle_missing_persisted_count, 0) >= 0
      AND COALESCE(bundle_file_error_count, 0) >= 0
    ),
  ADD CONSTRAINT support_debug_bundle_submissions_bundle_failure_stage_check
    CHECK (
      bundle_failure_stage IS NULL
      OR bundle_failure_stage ~ '^[a-z0-9_]{1,64}$'
    );

CREATE INDEX IF NOT EXISTS support_debug_bundle_submissions_bundle_status_idx
  ON public.support_debug_bundle_submissions (bundle_status, created_at DESC);

COMMENT ON COLUMN public.support_debug_bundle_submissions.bundle_status IS
  'Private bundle lifecycle independent of report/GitHub review status';
COMMENT ON COLUMN public.support_debug_bundle_submissions.bundle_completion_token_hash IS
  'SHA-256 of the one-time server completion bearer; raw tokens are never stored in cloud metadata';
COMMENT ON COLUMN public.support_debug_bundle_submissions.bundle_schema_version IS
  'Privacy-safe debug bundle manifest schema reported by RhythmServer';
COMMENT ON COLUMN public.support_debug_bundle_submissions.bundle_failure_stage IS
  'Bounded collection failure stage; never a raw exception or endpoint';

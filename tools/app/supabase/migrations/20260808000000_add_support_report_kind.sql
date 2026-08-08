-- Classify app support submissions as bugs or feature requests while keeping
-- every existing and legacy-client row bug-compatible by default.

ALTER TABLE public.support_debug_bundle_submissions
  ADD COLUMN IF NOT EXISTS report_kind TEXT NOT NULL DEFAULT 'bug';

ALTER TABLE public.support_debug_bundle_submissions
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_report_kind_check;

ALTER TABLE public.support_debug_bundle_submissions
  ADD CONSTRAINT support_debug_bundle_submissions_report_kind_check
  CHECK (report_kind IN ('bug', 'feature'));

COMMENT ON COLUMN public.support_debug_bundle_submissions.report_kind IS
  'User-selected GitHub classification for this debug-bundle-backed support report';

COMMENT ON TABLE public.support_debug_bundle_submissions IS
  'Support-facing metadata for bug or feature reports and their private debug bundle archives';

-- Track GitHub issues created for support debug bundle bug reports.

ALTER TABLE support_debug_bundle_submissions
  ADD COLUMN IF NOT EXISTS github_issue_url TEXT,
  ADD COLUMN IF NOT EXISTS github_issue_number INTEGER,
  ADD COLUMN IF NOT EXISTS github_issue_created_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS github_issue_error TEXT;

ALTER TABLE support_debug_bundle_submissions
  DROP CONSTRAINT IF EXISTS support_debug_bundle_submissions_status_check;

ALTER TABLE support_debug_bundle_submissions
  ADD CONSTRAINT support_debug_bundle_submissions_status_check
  CHECK (status IN ('received', 'reported', 'reviewing', 'resolved', 'error'));

CREATE INDEX IF NOT EXISTS support_debug_bundle_submissions_github_issue_number_idx
  ON support_debug_bundle_submissions (github_issue_number)
  WHERE github_issue_number IS NOT NULL;

COMMENT ON COLUMN support_debug_bundle_submissions.github_issue_url IS
  'GitHub issue URL created for this bug report';
COMMENT ON COLUMN support_debug_bundle_submissions.github_issue_number IS
  'GitHub issue number created for this bug report';
COMMENT ON COLUMN support_debug_bundle_submissions.github_issue_error IS
  'Most recent GitHub issue creation error, if automation failed';

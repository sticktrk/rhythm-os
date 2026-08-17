-- Bind support submission metadata to the authenticated user's storage prefix.
-- Staff tooling relies on this relationship before downloading private bundles.

DROP POLICY IF EXISTS "Users can create their debug bundle submissions"
  ON public.support_debug_bundle_submissions;
CREATE POLICY "Users can create their debug bundle submissions"
  ON public.support_debug_bundle_submissions FOR INSERT TO authenticated
  WITH CHECK (
    auth.uid() = user_id
    AND (
      bundle_storage_path IS NULL
      OR auth.uid()::TEXT = (storage.foldername(bundle_storage_path))[1]
    )
  );

COMMENT ON POLICY "Users can create their debug bundle submissions"
  ON public.support_debug_bundle_submissions IS
  'Requires support bundle metadata to reference the submitting user storage prefix.';

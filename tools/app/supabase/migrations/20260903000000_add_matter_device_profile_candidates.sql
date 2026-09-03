-- Bulb Audition reports produce immutable pending candidates beside the
-- currently approved fleet profile. Submissions must never revoke or replace
-- an approved profile before staff review.

CREATE TABLE IF NOT EXISTS public.matter_device_profile_candidates (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  profile_key TEXT NOT NULL,
  candidate_version BIGINT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'approved', 'rejected')),
  profile_payload JSONB NOT NULL,
  source_report_id UUID REFERENCES public.matter_bulb_test_reports(id) ON DELETE CASCADE,
  reviewed_at TIMESTAMPTZ,
  reviewed_by UUID REFERENCES auth.users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (profile_key, candidate_version)
);

CREATE INDEX IF NOT EXISTS matter_device_profile_candidates_pending_idx
  ON public.matter_device_profile_candidates (status, created_at DESC);
CREATE INDEX IF NOT EXISTS matter_device_profile_candidates_profile_idx
  ON public.matter_device_profile_candidates (profile_key, candidate_version DESC);

ALTER TABLE public.matter_device_profile_candidates ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Staff can read Matter device profile candidates"
  ON public.matter_device_profile_candidates;
CREATE POLICY "Staff can read Matter device profile candidates"
  ON public.matter_device_profile_candidates FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can review Matter device profile candidates"
  ON public.matter_device_profile_candidates;
CREATE POLICY "Staff can review Matter device profile candidates"
  ON public.matter_device_profile_candidates FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

CREATE OR REPLACE FUNCTION public.preserve_matter_profile_candidate_evidence()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.profile_key IS DISTINCT FROM OLD.profile_key
    OR NEW.candidate_version IS DISTINCT FROM OLD.candidate_version
    OR NEW.profile_payload IS DISTINCT FROM OLD.profile_payload
    OR NEW.source_report_id IS DISTINCT FROM OLD.source_report_id
    OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
    RAISE EXCEPTION 'Matter profile candidate evidence is immutable'
      USING ERRCODE = '22000';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS matter_device_profile_candidates_preserve_evidence
  ON public.matter_device_profile_candidates;
CREATE TRIGGER matter_device_profile_candidates_preserve_evidence
  BEFORE UPDATE ON public.matter_device_profile_candidates
  FOR EACH ROW EXECUTE FUNCTION public.preserve_matter_profile_candidate_evidence();

DROP TRIGGER IF EXISTS matter_device_profile_candidates_updated_at
  ON public.matter_device_profile_candidates;
CREATE TRIGGER matter_device_profile_candidates_updated_at
  BEFORE UPDATE ON public.matter_device_profile_candidates
  FOR EACH ROW EXECUTE FUNCTION public.update_updated_at_column();

COMMENT ON TABLE public.matter_device_profile_candidates IS
  'Immutable pending Bulb Audition profile versions awaiting staff review; report submission never mutates the approved fleet profile';

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

CREATE OR REPLACE FUNCTION public.approve_matter_profile_candidate(candidate_id UUID)
RETURNS public.published_matter_device_profiles
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
  candidate_row public.matter_device_profile_candidates%ROWTYPE;
  published_row public.published_matter_device_profiles%ROWTYPE;
  reviewer_id UUID := auth.uid();
BEGIN
  IF NOT public.is_rhythm_admin() THEN
    RAISE EXCEPTION 'not authorized'
      USING ERRCODE = '42501';
  END IF;

  SELECT candidate.*
  INTO candidate_row
  FROM public.matter_device_profile_candidates AS candidate
  WHERE candidate.id = $1
  FOR UPDATE;

  IF NOT FOUND THEN
    RAISE EXCEPTION 'Matter profile candidate not found'
      USING ERRCODE = 'P0002';
  END IF;
  IF candidate_row.status <> 'pending' THEN
    RAISE EXCEPTION 'Matter profile candidate has already been reviewed'
      USING ERRCODE = '22000';
  END IF;

  INSERT INTO public.published_matter_device_profiles (
    profile_key,
    schema_version,
    profile_version,
    approved,
    approved_at,
    approved_by,
    manufacturer,
    model,
    device_name,
    matter_vendor_id,
    matter_product_id,
    firmware_version,
    cluster_fingerprint,
    capabilities,
    quirks,
    recommended_control_strategy,
    evidence,
    source_report_id,
    client_report_id
  )
  VALUES (
    candidate_row.profile_key,
    COALESCE((candidate_row.profile_payload ->> 'schema_version')::INTEGER, 1),
    candidate_row.candidate_version,
    TRUE,
    NOW(),
    reviewer_id,
    candidate_row.profile_payload ->> 'manufacturer',
    candidate_row.profile_payload ->> 'model',
    candidate_row.profile_payload ->> 'device_name',
    (candidate_row.profile_payload ->> 'matter_vendor_id')::INTEGER,
    (candidate_row.profile_payload ->> 'matter_product_id')::INTEGER,
    candidate_row.profile_payload ->> 'firmware_version',
    COALESCE(candidate_row.profile_payload -> 'cluster_fingerprint', '{}'::JSONB),
    COALESCE(candidate_row.profile_payload -> 'capabilities', '{}'::JSONB),
    COALESCE(candidate_row.profile_payload -> 'quirks', '{}'::JSONB),
    COALESCE(candidate_row.profile_payload -> 'recommended_control_strategy', '{}'::JSONB),
    COALESCE(candidate_row.profile_payload -> 'evidence', '{}'::JSONB),
    candidate_row.source_report_id,
    candidate_row.profile_payload ->> 'client_report_id'
  )
  ON CONFLICT (profile_key) DO UPDATE
  SET
    schema_version = EXCLUDED.schema_version,
    profile_version = EXCLUDED.profile_version,
    approved = TRUE,
    approved_at = EXCLUDED.approved_at,
    approved_by = EXCLUDED.approved_by,
    manufacturer = EXCLUDED.manufacturer,
    model = EXCLUDED.model,
    device_name = EXCLUDED.device_name,
    matter_vendor_id = EXCLUDED.matter_vendor_id,
    matter_product_id = EXCLUDED.matter_product_id,
    firmware_version = EXCLUDED.firmware_version,
    cluster_fingerprint = EXCLUDED.cluster_fingerprint,
    capabilities = EXCLUDED.capabilities,
    quirks = EXCLUDED.quirks,
    recommended_control_strategy = EXCLUDED.recommended_control_strategy,
    evidence = EXCLUDED.evidence,
    source_report_id = EXCLUDED.source_report_id,
    client_report_id = EXCLUDED.client_report_id,
    updated_at = NOW()
  RETURNING * INTO published_row;

  UPDATE public.matter_device_profile_candidates
  SET
    status = 'approved',
    reviewed_at = NOW(),
    reviewed_by = reviewer_id
  WHERE id = candidate_row.id;

  RETURN published_row;
END;
$$;

REVOKE ALL ON FUNCTION public.approve_matter_profile_candidate(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.approve_matter_profile_candidate(UUID) TO authenticated;

COMMENT ON FUNCTION public.approve_matter_profile_candidate(UUID) IS
  'Admin-only atomic promotion of one immutable Bulb Audition candidate into the approved fleet profile feed';

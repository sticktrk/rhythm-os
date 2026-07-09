-- Atomically add a member after an edge function has verified local device proof.
--
-- This function is intentionally granted only to service_role. Ordinary clients
-- should use the hardened add_home_member RPC, which requires existing home
-- membership. The local-device-proof edge function supplies the extra
-- authorization boundary for new members.

CREATE OR REPLACE FUNCTION public.add_verified_device_home_member(
  home_uuid UUID,
  user_uuid UUID
)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
BEGIN
  UPDATE public.homes
  SET
    member_ids = CASE
      WHEN user_uuid = ANY(member_ids) THEN member_ids
      ELSE array_append(member_ids, user_uuid)
    END,
    updated_at = NOW()
  WHERE id = home_uuid;

  IF NOT FOUND THEN
    RAISE EXCEPTION 'home not found'
      USING ERRCODE = 'P0002';
  END IF;
END;
$$;

REVOKE ALL ON FUNCTION public.add_verified_device_home_member(UUID, UUID)
  FROM PUBLIC;
REVOKE ALL ON FUNCTION public.add_verified_device_home_member(UUID, UUID)
  FROM anon;
REVOKE ALL ON FUNCTION public.add_verified_device_home_member(UUID, UUID)
  FROM authenticated;
GRANT EXECUTE ON FUNCTION public.add_verified_device_home_member(UUID, UUID)
  TO service_role;

COMMENT ON FUNCTION public.add_verified_device_home_member(UUID, UUID) IS
  'Service-role-only helper used after an edge function verifies local Rhythm OS device proof.';

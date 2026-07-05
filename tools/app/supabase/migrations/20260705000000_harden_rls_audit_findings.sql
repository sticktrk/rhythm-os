-- Harden two gaps found during an RLS audit:
--   A. add_home_member/remove_home_member SECURITY DEFINER escalation.
--   B. Support/user views still carrying default anon+authenticated DML grants.
--
-- ============================================================
-- A. Home membership helper functions
-- ============================================================
--
-- add_home_member/remove_home_member were created as SECURITY DEFINER in the
-- initial schema with no authorization check and the default PUBLIC EXECUTE
-- grant. Because they run with owner rights, their UPDATEs bypass the homes
-- RLS policies entirely -- any caller (anon or authenticated) could add
-- themselves to, or evict a member from, ANY home given its UUID, then inherit
-- full membership-based access to that home and its hubs/remote-access/tokens.
--
-- This migration:
--   1. Rebuilds both functions with a pinned search_path and an explicit
--      authorization guard (caller must be a member of the target home, or an
--      enabled Rhythm admin), mirroring set_rhythm_subscription_tier.
--   2. Removes the default PUBLIC/anon EXECUTE grant, leaving only authenticated
--      (still subject to the in-body guard).

CREATE OR REPLACE FUNCTION public.add_home_member(home_uuid UUID, user_uuid UUID)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
BEGIN
  IF NOT (
    public.is_rhythm_admin()
    OR EXISTS (
      SELECT 1 FROM public.homes
      WHERE homes.id = home_uuid
        AND auth.uid() = ANY(homes.member_ids)
    )
  ) THEN
    RAISE EXCEPTION 'not authorized'
      USING ERRCODE = '42501';
  END IF;

  UPDATE public.homes
  SET member_ids = array_append(member_ids, user_uuid)
  WHERE id = home_uuid
    AND NOT (user_uuid = ANY(member_ids));
END;
$$;

CREATE OR REPLACE FUNCTION public.remove_home_member(home_uuid UUID, user_uuid UUID)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
BEGIN
  IF NOT (
    public.is_rhythm_admin()
    OR EXISTS (
      SELECT 1 FROM public.homes
      WHERE homes.id = home_uuid
        AND auth.uid() = ANY(homes.member_ids)
    )
  ) THEN
    RAISE EXCEPTION 'not authorized'
      USING ERRCODE = '42501';
  END IF;

  UPDATE public.homes
  SET member_ids = array_remove(member_ids, user_uuid)
  WHERE id = home_uuid;
END;
$$;

-- Drop the default PUBLIC EXECUTE grant (which also covers anon) and re-grant
-- to authenticated only. The in-body guard still applies on top of this.
REVOKE ALL ON FUNCTION public.add_home_member(UUID, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.add_home_member(UUID, UUID) FROM anon;
GRANT EXECUTE ON FUNCTION public.add_home_member(UUID, UUID) TO authenticated;

REVOKE ALL ON FUNCTION public.remove_home_member(UUID, UUID) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.remove_home_member(UUID, UUID) FROM anon;
GRANT EXECUTE ON FUNCTION public.remove_home_member(UUID, UUID) TO authenticated;

COMMENT ON FUNCTION public.add_home_member(UUID, UUID) IS
  'Adds a user to a home. Caller must already be a member of the home or an enabled Rhythm admin.';
COMMENT ON FUNCTION public.remove_home_member(UUID, UUID) IS
  'Removes a user from a home. Caller must be a member of the home or an enabled Rhythm admin.';

-- ============================================================
-- B. Support/user view grants
-- ============================================================
--
-- These views were meant to be SELECT-only to authenticated. The original
-- migrations locked them with `REVOKE ALL ... FROM PUBLIC`, but Supabase's
-- default privileges grant full DML directly to `anon` and `authenticated`,
-- not via PUBLIC -- so those direct grants survived. Reads are still filtered
-- by each view's is_rhythm_staff()/membership WHERE clause, but because the
-- views run with owner rights and have no WITH CHECK OPTION, the surviving
-- INSERT grant is an unconstrained write path into the base tables. Revoke
-- from the roles directly (as the sensitive base tables already do) and
-- re-grant SELECT to authenticated only.

DO $$
DECLARE
  v TEXT;
  support_views TEXT[] := ARRAY[
    'rhythm_support_hubs',
    'rhythm_support_cloud_snapshots',
    'rhythm_support_customers',
    'rhythm_support_access_grants',
    'rhythm_support_server_light_activity_events',
    'user_support_access_grants',
    'user_server_light_activity_device_tokens'
  ];
BEGIN
  FOREACH v IN ARRAY support_views LOOP
    EXECUTE format('REVOKE ALL ON public.%I FROM PUBLIC', v);
    EXECUTE format('REVOKE ALL ON public.%I FROM anon', v);
    EXECUTE format('REVOKE ALL ON public.%I FROM authenticated', v);
    EXECUTE format('GRANT SELECT ON public.%I TO authenticated', v);
  END LOOP;
END $$;

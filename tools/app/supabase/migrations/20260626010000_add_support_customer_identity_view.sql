-- Support-safe customer identity lookup for the customer support console.
--
-- Staff need human identifiers to find the right home. This view exposes only
-- basic Auth identity fields to enabled Rhythm staff accounts.

DROP VIEW IF EXISTS public.rhythm_support_customers;
CREATE VIEW public.rhythm_support_customers
WITH (security_barrier = true)
AS
SELECT
  users.id AS user_id,
  users.email,
  COALESCE(
    NULLIF(users.raw_user_meta_data ->> 'full_name', ''),
    NULLIF(users.raw_user_meta_data ->> 'name', ''),
    NULLIF(users.raw_user_meta_data ->> 'display_name', '')
  ) AS name,
  users.created_at,
  users.updated_at
FROM auth.users
WHERE public.is_rhythm_staff();

REVOKE ALL ON public.rhythm_support_customers FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_customers TO authenticated;

COMMENT ON VIEW public.rhythm_support_customers IS
  'Support-safe customer identity fields for enabled Rhythm staff accounts';

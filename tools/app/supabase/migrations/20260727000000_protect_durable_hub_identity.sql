-- A regular app upsert must never replace the established physical identity
-- of a cloud hub. Identity promotion/reconciliation is performed by the
-- service-role verified-device-proof flow.

CREATE OR REPLACE FUNCTION public.protect_durable_hub_server_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = public
AS $$
DECLARE
  old_identity TEXT := lower(btrim(OLD.server_instance_id));
  new_identity TEXT := lower(btrim(NEW.server_instance_id));
BEGIN
  IF OLD.type <> 'server'
    OR old_identity IS NULL
    OR old_identity = ''
    OR old_identity LIKE 'endpoint:%'
    OR old_identity IS NOT DISTINCT FROM new_identity
  THEN
    RETURN NEW;
  END IF;

  -- Service-role Edge Functions and SECURITY DEFINER reconciliation RPCs are
  -- the only supported paths for changing an established durable identity.
  IF auth.role() = 'service_role'
    OR current_user IN ('postgres', 'supabase_admin', 'service_role')
  THEN
    RETURN NEW;
  END IF;

  RAISE EXCEPTION
    'Established durable hub identity cannot be changed by an app sync'
    USING ERRCODE = '23514',
      DETAIL = format(
        'hub_id=%s old_server_instance_id=%s new_server_instance_id=%s',
        OLD.id,
        old_identity,
        COALESCE(new_identity, '<null>')
      ),
      HINT = 'Use verified local device proof to reconcile this Box.';
END;
$$;

DROP TRIGGER IF EXISTS protect_durable_hub_server_identity
  ON public.hubs;
CREATE TRIGGER protect_durable_hub_server_identity
BEFORE UPDATE OF server_instance_id ON public.hubs
FOR EACH ROW
EXECUTE FUNCTION public.protect_durable_hub_server_identity();

COMMENT ON FUNCTION public.protect_durable_hub_server_identity() IS
  'Prevents authenticated app upserts from rebinding an established cloud hub to different physical hardware';

-- Short-lived, single-use reviews. Only the authenticated Edge Function's
-- service identity can operate on these rows. Household access is rechecked
-- under the same row locks at preview, confirmation and phone dispatch.
CREATE TABLE public.customer_assistant_reviews (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  actor_id uuid NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  change text NOT NULL CHECK (change IN ('grant_support','revoke_support','remove_member','enable_remote','submit_report','join_home')),
  arguments jsonb NOT NULL,
  support_grant_id uuid,
  remote_dispatched boolean NOT NULL DEFAULT false,
  remote_tunnel_id text,
  revision jsonb NOT NULL,
  state text NOT NULL DEFAULT 'review' CHECK (state IN ('review','awaiting_device','claimed','applied','acknowledged','uncertain','cancelled')),
  expires_at timestamptz NOT NULL DEFAULT (now() + interval '10 minutes')
);
ALTER TABLE public.customer_assistant_reviews ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON public.customer_assistant_reviews FROM PUBLIC, anon, authenticated;
GRANT SELECT, INSERT, UPDATE, DELETE ON public.customer_assistant_reviews TO service_role;
CREATE INDEX customer_assistant_actor_idx ON public.customer_assistant_reviews(actor_id, expires_at);
CREATE INDEX customer_assistant_expiry_idx ON public.customer_assistant_reviews(expires_at);

-- A household member may see the display name a fellow member chose.
-- The caller cannot look up arbitrary Auth users or retrieve email addresses.
CREATE FUNCTION public.customer_assistant_home_members(p_home_id uuid)
RETURNS TABLE(member_id uuid, display_name text, role text, is_self boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
  SELECT m.id, left(nullif(btrim(u.raw_user_meta_data->>'display_name'),''),120),
         CASE WHEN m.id=h.owner_id THEN 'owner' ELSE 'member' END,
         m.id=auth.uid()
  FROM public.homes h
  CROSS JOIN LATERAL (SELECT DISTINCT unnest(h.member_ids || h.owner_id) AS id) m
  LEFT JOIN auth.users u ON u.id=m.id
  WHERE h.id=p_home_id AND auth.uid()=ANY(h.member_ids)
  ORDER BY (m.id=h.owner_id) DESC,m.id
$$;
REVOKE ALL ON FUNCTION public.customer_assistant_home_members(uuid) FROM PUBLIC,anon;
GRANT EXECUTE ON FUNCTION public.customer_assistant_home_members(uuid) TO authenticated;

-- Once a resident has explicitly limited support access, background sync on
-- any of their devices must respect that choice, including older app builds.
CREATE TABLE public.hub_support_review_policy (
  hub_id uuid PRIMARY KEY REFERENCES public.hubs(id) ON DELETE CASCADE,
  revision uuid NOT NULL DEFAULT gen_random_uuid()
);
ALTER TABLE public.hub_support_review_policy ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON public.hub_support_review_policy FROM PUBLIC, anon, authenticated;
GRANT SELECT, INSERT, UPDATE, DELETE ON public.hub_support_review_policy TO service_role;

CREATE FUNCTION public.customer_assistant_review(
  p_actor uuid, p_step text, p_args jsonb, p_id uuid DEFAULT NULL
) RETURNS jsonb LANGUAGE plpgsql SECURITY INVOKER SET search_path = '' AS $$
DECLARE
  r public.customer_assistant_reviews;
  home public.homes;
  hub public.hubs;
  member uuid;
  grant_id uuid;
  grant_updated timestamptz;
  rev jsonb;
  change_type text;
  current_args jsonb;
  answer jsonb;
BEGIN
  IF p_actor IS NULL OR p_step NOT IN ('plan','confirm','claim','complete','cancel','remote_dispatch','remote_provisioned') THEN
    RAISE EXCEPTION 'invalid_request' USING ERRCODE = '22023';
  END IF;
  IF p_step = 'plan' THEN
    PERFORM pg_advisory_xact_lock(hashtextextended(p_actor::text, 7241));
    DELETE FROM public.customer_assistant_reviews WHERE id IN (
      SELECT id FROM public.customer_assistant_reviews WHERE expires_at <= now() LIMIT 64 FOR UPDATE SKIP LOCKED
    );
    IF (SELECT count(*) FROM public.customer_assistant_reviews WHERE actor_id=p_actor AND expires_at>now() AND state IN ('review','awaiting_device','claimed')) >= 16 THEN
      RAISE EXCEPTION 'review_capacity' USING ERRCODE='P0001';
    END IF;
    change_type := p_args->>'change';
    current_args := p_args;
  ELSE
    SELECT * INTO r FROM public.customer_assistant_reviews WHERE id=p_id AND actor_id=p_actor FOR UPDATE;
    IF NOT FOUND OR r.expires_at <= now() THEN RAISE EXCEPTION 'review_expired' USING ERRCODE='P0001'; END IF;
    IF p_step='cancel' THEN
      IF r.state NOT IN ('review','awaiting_device','cancelled') THEN
        RAISE EXCEPTION 'action_already_dispatched' USING ERRCODE='P0001';
      END IF;
      UPDATE public.customer_assistant_reviews SET state='cancelled',arguments=arguments-'report_summary' WHERE id=r.id;
      RETURN jsonb_build_object('status','cancelled','plan_id',r.id);
    END IF;
    IF r.state='cancelled' OR (p_step='confirm' AND p_args->'confirmed' IS DISTINCT FROM 'true'::jsonb)
       OR (p_step='claim' AND r.state <> 'awaiting_device')
       OR (p_step='complete' AND r.state NOT IN ('claimed','applied')) THEN
      RAISE EXCEPTION 'review_state_changed' USING ERRCODE='P0001';
    END IF;
    change_type := r.change;
    current_args := r.arguments;
  END IF;

  IF p_step IN ('confirm','complete') AND r.state='applied' AND change_type='remove_member'
      AND current_args->>'member_id'=p_actor::text THEN
    -- Leaving the home removes the permission that was needed to perform the
    -- action. Its author can still retrieve their own unexpired receipt.
    NULL;
  ELSIF change_type='join_home' THEN
    -- The phone is the only party that can prove possession of an unknown
    -- hub. Reveal nothing about its account before the proof succeeds.
    IF coalesce(length(current_args->>'server_instance_id'),0) NOT BETWEEN 1 AND 128 THEN
      RAISE EXCEPTION 'invalid_identity' USING ERRCODE='22023'; END IF;
    rev := jsonb_build_object('server_instance_id',current_args->>'server_instance_id');
  ELSE
    SELECT * INTO home FROM public.homes WHERE id=(current_args->>'home_id')::uuid FOR UPDATE;
    IF NOT FOUND OR NOT (p_actor=ANY(home.member_ids)) THEN
      RAISE EXCEPTION 'home_access_required' USING ERRCODE='42501'; END IF;
    rev := jsonb_build_object('owner',home.owner_id,'members',to_jsonb(home.member_ids));
    IF change_type='remove_member' THEN
      member := (current_args->>'member_id')::uuid;
      IF member=home.owner_id OR (p_actor<>member AND p_actor<>home.owner_id) THEN
        RAISE EXCEPTION 'membership_authority_required' USING ERRCODE='42501'; END IF;
      IF r.state IS DISTINCT FROM 'applied' AND (member IS NULL OR NOT (member=ANY(home.member_ids))) THEN
        RAISE EXCEPTION 'membership_changed' USING ERRCODE='P0001'; END IF;
    ELSE
      SELECT * INTO hub FROM public.hubs WHERE id=(current_args->>'hub_id')::uuid AND home_id=home.id AND type='server' FOR UPDATE;
      IF NOT FOUND THEN RAISE EXCEPTION 'hub_access_required' USING ERRCODE='42501'; END IF;
      IF hub.server_instance_id IS NULL OR hub.server_instance_id='' THEN RAISE EXCEPTION 'hub_identity_required' USING ERRCODE='P0001'; END IF;
      IF current_args ? 'server_instance_id' AND current_args->>'server_instance_id' IS DISTINCT FROM hub.server_instance_id THEN
        RAISE EXCEPTION 'hub_identity_changed' USING ERRCODE='P0001'; END IF;
      current_args := current_args || jsonb_build_object('server_instance_id',hub.server_instance_id);
      rev := rev || jsonb_build_object('hub',hub.id,'instance',hub.server_instance_id,'enabled',hub.enabled);
      IF change_type IN ('grant_support','revoke_support','enable_remote') THEN
        SELECT g.id,g.updated_at INTO grant_id,grant_updated FROM public.hub_support_access_grants g
          WHERE g.hub_id=hub.id AND scope='beta_admin' AND revoked_at IS NULL FOR UPDATE;
        rev := rev || jsonb_build_object('grant_id',grant_id,'grant_updated',grant_updated,
          'policy',(SELECT revision FROM public.hub_support_review_policy WHERE hub_id=hub.id));
      END IF;
      IF change_type='enable_remote' THEN
        rev := rev || jsonb_build_object('disabled_at',hub.remote_access_disabled_at,'endpoint',hub.remote_endpoint,
          'mapping',(SELECT jsonb_build_array(m.home_id,m.server_instance_id,m.hostname,m.tunnel_id)
            FROM public.hub_remote_access m WHERE m.hub_id=hub.id));
      END IF;
    END IF;
  END IF;

  IF p_step='plan' THEN
    INSERT INTO public.customer_assistant_reviews (actor_id,change,arguments,revision)
      VALUES(p_actor,change_type,current_args,rev) RETURNING * INTO r;
  ELSIF (p_step='confirm' AND r.state <> 'review') OR (p_step='complete' AND r.state='applied') THEN
    -- A repeat can retrieve a receipt but never dispatch a second action.
    NULL;
  ELSIF p_step IN ('remote_dispatch','remote_provisioned') THEN
    -- These steps are called only by the provisioning service. Bind its
    -- request to the same resident, Box and review as the phone's claim.
    IF change_type<>'enable_remote' OR r.state<>'claimed' OR NOT (p_args @> (current_args-'change')) THEN
      RAISE EXCEPTION 'review_changed' USING ERRCODE='P0001';
    END IF;
    IF p_step='remote_dispatch' THEN
      IF r.remote_dispatched OR r.revision IS DISTINCT FROM rev THEN
        RAISE EXCEPTION 'review_stale' USING ERRCODE='P0001';
      END IF;
      -- Fence legacy automatic support grants before starting provisioning.
      INSERT INTO public.hub_support_review_policy (hub_id) VALUES (hub.id)
        ON CONFLICT (hub_id) DO UPDATE SET revision=gen_random_uuid();
      IF hub.remote_access_disabled_at IS NOT NULL THEN
        UPDATE public.hubs SET remote_access_disabled_at=NULL WHERE id=hub.id;
      END IF;
      rev := rev || jsonb_build_object('disabled_at',NULL,
        'policy',(SELECT revision FROM public.hub_support_review_policy WHERE hub_id=hub.id));
      UPDATE public.customer_assistant_reviews SET remote_dispatched=true,revision=rev WHERE id=r.id;
    ELSE
      IF NOT r.remote_dispatched OR r.remote_tunnel_id IS NOT NULL THEN
        RAISE EXCEPTION 'review_state_changed' USING ERRCODE='P0001';
      END IF;
      IF hub.remote_access_disabled_at IS NOT NULL THEN
        RETURN jsonb_build_object('status','disabled_by_user');
      END IF;
      IF r.revision IS DISTINCT FROM rev THEN
        RAISE EXCEPTION 'review_stale' USING ERRCODE='P0001';
      END IF;
      -- Commit the mapping, endpoint and provisioning evidence together. An
      -- older result cannot overwrite a route changed while the vendor ran.
      IF r.revision->'mapping'='null'::jsonb THEN
        INSERT INTO public.hub_remote_access(hub_id,home_id,server_instance_id,hostname,tunnel_id,tunnel_name)
          VALUES(hub.id,home.id,hub.server_instance_id,p_args->>'hostname',p_args->>'tunnel_id',p_args->>'tunnel_name')
          ON CONFLICT DO NOTHING;
      ELSE
        UPDATE public.hub_remote_access m SET tunnel_name=p_args->>'tunnel_name' WHERE m.hub_id=hub.id
          AND jsonb_build_array(m.home_id,m.server_instance_id,m.hostname,m.tunnel_id)=r.revision->'mapping'
          AND m.home_id=home.id AND m.server_instance_id=hub.server_instance_id
          AND m.hostname=p_args->>'hostname' AND m.tunnel_id=p_args->>'tunnel_id';
      END IF;
      IF NOT FOUND THEN RAISE EXCEPTION 'remote_mapping_changed' USING ERRCODE='P0001'; END IF;
      UPDATE public.hubs SET remote_endpoint=jsonb_build_object('host',p_args->>'hostname','port',443,'useSsl',true)
        WHERE id=hub.id;
      UPDATE public.customer_assistant_reviews SET remote_tunnel_id=p_args->>'tunnel_id' WHERE id=r.id;
    END IF;
    RETURN jsonb_build_object('status','ok');
  ELSIF p_step='complete' THEN
    IF (p_args->>'outcome') IS NULL OR p_args->>'outcome' NOT IN ('acknowledged','uncertain') THEN
      RAISE EXCEPTION 'invalid_outcome' USING ERRCODE='22023'; END IF;
    -- A phone can report ambiguity. An acknowledgement must also agree
    -- with the service's durable record for this particular action.
    IF p_args->>'outcome'='acknowledged' AND change_type IN ('remove_member','revoke_support') THEN
      IF r.revision IS DISTINCT FROM rev THEN RAISE EXCEPTION 'review_stale' USING ERRCODE='P0001'; END IF;
      IF change_type='remove_member' THEN
        UPDATE public.homes SET member_ids=array_remove(member_ids, member) WHERE id=home.id;
      ELSE
        INSERT INTO public.hub_support_review_policy (hub_id) VALUES (hub.id)
          ON CONFLICT (hub_id) DO UPDATE SET revision=gen_random_uuid();
        UPDATE public.hub_support_access_grants SET revoked_at=now(),
          encrypted_token='{"version":"support_access_token_revoked_v1"}'::jsonb WHERE id=grant_id;
      END IF;
      p_args:=jsonb_build_object('outcome','applied');
    ELSIF p_args->>'outcome'='acknowledged' AND (CASE change_type
      WHEN 'grant_support' THEN r.support_grant_id IS NOT NULL AND r.support_grant_id=grant_id
      WHEN 'enable_remote' THEN hub.remote_access_disabled_at IS NULL AND EXISTS (
        SELECT 1 FROM public.hub_remote_access m WHERE m.hub_id=hub.id AND m.home_id=home.id
          AND m.server_instance_id=hub.server_instance_id AND m.tunnel_id=r.remote_tunnel_id
          AND hub.remote_endpoint=jsonb_build_object('host',m.hostname,'port',443,'useSsl',true))
      WHEN 'join_home' THEN EXISTS (
        SELECT 1 FROM public.hubs b JOIN public.homes a ON a.id=b.home_id
        WHERE b.type='server' AND b.server_instance_id=current_args->>'server_instance_id' AND p_actor=ANY(a.member_ids)
          AND (NOT current_args ? 'home_id' OR a.id=(current_args->>'home_id')::uuid)
          AND (NOT current_args ? 'hub_id' OR b.id=(current_args->>'hub_id')::uuid))
      WHEN 'submit_report' THEN EXISTS (
        SELECT 1 FROM public.support_debug_bundle_submissions b WHERE b.id=r.id AND b.user_id=p_actor
          AND b.summary=btrim(current_args->>'report_summary') AND (
            ((current_args->>'include_bundle')::boolean AND b.server_hub_id=hub.id::text AND b.bundle_status IN ('collecting','uploaded')) OR
            (NOT (current_args->>'include_bundle')::boolean AND b.bundle_status='none' AND b.bundle_storage_path IS NULL)))
      ELSE false END) IS NOT TRUE THEN
      p_args:=jsonb_build_object('outcome','uncertain');
    END IF;
    UPDATE public.customer_assistant_reviews SET state=p_args->>'outcome', arguments=arguments-'report_summary' WHERE id=r.id RETURNING * INTO r;
  ELSE
    IF r.revision IS DISTINCT FROM rev THEN RAISE EXCEPTION 'review_stale' USING ERRCODE='P0001'; END IF;
    IF p_step='claim' THEN
      UPDATE public.customer_assistant_reviews SET state='claimed', expires_at=greatest(expires_at,now()+interval '10 minutes')
        WHERE id=r.id RETURNING * INTO r;
    ELSE
      UPDATE public.customer_assistant_reviews SET state='awaiting_device' WHERE id=r.id RETURNING * INTO r;
    END IF;
  END IF;
  -- No database rows, device proofs, endpoints, URLs or credentials cross
  -- this boundary. Only the user's bounded arguments are returned.
  answer := r.arguments || jsonb_build_object('status',r.state,'plan_id',r.id,'expires_at',r.expires_at,
       'operation','apply_customer_plan','requires_confirmation',r.state='review');
  IF change_type IN ('grant_support','revoke_support') THEN
    answer := answer || jsonb_build_object('support_scope','beta_admin','previous_grant_active',r.revision->>'grant_id' IS NOT NULL);
  END IF;
  RETURN answer;
END;
$$;
REVOKE ALL ON FUNCTION public.customer_assistant_review(uuid,text,jsonb,uuid) FROM PUBLIC, anon, authenticated;
GRANT EXECUTE ON FUNCTION public.customer_assistant_review(uuid,text,jsonb,uuid) TO service_role;

-- Serialize grant replacement and revocation on the same home/hub rows as the
-- review transaction. The envelope never leaves this function in a receipt.
CREATE FUNCTION public.store_customer_support_grant(
  p_actor uuid, p_hub uuid, p_action text, p_payload jsonb, p_review uuid DEFAULT NULL
) RETURNS jsonb LANGUAGE plpgsql SECURITY INVOKER SET search_path = '' AS $$
DECLARE
  h public.hubs;
  household public.homes;
  previous public.hub_support_access_grants;
  created public.hub_support_access_grants;
  review public.customer_assistant_reviews;
  policy uuid;
BEGIN
  IF p_review IS NOT NULL THEN
    SELECT * INTO review FROM public.customer_assistant_reviews WHERE id=p_review AND actor_id=p_actor FOR UPDATE;
    IF NOT FOUND OR review.change<>'grant_support' OR review.state<>'claimed' OR review.support_grant_id IS NOT NULL
      OR review.expires_at<=now() OR review.arguments->>'hub_id' IS DISTINCT FROM p_hub::text THEN
      RAISE EXCEPTION 'review_required' USING ERRCODE='42501';
    END IF;
  END IF;
  SELECT a.* INTO household FROM public.homes a JOIN public.hubs b ON b.home_id=a.id
    WHERE b.id=p_hub AND p_actor=ANY(a.member_ids) FOR UPDATE OF a;
  IF NOT FOUND THEN RAISE EXCEPTION 'home_access_required' USING ERRCODE='42501'; END IF;
  SELECT * INTO h FROM public.hubs WHERE id=p_hub AND home_id=household.id AND type='server' FOR UPDATE;
  IF NOT FOUND THEN RAISE EXCEPTION 'hub_access_required' USING ERRCODE='42501'; END IF;
  SELECT revision INTO policy FROM public.hub_support_review_policy WHERE hub_id=h.id;
  SELECT * INTO previous FROM public.hub_support_access_grants WHERE hub_id=h.id AND scope='beta_admin' AND revoked_at IS NULL FOR UPDATE;

  IF p_action='grant' THEN
    IF p_review IS NULL AND policy IS NOT NULL THEN RAISE EXCEPTION 'review_required' USING ERRCODE='42501'; END IF;
    IF p_review IS NOT NULL THEN
      IF review.revision IS DISTINCT FROM jsonb_build_object(
        'owner',household.owner_id,'members',to_jsonb(household.member_ids),'hub',h.id,'instance',h.server_instance_id,
        'enabled',h.enabled,'grant_id',previous.id,'grant_updated',previous.updated_at,'policy',policy) THEN
        RAISE EXCEPTION 'review_changed' USING ERRCODE='P0001';
      END IF;
    END IF;
  ELSIF p_action='revoke' THEN
    INSERT INTO public.hub_support_review_policy (hub_id) VALUES (h.id)
      ON CONFLICT (hub_id) DO UPDATE SET revision=gen_random_uuid();
  ELSE
    RAISE EXCEPTION 'invalid_action' USING ERRCODE='22023';
  END IF;
  UPDATE public.hub_support_access_grants SET revoked_at=now(),encrypted_token='{"version":"support_access_token_revoked_v1"}'
    WHERE id=previous.id;
  IF p_action='revoke' THEN
    RETURN jsonb_build_object('status','ok','revoked_count',CASE WHEN previous.id IS NULL THEN 0 ELSE 1 END,
      'token_ids',CASE WHEN previous.id IS NULL THEN '[]'::jsonb ELSE jsonb_build_array(previous.token_id) END);
  END IF;
  INSERT INTO public.hub_support_access_grants(hub_id,home_id,token_id,scope,label,encrypted_token,key_id,granted_by,expires_at)
    VALUES(h.id,household.id,p_payload->>'token_id','beta_admin',p_payload->>'label',p_payload->'encrypted_token',
      p_payload->>'key_id',p_actor,(p_payload->>'expires_at')::timestamptz) RETURNING * INTO created;
  IF p_review IS NOT NULL THEN
    UPDATE public.customer_assistant_reviews SET support_grant_id=created.id WHERE id=p_review;
  END IF;
  RETURN jsonb_build_object('status','ok','grant',jsonb_build_object(
    'id',created.id,'hub_id',h.id,'home_id',h.home_id,'token_id',created.token_id,'label',created.label,
    'scope',created.scope,'key_id',created.key_id,'granted_by',p_actor,'expires_at',created.expires_at,
    'revoked_at',created.revoked_at,'created_at',created.created_at,'updated_at',created.updated_at));
END;
$$;
REVOKE ALL ON FUNCTION public.store_customer_support_grant(uuid,uuid,text,jsonb,uuid) FROM PUBLIC, anon, authenticated;
GRANT EXECUTE ON FUNCTION public.store_customer_support_grant(uuid,uuid,text,jsonb,uuid) TO service_role;

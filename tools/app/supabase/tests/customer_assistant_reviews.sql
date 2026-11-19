-- Run with psql against a disposable migrated database.
BEGIN;
DO $test$
DECLARE
  actor uuid:=gen_random_uuid(); member uuid:=gen_random_uuid(); other_user uuid:=gen_random_uuid();
  home uuid:=gen_random_uuid(); hub uuid:=gen_random_uuid(); plan jsonb; receipt jsonb; arguments jsonb; target jsonb; policy_before uuid;
BEGIN
  INSERT INTO auth.users(id) VALUES (actor),(member),(other_user);
  INSERT INTO public.homes(id,name,owner_id,member_ids) VALUES(home,'Fixture home',actor,ARRAY[actor,member]);
  INSERT INTO public.hubs(id,home_id,type,name,endpoint,server_instance_id) VALUES(hub,home,'server','Fixture hub','{}','fixture-instance');
  UPDATE auth.users SET raw_user_meta_data='{"display_name":"Housemate","email":"private@example.invalid"}' WHERE id=member;
  PERFORM set_config('request.jwt.claim.sub',actor::text,true);
  SET LOCAL ROLE authenticated;
  ASSERT (SELECT display_name='Housemate' AND role='member' AND NOT is_self FROM public.customer_assistant_home_members(home) m WHERE m.member_id=member);
  ASSERT (SELECT count(*)=2 FROM public.customer_assistant_home_members(home));
  PERFORM set_config('request.jwt.claim.sub',other_user::text,true);
  ASSERT NOT EXISTS (SELECT 1 FROM public.customer_assistant_home_members(home));
  RESET ROLE;
  SET LOCAL ROLE service_role;
  ASSERT NOT has_table_privilege('authenticated','public.hub_remote_access','update');
  ASSERT NOT has_function_privilege('authenticated','public.customer_assistant_review(uuid,text,jsonb,uuid)','execute');
  ASSERT NOT has_table_privilege('authenticated','public.customer_assistant_reviews','select');
  arguments:=jsonb_build_object('change','remove_member','home_id',home,'member_id',member);
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  BEGIN
    PERFORM public.customer_assistant_review(other_user,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'independent actor used proposal' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  BEGIN
    PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":false}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'unconfirmed removal' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  UPDATE public.homes SET member_ids=array_append(member_ids,other_user) WHERE id=home;
  BEGIN
    PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'stale removal' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  ASSERT (SELECT member=ANY(member_ids) FROM public.homes WHERE id=home);
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  receipt:=public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  ASSERT receipt->>'status'='applied';
  ASSERT NOT (SELECT member=ANY(member_ids) FROM public.homes WHERE id=home);
  ASSERT public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid)->>'status'='applied';

  arguments:=jsonb_build_object('change','grant_support','home_id',home,'hub_id',hub);
  BEGIN
    PERFORM public.customer_assistant_review(member,'plan',arguments);
    RAISE EXCEPTION 'removed member retained access' USING ERRCODE='XX000';
  EXCEPTION WHEN insufficient_privilege THEN NULL; END;
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  receipt:=public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  ASSERT receipt->>'status'='awaiting_device';
  UPDATE public.hubs SET server_instance_id='changed-instance' WHERE id=hub;
  BEGIN
    PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'changed physical hub accepted' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  ASSERT public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid)->>'status'='claimed';
  BEGIN
    PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'device action dispatched twice' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  -- A background sync from another phone cannot undo a reviewed revocation.
  arguments:=jsonb_build_object('change','revoke_support','home_id',home,'hub_id',hub);
  receipt:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(receipt->>'plan_id')::uuid);
  BEGIN
    PERFORM public.store_customer_support_grant(actor,hub,'grant','{"token_id":"fixture","encrypted_token":{}}');
    RAISE EXCEPTION 'legacy automatic grant ignored resident choice' USING ERRCODE='XX000';
  EXCEPTION WHEN insufficient_privilege THEN NULL; END;
  BEGIN
    PERFORM public.store_customer_support_grant(actor,hub,'grant','{"token_id":"fixture","encrypted_token":{}}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'previous claim ignored more recent revocation' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  arguments:=arguments||'{"change":"grant_support"}';
  receipt:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(receipt->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(receipt->>'plan_id')::uuid);
  ASSERT (public.store_customer_support_grant(actor,hub,'grant','{"token_id":"fixture","encrypted_token":{}}',(receipt->>'plan_id')::uuid)->'grant'->>'token_id')='fixture';
  BEGIN
    PERFORM public.store_customer_support_grant(actor,hub,'grant','{"token_id":"replacement","encrypted_token":{}}',(receipt->>'plan_id')::uuid);
    RAISE EXCEPTION 'grant authorization consumed twice' USING ERRCODE='XX000';
  EXCEPTION WHEN insufficient_privilege THEN NULL; END;
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(receipt->>'plan_id')::uuid)->>'status'='acknowledged';
  -- An app or MCP caller cannot report a positive grant without the actual row.
  receipt:=public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid);
  ASSERT receipt->>'status'='uncertain';
  -- A member who leaves can recover their own successful receipt after a lost response.
  arguments:=jsonb_build_object('change','remove_member','home_id',home,'member_id',other_user);
  plan:=public.customer_assistant_review(other_user,'plan',arguments);
  PERFORM public.customer_assistant_review(other_user,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  ASSERT NOT (SELECT other_user=ANY(member_ids) FROM public.homes WHERE id=home);
  ASSERT public.customer_assistant_review(other_user,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid)->>'status'='applied';

  SELECT revision INTO policy_before FROM public.hub_support_review_policy WHERE hub_id=hub;
  target:=jsonb_build_object('home_id',home,'hub_id',hub,'server_instance_id','changed-instance');
  arguments:=jsonb_build_object('change','enable_remote','home_id',home,'hub_id',hub);
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  -- A second phone disabled remote access while the first was checking its Box.
  UPDATE public.hubs SET remote_access_disabled_at=now() WHERE id=hub;
  BEGIN
    PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'bootstrap overrode a newer disable' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  ASSERT (SELECT remote_access_disabled_at IS NOT NULL FROM public.hubs WHERE id=hub);
  ASSERT (SELECT revision=policy_before FROM public.hub_support_review_policy WHERE hub_id=hub);
  ASSERT NOT (SELECT remote_dispatched FROM public.customer_assistant_reviews WHERE id=(plan->>'plan_id')::uuid);

  INSERT INTO public.hub_remote_access(hub_id,home_id,hostname,tunnel_id,tunnel_name,server_instance_id)
    VALUES(hub,home,'fixture.invalid','fixture-route','fixture','changed-instance');
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  ASSERT (SELECT remote_access_disabled_at IS NULL FROM public.hubs WHERE id=hub);
  BEGIN
    PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'bootstrap dispatched twice' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  -- The pre-existing route does not prove the dispatched setup succeeded.
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid)->>'status'='uncertain';
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_provisioned',
    target || '{"tunnel_id":"fixture-route","hostname":"fixture.invalid","tunnel_name":"fixture"}',(plan->>'plan_id')::uuid);
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid)->>'status'='acknowledged';

  -- Losing the review while the vendor is busy cannot install or replace a database route.
  DELETE FROM public.hub_remote_access WHERE hub_id=hub;
  UPDATE public.hubs SET remote_endpoint=NULL WHERE id=hub;
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  UPDATE public.customer_assistant_reviews SET expires_at=now() WHERE id=(plan->>'plan_id')::uuid;
  BEGIN
    PERFORM public.customer_assistant_review(actor,'remote_provisioned',
      target || '{"tunnel_id":"expired-route","hostname":"fixture.invalid","tunnel_name":"fixture"}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'expired provisioning result accepted' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  ASSERT NOT EXISTS (SELECT 1 FROM public.hub_remote_access WHERE hub_id=hub);

  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  INSERT INTO public.hub_remote_access(hub_id,home_id,hostname,tunnel_id,tunnel_name,server_instance_id)
    VALUES(hub,home,'fixture.invalid','newer-route','fixture','changed-instance');
  BEGIN
    PERFORM public.customer_assistant_review(actor,'remote_provisioned',
      target || '{"tunnel_id":"stale-route","hostname":"fixture.invalid","tunnel_name":"fixture"}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'superseded provisioning result accepted' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  ASSERT (SELECT tunnel_id='newer-route' FROM public.hub_remote_access WHERE hub_id=hub);
  ASSERT (SELECT remote_endpoint IS NULL FROM public.hubs WHERE id=hub);

  DELETE FROM public.hub_remote_access WHERE hub_id=hub;
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_provisioned',
    target || '{"tunnel_id":"fixture-route","hostname":"fixture.invalid","tunnel_name":"fixture"}',(plan->>'plan_id')::uuid);
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid)->>'status'='acknowledged';

  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  receipt:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(receipt->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(receipt->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(receipt->>'plan_id')::uuid);
  BEGIN
    PERFORM public.customer_assistant_review(actor,'remote_provisioned',
      target || '{"tunnel_id":"stale-route","hostname":"fixture.invalid","tunnel_name":"fixture"}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'superseded worker committed its endpoint' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  -- A disable that lands during vendor provisioning still wins.
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'remote_dispatch',target,(plan->>'plan_id')::uuid);
  UPDATE public.hubs SET remote_access_disabled_at=now(), remote_endpoint=NULL WHERE id=hub;
  receipt:=public.customer_assistant_review(actor,'remote_provisioned',
    target || '{"tunnel_id":"fixture-route","hostname":"fixture.invalid","tunnel_name":"fixture"}',(plan->>'plan_id')::uuid);
  ASSERT receipt->>'status'='disabled_by_user';
  ASSERT (SELECT remote_endpoint IS NULL FROM public.hubs WHERE id=hub);
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid)->>'status'='uncertain';

  arguments:=jsonb_build_object('change','submit_report','home_id',home,'hub_id',hub,'report_summary','fixture','include_bundle',false);
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  BEGIN
    PERFORM public.customer_assistant_review(other_user,'cancel','{}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'another actor cancelled the plan' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  ASSERT public.customer_assistant_review(actor,'cancel','{}',(plan->>'plan_id')::uuid)->>'status'='cancelled';
  ASSERT NOT (SELECT r.arguments ? 'report_summary' FROM public.customer_assistant_reviews r WHERE r.id=(plan->>'plan_id')::uuid);
  BEGIN
    PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
    RAISE EXCEPTION 'another phone claimed a discarded report' USING ERRCODE='XX000';
  EXCEPTION WHEN SQLSTATE 'P0001' THEN NULL; END;
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid)->>'status'='uncertain';
  plan:=public.customer_assistant_review(actor,'plan',arguments);
  PERFORM public.customer_assistant_review(actor,'confirm','{"confirmed":true}',(plan->>'plan_id')::uuid);
  PERFORM public.customer_assistant_review(actor,'claim','{}',(plan->>'plan_id')::uuid);
  INSERT INTO public.support_debug_bundle_submissions(id,user_id,summary) VALUES((plan->>'plan_id')::uuid,actor,'fixture');
  ASSERT public.customer_assistant_review(actor,'complete','{"outcome":"acknowledged"}',(plan->>'plan_id')::uuid)->>'status'='acknowledged';
END;
$test$;
ROLLBACK;

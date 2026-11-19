-- Run with psql against a disposable migrated database.
BEGIN;
DO $test$
DECLARE
  actor uuid:=gen_random_uuid(); member uuid:=gen_random_uuid(); other_user uuid:=gen_random_uuid();
  home uuid:=gen_random_uuid(); hub uuid:=gen_random_uuid(); plan jsonb; receipt jsonb; arguments jsonb;
BEGIN
  INSERT INTO auth.users(id) VALUES (actor),(member),(other_user);
  INSERT INTO public.homes(id,name,owner_id,member_ids) VALUES(home,'Fixture home',actor,ARRAY[actor,member]);
  INSERT INTO public.hubs(id,home_id,type,name,endpoint,server_instance_id) VALUES(hub,home,'server','Fixture hub','{}','fixture-instance');
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
  arguments:=jsonb_build_object('change','submit_report','home_id',home,'hub_id',hub,'report_summary','fixture','include_bundle',false);
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

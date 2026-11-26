begin;
do $test$
declare
  first_user uuid := gen_random_uuid();
  second_user uuid := gen_random_uuid();
  home uuid := gen_random_uuid();
  hub uuid := gen_random_uuid();
  device_token text := repeat('ab', 32);
  old_registration uuid;
  role_name text;
  privilege_name text;
begin
  foreach role_name in array array['anon', 'authenticated'] loop
    foreach privilege_name in array array['select','insert','update','delete','truncate','references','trigger'] loop
      assert not has_table_privilege(role_name,'public.push_devices',privilege_name);
      assert not has_table_privilege(role_name,'public.hub_attention_rings',privilege_name);
    end loop;
    assert not has_function_privilege(role_name,'public.reserve_hub_attention_ring(uuid)','execute');
  end loop;
  foreach role_name in array array['anon','service_role','authenticated'] loop
    assert has_function_privilege(role_name,'public.register_push_device(text,text,text,text)','execute') = (role_name='authenticated');
    assert has_function_privilege(role_name,'public.unregister_push_device(text)','execute') = (role_name='authenticated');
  end loop;
  assert has_function_privilege('service_role','public.reserve_hub_attention_ring(uuid)','execute');

  insert into auth.users(id) values (first_user), (second_user);
  perform set_config('request.jwt.claim.sub',first_user::text,true);
  set local role authenticated;
  perform public.register_push_device('ios','production',device_token,'app.fixture');
  reset role;
  select id into strict old_registration from public.push_devices where user_id=first_user and push_devices.token=device_token;
  perform set_config('request.jwt.claim.sub',second_user::text,true);
  set local role authenticated;
  perform public.register_push_device('ios','development',device_token,'app.fixture');
  reset role;
  assert not exists (select from public.push_devices where id=old_registration or user_id=first_user);
  assert (select count(*)=1 from public.push_devices where user_id=second_user and environment='development');

  perform set_config('request.jwt.claim.sub',first_user::text,true);
  perform public.unregister_push_device(device_token);
  assert (select count(*)=1 from public.push_devices where user_id=second_user);
  perform set_config('request.jwt.claim.sub',second_user::text,true);
  for n in 1..12 loop
    perform public.register_push_device('ios','production',lpad(to_hex(n),64,'0'),'app.fixture');
  end loop;
  assert (select count(*)=10 from public.push_devices where user_id=second_user);
  assert not exists (select from public.push_devices where push_devices.token in (device_token,lpad('1',64,'0'),lpad('2',64,'0')));
  perform public.unregister_push_device(lpad('c',64,'0'));
  assert (select count(*)=9 from public.push_devices where user_id=second_user);
  delete from auth.users where id=second_user;
  assert not exists (select from public.push_devices where user_id=second_user);

  insert into public.homes(id,name,owner_id,member_ids) values (home,'Fixture',first_user,array[first_user]);
  insert into public.hubs(id,home_id,name,type,endpoint) values (hub,home,'Fixture','server','{}');
  for n in 1..6 loop assert public.reserve_hub_attention_ring(hub); end loop;
  assert not public.reserve_hub_attention_ring(hub);
  update public.hub_attention_rings set ring_times[1]=clock_timestamp()-interval '10 minutes' where hub_id=hub;
  assert public.reserve_hub_attention_ring(hub);
  assert not public.reserve_hub_attention_ring(hub);
end
$test$;
rollback;

begin;
do $$
declare
  actor uuid := '00000000-0000-4000-8000-000000000081';
begin
  insert into auth.users(id) values (actor);
  for turn in 1..60 loop
    if not public.reserve_lighting_chat_turn(actor) then raise exception 'Early rejection'; end if;
  end loop;
  if public.reserve_lighting_chat_turn(actor) then raise exception 'Quota exceeded'; end if;
  update public.lighting_chat_usage set usage_day=usage_day-1 where user_id=actor;
  if not public.reserve_lighting_chat_turn(actor) then raise exception 'Missing rollover'; end if;
  if (select turns from public.lighting_chat_usage where user_id=actor) <> 1 then raise exception 'Counter rollover'; end if;
  if has_function_privilege('authenticated','public.reserve_lighting_chat_turn(uuid)','execute')
    or has_function_privilege('anon','public.reserve_lighting_chat_turn(uuid)','execute')
    or has_table_privilege('authenticated','public.lighting_chat_usage','select')
    or not has_function_privilege('service_role','public.reserve_lighting_chat_turn(uuid)','execute')
    then raise exception 'Unexpected counter privileges'; end if;
  delete from auth.users where id=actor;
  if exists(select from public.lighting_chat_usage where user_id=actor) then raise exception 'Counter survived deletion'; end if;
end $$;
rollback;

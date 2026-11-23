-- One bounded counter per signed-in resident. Chat content is never stored.
create table public.lighting_chat_usage (
  user_id uuid primary key references auth.users(id) on delete cascade,
  usage_day date not null,
  turns integer not null check (turns between 1 and 60)
);
alter table public.lighting_chat_usage enable row level security;
revoke all on public.lighting_chat_usage from public, anon, authenticated;

create function public.reserve_lighting_chat_turn(p_actor uuid) returns boolean
language plpgsql security definer set search_path = '' as $$
declare
  today_utc date := (now() at time zone 'utc')::date;
begin
  insert into public.lighting_chat_usage as usage (user_id, usage_day, turns)
    values (p_actor, today_utc, 1)
  on conflict (user_id) do update set
    turns = case when usage.usage_day = today_utc then usage.turns + 1 else 1 end,
    usage_day = today_utc
  where usage.usage_day < today_utc or (usage.usage_day = today_utc and usage.turns < 60);
  return found;
end;
$$;
revoke all on function public.reserve_lighting_chat_turn(uuid) from public, anon, authenticated;
grant execute on function public.reserve_lighting_chat_turn(uuid) to service_role;

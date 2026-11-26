create table public.push_devices (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  platform text not null check (platform = 'ios'),
  environment text not null check (environment in ('development', 'production')),
  token text not null unique check (length(token) between 2 and 1024 and token ~ '^[0-9a-f]+$'),
  app_id text not null check (length(app_id) between 1 and 255),
  updated_at timestamptz not null default clock_timestamp()
);
create index push_devices_user_id_idx on public.push_devices(user_id);
alter table public.push_devices enable row level security;
revoke all on public.push_devices from public, anon, authenticated;
grant select, delete on public.push_devices to service_role;

create function public.register_push_device(p_platform text, p_environment text, p_token text, p_app_id text)
returns void language plpgsql security definer set search_path = '' as $$
declare
  actor uuid := auth.uid();
  device_token text := lower(p_token);
  account_lock integer;
begin
  if actor is null then raise insufficient_privilege; end if;
  -- Serialize each token's transfers. Lock both accounts in a consistent
  -- order so two full accounts swapping devices cannot deadlock on eviction.
  perform pg_advisory_xact_lock(1381257302, hashtext(device_token));
  for account_lock in
    select hashtext(actor::text)
    union select hashtext(user_id::text) from public.push_devices where token = device_token
    order by 1
  loop
    perform pg_advisory_xact_lock(1381257301, account_lock);
  end loop;
  delete from public.push_devices where token = device_token;
  insert into public.push_devices(user_id, platform, environment, token, app_id)
    values (actor, p_platform, p_environment, device_token, p_app_id);
  delete from public.push_devices where id in (
    select id from public.push_devices where user_id = actor order by updated_at desc, id desc offset 10
  );
end;
$$;
revoke all on function public.register_push_device(text,text,text,text) from public, anon, authenticated, service_role;
grant execute on function public.register_push_device(text,text,text,text) to authenticated;

create function public.unregister_push_device(p_token text)
returns void language plpgsql security definer set search_path = '' as $$
begin
  perform pg_advisory_xact_lock(1381257302, hashtext(lower(p_token)));
  delete from public.push_devices where user_id = auth.uid() and token = lower(p_token);
end;
$$;
revoke all on function public.unregister_push_device(text) from public, anon, authenticated, service_role;
grant execute on function public.unregister_push_device(text) to authenticated;

-- One row per hub, with at most six timestamps. A rolling window prevents
-- a burst of twelve notifications at a fixed counter's rollover.
create table public.hub_attention_rings (
  hub_id uuid primary key references public.hubs(id) on delete cascade,
  ring_times timestamptz[] not null default '{}'
    check (cardinality(ring_times) <= 6)
);
alter table public.hub_attention_rings enable row level security;
revoke all on public.hub_attention_rings from public, anon, authenticated;

create function public.reserve_hub_attention_ring(p_hub_id uuid)
returns boolean language plpgsql security definer set search_path = '' as $$
declare
  recent timestamptz[];
  ring_time timestamptz;
begin
  insert into public.hub_attention_rings(hub_id) values (p_hub_id) on conflict do nothing;
  select ring_times into strict recent from public.hub_attention_rings where hub_id = p_hub_id for update;
  ring_time := clock_timestamp();
  recent := array(select t from unnest(recent) t where t > ring_time - interval '10 minutes');
  if cardinality(recent) >= 6 then return false; end if;
  update public.hub_attention_rings set ring_times = array_append(recent, ring_time) where hub_id = p_hub_id;
  return true;
end;
$$;
revoke all on function public.reserve_hub_attention_ring(uuid) from public, anon, authenticated;
grant execute on function public.reserve_hub_attention_ring(uuid) to service_role;

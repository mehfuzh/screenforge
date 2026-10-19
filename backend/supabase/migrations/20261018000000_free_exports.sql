-- Free exports used per device, keyed by a SHA-256 of the machine's hardware UUID so a
-- reinstall (or a deleted support folder) cannot reset the count. The app keeps a local copy
-- for offline use and takes the higher of the two.
create table if not exists public.free_exports (
  device_id text primary key check (device_id ~ '^[0-9a-f]{64}$'),
  used integer not null default 0 check (used >= 0),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
alter table public.free_exports enable row level security; -- service role only

-- One more export for a device; creates the row on first use. Returns the new total.
create or replace function public.record_free_export(p_device text)
returns integer
language sql
security definer
set search_path = public
as $$
  insert into public.free_exports (device_id, used)
  values (p_device, 1)
  on conflict (device_id) do update
    set used = public.free_exports.used + 1, updated_at = now()
  returning used;
$$;
revoke all on function public.record_free_export(text) from public, anon, authenticated;

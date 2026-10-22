-- Device activation for license keys. A license is active on one computer at a time, recorded
-- here. A key is only handed to that computer, signed for it, and only after it proved it can
-- read the license email: `license-activate` emails a 6-digit code, and entering it moves the
-- license to that computer (the previous one's row is deleted; its key stops working the next
-- time it checks in). Devices are a SHA-256 of the machine's hardware UUID (the raw UUID never
-- leaves the machine).
create table if not exists public.license_activations (
  email text not null check (email = lower(email)),
  device_id text not null check (device_id ~ '^[0-9a-f]{64}$'),
  created_at timestamptz not null default now(),
  last_seen_at timestamptz not null default now(),
  primary key (email, device_id)
);
alter table public.license_activations enable row level security; -- service role only

-- Pending codes: only a hash is stored; a code expires after 15 minutes and allows 5 tries.
create table if not exists public.license_codes (
  email text not null check (email = lower(email)),
  device_id text not null check (device_id ~ '^[0-9a-f]{64}$'),
  code_hash text not null,
  expires_at timestamptz not null,
  attempts integer not null default 0,
  created_at timestamptz not null default now(),
  primary key (email, device_id)
);
alter table public.license_codes enable row level security; -- service role only

-- Every code sent, for rate limiting (per email: 60 s apart, 5 an hour).
create table if not exists public.license_code_sends (
  email text not null,
  sent_at timestamptz not null default now()
);
create index if not exists license_code_sends_email_idx on public.license_code_sends (email, sent_at);
alter table public.license_code_sends enable row level security; -- service role only

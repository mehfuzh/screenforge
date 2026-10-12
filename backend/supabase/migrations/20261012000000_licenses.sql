create table if not exists public.licenses (
  id uuid primary key default gen_random_uuid(),
  email text not null,
  plan text not null check (plan in ('monthly', 'lifetime')),
  stripe_customer_id text,
  stripe_subscription_id text,
  issued_at date not null default current_date,
  expires_at date,
  key text not null,
  created_at timestamptz not null default now()
);
create index if not exists licenses_email_idx on public.licenses (lower(email));
alter table public.licenses enable row level security; -- service role only

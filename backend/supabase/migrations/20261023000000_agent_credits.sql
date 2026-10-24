-- Agent credits. 1 credit = US$0.01 of Claude usage at list price; the `agent` function spends
-- them per request. Accounts are license emails (lower case), the same identity as
-- `licenses` / `license_activations`: a computer spends the credits of the license it is
-- activated for.
--
--  * monthly: Pro's allowance, set (not added) at every paid invoice, so it resets each
--    billing period; spent first.
--  * topup:   bought in packs, never expires; spent once the monthly credits are gone.
--  * trial:   a few credits per computer without a license, to try the agent.
--
-- Every change is a row in `credit_ledger`; Stripe-driven ones carry the Stripe object id in
-- `ref`, unique per kind, so a webhook delivered twice changes nothing the second time.
create table if not exists public.credit_accounts (
  email text primary key check (email = lower(email)),
  monthly numeric(12, 4) not null default 0,
  topup numeric(12, 4) not null default 0,
  monthly_until timestamptz,
  updated_at timestamptz not null default now()
);
alter table public.credit_accounts enable row level security; -- service role only

create table if not exists public.credit_ledger (
  id bigint generated always as identity primary key,
  email text not null,
  kind text not null check (kind in ('monthly', 'topup', 'refund', 'usage', 'adjust')),
  credits numeric(12, 4) not null,
  ref text,
  created_at timestamptz not null default now()
);
create unique index if not exists credit_ledger_ref on public.credit_ledger (kind, ref) where ref is not null;
create index if not exists credit_ledger_email on public.credit_ledger (email, created_at);
alter table public.credit_ledger enable row level security; -- service role only

create table if not exists public.credit_trials (
  device_id text primary key check (device_id ~ '^[0-9a-f]{64}$'),
  used numeric(12, 4) not null default 0,
  updated_at timestamptz not null default now()
);
alter table public.credit_trials enable row level security; -- service role only

-- Balance of an account (zeros when it has none).
create or replace function public.credits_balance(p_email text)
returns table (monthly numeric, topup numeric, monthly_until timestamptz)
language sql stable security definer set search_path = public
as $$
  select coalesce(a.monthly, 0), coalesce(a.topup, 0), a.monthly_until
  from (select 1) one left join public.credit_accounts a on a.email = lower(p_email);
$$;

-- Pro's monthly credits for the period paid by invoice `p_ref`: replaces what was left of the
-- last period. Returns false when this invoice was already granted.
create or replace function public.credits_grant_monthly(p_email text, p_credits numeric, p_until timestamptz, p_ref text)
returns boolean
language plpgsql security definer set search_path = public
as $$
begin
  insert into public.credit_ledger (email, kind, credits, ref) values (lower(p_email), 'monthly', p_credits, p_ref);
  insert into public.credit_accounts (email, monthly, monthly_until) values (lower(p_email), p_credits, p_until)
  on conflict (email) do update set monthly = p_credits, monthly_until = p_until, updated_at = now();
  return true;
exception when unique_violation then
  return false;
end;
$$;

-- A top-up pack paid by checkout session `p_ref`. Returns false when already added.
create or replace function public.credits_topup(p_email text, p_credits numeric, p_ref text)
returns boolean
language plpgsql security definer set search_path = public
as $$
begin
  insert into public.credit_ledger (email, kind, credits, ref) values (lower(p_email), 'topup', p_credits, p_ref);
  insert into public.credit_accounts (email, topup) values (lower(p_email), p_credits)
  on conflict (email) do update set topup = public.credit_accounts.topup + p_credits, updated_at = now();
  return true;
exception when unique_violation then
  return false;
end;
$$;

-- A refunded top-up (charge `p_ref`): takes back up to what is left of the top-up credits.
create or replace function public.credits_refund(p_email text, p_credits numeric, p_ref text)
returns boolean
language plpgsql security definer set search_path = public
as $$
declare taken numeric;
begin
  select least(topup, p_credits) into taken from public.credit_accounts where email = lower(p_email) for update;
  insert into public.credit_ledger (email, kind, credits, ref) values (lower(p_email), 'refund', -coalesce(taken, 0), p_ref);
  update public.credit_accounts set topup = topup - coalesce(taken, 0), updated_at = now() where email = lower(p_email);
  return true;
exception when unique_violation then
  return false;
end;
$$;

-- Spend `p_credits` after a request: monthly first (only while its period lasts), then the
-- top-up. The last request may take the balance slightly below zero; the next one is refused.
create or replace function public.credits_spend(p_email text, p_credits numeric, p_ref text)
returns table (monthly numeric, topup numeric)
language plpgsql security definer set search_path = public
as $$
declare m numeric; t numeric; until timestamptz; from_monthly numeric;
begin
  select a.monthly, a.topup, a.monthly_until into m, t, until from public.credit_accounts a where a.email = lower(p_email) for update;
  if not found then
    insert into public.credit_accounts (email) values (lower(p_email));
    m := 0; t := 0;
  end if;
  if until is null or until < now() then
    m := 0;
  end if;
  from_monthly := greatest(least(m, p_credits), 0);
  m := m - from_monthly;
  t := t - (p_credits - from_monthly);
  update public.credit_accounts a set monthly = m, topup = t, updated_at = now() where a.email = lower(p_email);
  insert into public.credit_ledger (email, kind, credits, ref) values (lower(p_email), 'usage', -p_credits, p_ref);
  return query select m, t;
end;
$$;

-- Trial credits used by a computer without a license; returns the new total used.
create or replace function public.credits_trial_spend(p_device text, p_credits numeric)
returns numeric
language sql security definer set search_path = public
as $$
  insert into public.credit_trials (device_id, used) values (p_device, p_credits)
  on conflict (device_id) do update set used = public.credit_trials.used + p_credits, updated_at = now()
  returning used;
$$;

revoke all on function public.credits_balance(text) from public, anon, authenticated;
revoke all on function public.credits_grant_monthly(text, numeric, timestamptz, text) from public, anon, authenticated;
revoke all on function public.credits_topup(text, numeric, text) from public, anon, authenticated;
revoke all on function public.credits_refund(text, numeric, text) from public, anon, authenticated;
revoke all on function public.credits_spend(text, numeric, text) from public, anon, authenticated;
revoke all on function public.credits_trial_spend(text, numeric) from public, anon, authenticated;

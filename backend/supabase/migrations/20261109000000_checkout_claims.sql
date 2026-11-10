-- Activation without typing an email. Upgrading in the app sends a one-time secret made on the
-- computer (only its SHA-256 travels: in the Checkout Session's metadata) with the computer's
-- device id. Once paid, stripe-webhook records the license here; the app polls
-- `license-claim` with the secret and is activated on that computer, no code needed.
-- `fresh` is false when the checkout email already had a license before this purchase: then
-- paying is no proof of owning the address (anyone can type someone else's email at checkout),
-- so the claim asks for the emailed code as before instead of moving that account's activation.
create table if not exists public.checkout_claims (
  claim_hash text primary key check (claim_hash ~ '^[0-9a-f]{64}$'),
  device_id text not null check (device_id ~ '^[0-9a-f]{64}$'),
  email text not null check (email = lower(email)),
  fresh boolean not null,
  created_at timestamptz not null default now(),
  claimed_at timestamptz
);
alter table public.checkout_claims enable row level security; -- service role only

-- Unsplash search replies shared by every user of the app, so a search anyone already made is
-- answered without spending the app's Unsplash rate limit (one key for all users). Keyed by
-- the normalised query and page; the `unsplash` function refreshes rows older than its TTL and
-- falls back to a stale row when Unsplash refuses (rate limited or down).
create table if not exists public.unsplash_cache (
  key text primary key,
  body jsonb not null,
  fetched_at timestamptz not null default now()
);
create index if not exists unsplash_cache_fetched_at on public.unsplash_cache (fetched_at);
alter table public.unsplash_cache enable row level security; -- service role only

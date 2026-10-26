-- Jamendo search replies shared by every user of the app (Audio → Find music), like
-- `unsplash_cache`: keyed by the normalised query, page and instrumental filter; the `jamendo`
-- function refreshes rows older than its TTL and falls back to a stale row when Jamendo refuses.
create table if not exists public.jamendo_cache (
  key text primary key,
  body jsonb not null,
  fetched_at timestamptz not null default now()
);
create index if not exists jamendo_cache_fetched_at on public.jamendo_cache (fetched_at);
alter table public.jamendo_cache enable row level security; -- service role only

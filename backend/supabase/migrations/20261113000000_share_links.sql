-- Cloud share links (Pro): a video uploaded to the R2 bucket and the page at
-- screenforge.studio/s/<id> that plays it. Written only by the `share` function (service role).
create table if not exists public.share_links (
  id text primary key,                       -- the link's slug (random, unguessable)
  email text not null,                       -- owner: the license email of the uploading computer
  device_id text not null,                   -- computer that uploaded it
  title text not null default '',
  bytes bigint not null,
  content_type text not null default 'video/mp4',
  width integer,
  height integer,
  duration_ms integer,
  has_poster boolean not null default false,
  -- `uploading` until the app reports the upload done and the object's size is checked
  status text not null default 'uploading' check (status in ('uploading', 'ready')),
  views integer not null default 0,
  created_at timestamptz not null default now()
);
create index if not exists share_links_email_idx on public.share_links (lower(email), created_at desc);
alter table public.share_links enable row level security; -- service role only

-- one view, counted by the share page
create or replace function public.share_view(p_id text) returns void
language sql security definer set search_path = public as $$
  update public.share_links set views = views + 1 where id = p_id and status = 'ready';
$$;
revoke all on function public.share_view(text) from public, anon, authenticated;

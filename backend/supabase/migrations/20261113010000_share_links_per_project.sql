-- One link per project: sharing a project again replaces the video behind its link (same URL).
--   project        the project's id (ScreenForge project file), with the owner's email unique
--   pending_bytes  size of a replacement upload in flight; `bytes` changes once it is complete
--   rev            bumped by every completed upload, so the page and the CDN get the new video
alter table public.share_links add column if not exists project text;
alter table public.share_links add column if not exists pending_bytes bigint;
alter table public.share_links add column if not exists rev integer not null default 0;
alter table public.share_links add column if not exists updated_at timestamptz not null default now();
create unique index if not exists share_links_project_idx on public.share_links (email, project) where project is not null;

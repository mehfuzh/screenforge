-- My voice clone cleanup: clones nobody read with for a while are deleted at ElevenLabs to free
-- the account's voice slots. The row (name, consent) stays with voice_id null ("put away");
-- the app makes the clone again from the recordings it keeps when the voice is next used.
alter table public.voice_clones add column if not exists last_used_at timestamptz not null default now();
alter table public.voice_clones alter column voice_id drop not null;
create index if not exists voice_clones_last_used on public.voice_clones (last_used_at) where voice_id is not null;

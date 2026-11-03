-- "My voice": the ElevenLabs Instant Voice Clone a Pro user made of their own voice (Narration
-- → My voice), one per license email, so narration and re-voicing can read in it. The audio
-- is not kept here: ElevenLabs holds the clone, we keep its id and when the user agreed that
-- it is their voice. Deleting it in the app deletes the clone at ElevenLabs and this row.
create table if not exists public.voice_clones (
  email text primary key,
  voice_id text not null,
  name text not null,
  seconds numeric not null,
  consent_text text not null,
  consent_at timestamptz not null default now(),
  created_at timestamptz not null default now()
);
alter table public.voice_clones enable row level security; -- service role only

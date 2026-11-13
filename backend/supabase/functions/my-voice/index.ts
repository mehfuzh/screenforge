// "My voice": a Pro user's own voice, cloned by ElevenLabs (Instant Voice Clone) from recordings
// they made, so Narration, the agent and Re-voice can speak in it ("my_voice" in `voice` and
// `voice-enhance`). One clone per license email; the user must say it is their own voice.
//   { device, action: "status" }                                → { voice: Clone | null }
//   { device, action: "create", consent: true, samples, clean } → { voice: Clone }
//       samples: base64 WAVs (16-bit PCM mono), 10 s to 3 min in all; clean: take out
//       background noise first. Making it again replaces the clone.
//   { device, action: "rename", name }                          → { voice: Clone }
//   { device, action: "delete" }                                → { ok: true }
//   name (create and rename): what the user calls it, up to 40 characters ("My voice" if empty)
//   Clone = { name, seconds, createdAt, parked }; parked: put away by the cleanup (no clone at
//   ElevenLabs right now), made again by a "create" from the recordings the app kept
// Cleanup: clones unused for 30 days are put away, and when ElevenLabs has no slot left the
// least recently used one is, so the account's voice slots hold only voices in use.
// Making a clone is free at ElevenLabs (it takes one of the account's voice slots), so nothing
// is charged here; speaking in it is paid like the other ElevenLabs voices.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { decode } from "https://deno.land/std@0.190.0/encoding/base64.ts";
import { createClient, SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { payerFor } from "../_shared/credits.ts";
import { cors, elevenLabsFailed, fail, gate, isDevice, json, wavSeconds } from "../_shared/elevenlabs.ts";

/** What the user agrees to, stored with the clone. */
const CONSENT = "This is my own voice. I agree to ElevenLabs making a clone of it for my ScreenForge narration, and I will not use it to imitate anyone else.";
const MIN_SECONDS = 10;
const MAX_SECONDS = 180;
const MAX_SAMPLES = 10;
const MAX_NAME = 40;

/** The user's name for their voice: one line, trimmed, "My voice" when missing. */
const voiceName = (name: unknown) => {
  const n = typeof name === "string" ? [...name.replace(/[\u0000-\u001f\u007f]/g, " ").trim()].slice(0, MAX_NAME).join("").trim() : "";
  return n || "My voice";
};

// deno-lint-ignore no-explicit-any
const clone = (row: any) => (row ? { name: row.name, seconds: Number(row.seconds), createdAt: row.created_at, parked: !row.voice_id } : null);

/** Clones not read with for this long are put away (deleted at ElevenLabs, row kept). */
const IDLE_DAYS = 30;

/** Put away `rows`' clones: delete them at ElevenLabs, keep the rows with no voice. */
// deno-lint-ignore no-explicit-any
async function park(db: SupabaseClient, apiKey: string, rows: any[]) {
  for (const row of rows) {
    await deleteAtElevenLabs(apiKey, row.voice_id);
    const { error } = await db.from("voice_clones").update({ voice_id: null }).eq("email", row.email).eq("voice_id", row.voice_id);
    if (error) console.error("park", error.message);
  }
}

/** Put away clones unused for [`IDLE_DAYS`] (a few per call, so no request waits long). */
async function parkIdle(db: SupabaseClient, apiKey: string) {
  const before = new Date(Date.now() - IDLE_DAYS * 86_400_000).toISOString();
  const { data } = await db.from("voice_clones").select("email, voice_id").not("voice_id", "is", null).lt("last_used_at", before).order("last_used_at").limit(5);
  if (data?.length) await park(db, apiKey, data);
}

/** Make room for one more clone: put away the least recently used one that is not `email`'s. */
async function parkLeastUsed(db: SupabaseClient, apiKey: string, email: string): Promise<boolean> {
  const { data } = await db.from("voice_clones").select("email, voice_id").not("voice_id", "is", null).neq("email", email).order("last_used_at").limit(1);
  if (!data?.length) return false;
  await park(db, apiKey, data);
  return true;
}

/** A name for the clone at ElevenLabs that does not give away the user's email. */
async function cloneName(email: string): Promise<string> {
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(email)));
  return `ScreenForge user ${[...hash.slice(0, 6)].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

async function deleteAtElevenLabs(apiKey: string, voiceId: string) {
  const r = await fetch(`https://api.elevenlabs.io/v1/voices/${voiceId}`, { method: "DELETE", headers: { "xi-api-key": apiKey } }).catch(() => null);
  // already gone is fine
  if (r && !r.ok && r.status !== 404) console.error("elevenlabs delete", r.status, (await r.text().catch(() => "")).slice(0, 300));
}

async function current(db: SupabaseClient, email: string) {
  const { data } = await db.from("voice_clones").select("*").eq("email", email).maybeSingle();
  return data;
}

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const apiKey = Deno.env.get("ELEVENLABS_API_KEY");
  if (!apiKey) return fail("ElevenLabs is not configured on the server yet.", 503);
  const { device, action, consent, samples, clean, name } = await req.json().catch(() => ({}));
  if (!isDevice(device)) return fail("device required", 400);

  if (action === "status") {
    // no gate: a computer without Pro simply has no voice to show
    const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
    const payer = await payerFor(db, device);
    return json({ voice: payer.email ? clone(await current(db, payer.email.toLowerCase())) : null });
  }

  const gated = await gate(device);
  if (gated instanceof Response) return gated;
  const { db, payer } = gated;
  if (!payer.email) return fail("My voice needs ScreenForge Pro activated on this computer.", 402, { code: "pro_required" });
  const email = payer.email.toLowerCase();

  if (action === "delete") {
    const row = await current(db, email);
    if (row) {
      if (row.voice_id) await deleteAtElevenLabs(apiKey, row.voice_id);
      const { error } = await db.from("voice_clones").delete().eq("email", email);
      if (error) return fail(error.message, 500);
    }
    return json({ ok: true });
  }

  if (action === "rename") {
    const row = await current(db, email);
    if (!row) return fail("You have not made your voice yet. Create it under Narration → My voice.", 404, { code: "no_clone" });
    const { data, error } = await db.from("voice_clones").update({ name: voiceName(name) }).eq("email", email).select("*").single();
    if (error) return fail(error.message, 500);
    return json({ voice: clone(data) });
  }

  if (action !== "create") return fail("action must be status, create, rename or delete", 400);
  // ElevenLabs clones are switched off (flagged clones cannot be verified for app users); a
  // voice made on the user's computer replaces them. Delete and status still work.
  if (Deno.env.get("MY_VOICE_CLOUD") !== "on") return fail("My voice is not available right now. Pick another voice for your narration.", 503, { code: "my_voice_off" });
  if (consent !== true) return fail("Confirm that this is your own voice first.", 400);
  if (!Array.isArray(samples) || samples.length === 0 || samples.length > MAX_SAMPLES) return fail(`Send 1 to ${MAX_SAMPLES} recordings.`, 400);
  const form = new FormData();
  let seconds = 0;
  for (const [i, s] of samples.entries()) {
    let wav: Uint8Array;
    try {
      wav = decode(String(s));
    } catch {
      return fail("samples must be base64", 400);
    }
    const len = wavSeconds(wav);
    if (typeof len === "string") return fail(len, 400);
    seconds += len;
    form.append("files", new Blob([wav as Uint8Array<ArrayBuffer>], { type: "audio/wav" }), `sample-${i + 1}.wav`);
  }
  if (seconds < MIN_SECONDS) return fail(`Your recordings are ${Math.round(seconds)} s; ElevenLabs needs at least ${MIN_SECONDS} s of your voice (1–2 minutes sounds best).`, 400);
  if (seconds > MAX_SECONDS + 1) return fail(`Send at most ${MAX_SECONDS / 60} minutes of recordings.`, 400);
  form.append("name", await cloneName(email));
  form.append("description", "A ScreenForge user's own voice, cloned with their consent for their narration.");
  form.append("remove_background_noise", clean ? "true" : "false");

  await parkIdle(db, apiKey);
  const add = () =>
    fetch("https://api.elevenlabs.io/v1/voices/add", { method: "POST", headers: { "xi-api-key": apiKey }, body: form }).catch((e) => {
      console.error("elevenlabs", e);
      return null;
    });
  let r = await add();
  let detail = "";
  if (r && !r.ok && [400, 403].includes(r.status)) {
    detail = await r.text().catch(() => "");
    // no voice slot left: put away the least recently used clone and try once more
    if (/limit|slot|quota/i.test(detail) && (await parkLeastUsed(db, apiKey, email))) {
      r = await add();
      detail = r && !r.ok && [400, 403].includes(r.status) ? await r.text().catch(() => "") : "";
    }
  }
  if (r && !r.ok && [400, 403].includes(r.status)) {
    console.error("elevenlabs voices/add", r.status, detail.slice(0, 500));
    if (/limit|slot|quota/i.test(detail)) return fail("ScreenForge cannot make more voices right now. Please try again later.", 503);
    return fail("ElevenLabs could not make a voice from these recordings. Use recordings of only you speaking, with little background noise.", 422);
  }
  if (!r || !r.ok) return elevenLabsFailed(r, "make your voice");
  const { voice_id, requires_verification } = await r.json();
  if (typeof voice_id !== "string") return fail("ElevenLabs returned no voice.", 502);
  // a clone ElevenLabs flagged cannot be read until verified, which only its account owner can
  // do: do not keep it (the user's earlier voice, if any, stays)
  if (requires_verification === true) {
    console.error("elevenlabs voices/add: requires verification", voice_id);
    await deleteAtElevenLabs(apiKey, voice_id);
    return fail("ElevenLabs flagged these recordings and needs to verify the voice before it can be used, so it was not made. Try again with other recordings of only you speaking, in a quiet room.", 422, { code: "voice_blocked" });
  }

  // the new clone replaces the old one
  const old = await current(db, email);
  const row = { email, voice_id, name: voiceName(name), seconds: Math.round(seconds * 10) / 10, consent_text: CONSENT, consent_at: new Date().toISOString(), created_at: new Date().toISOString(), last_used_at: new Date().toISOString() };
  const { error } = await db.from("voice_clones").upsert(row);
  if (error) {
    await deleteAtElevenLabs(apiKey, voice_id);
    return fail(error.message, 500);
  }
  if (old?.voice_id && old.voice_id !== voice_id) await deleteAtElevenLabs(apiKey, old.voice_id);
  return json({ voice: clone(row) });
});

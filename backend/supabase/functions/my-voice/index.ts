// "My voice": a Pro user's own voice, cloned by ElevenLabs (Instant Voice Clone) from recordings
// they made, so Narration, the agent and Re-voice can speak in it ("my_voice" in `voice` and
// `voice-enhance`). One clone per license email; the user must say it is their own voice.
//   { device, action: "status" }                                → { voice: Clone | null }
//   { device, action: "create", consent: true, samples, clean } → { voice: Clone }
//       samples: base64 WAVs (16-bit PCM mono), 10 s to 3 min in all; clean: take out
//       background noise first. Making it again replaces the clone.
//   { device, action: "delete" }                                → { ok: true }
//   Clone = { name, seconds, createdAt }
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

// deno-lint-ignore no-explicit-any
const clone = (row: any) => (row ? { name: row.name, seconds: Number(row.seconds), createdAt: row.created_at } : null);

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
  const { device, action, consent, samples, clean } = await req.json().catch(() => ({}));
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
      await deleteAtElevenLabs(apiKey, row.voice_id);
      const { error } = await db.from("voice_clones").delete().eq("email", email);
      if (error) return fail(error.message, 500);
    }
    return json({ ok: true });
  }

  if (action !== "create") return fail("action must be status, create or delete", 400);
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

  const r = await fetch("https://api.elevenlabs.io/v1/voices/add", { method: "POST", headers: { "xi-api-key": apiKey }, body: form }).catch((e) => {
    console.error("elevenlabs", e);
    return null;
  });
  if (r && !r.ok && [400, 403].includes(r.status)) {
    const detail = await r.text().catch(() => "");
    console.error("elevenlabs voices/add", r.status, detail.slice(0, 500));
    if (/limit|slot|quota/i.test(detail)) return fail("ScreenForge cannot make more voices right now. Please try again later.", 503);
    return fail("ElevenLabs could not make a voice from these recordings. Use recordings of only you speaking, with little background noise.", 422);
  }
  if (!r || !r.ok) return elevenLabsFailed(r, "make your voice");
  const { voice_id } = await r.json();
  if (typeof voice_id !== "string") return fail("ElevenLabs returned no voice.", 502);

  // the new clone replaces the old one
  const old = await current(db, email);
  const row = { email, voice_id, name: "My voice", seconds: Math.round(seconds * 10) / 10, consent_text: CONSENT, consent_at: new Date().toISOString(), created_at: new Date().toISOString() };
  const { error } = await db.from("voice_clones").upsert(row);
  if (error) {
    await deleteAtElevenLabs(apiKey, voice_id);
    return fail(error.message, 500);
  }
  if (old && old.voice_id !== voice_id) await deleteAtElevenLabs(apiKey, old.voice_id);
  return json({ voice: clone(row) });
});

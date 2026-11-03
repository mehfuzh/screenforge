// ElevenLabs voices for Narration and the agent, so the ElevenLabs key never ships in the app: it
// is the ELEVENLABS_API_KEY secret here.
// { device, voice, text, speed } → { audio: base64 16-bit PCM mono at 24 kHz, alignment, sf_credits }
//   `voice` is one of VOICES (the app's ids, "el_brian" …; the ElevenLabs voice ids stay in
//   _shared/elevenlabs.ts so they can change without an app release), or "my_voice", the
//   user's own clone (my-voice); `alignment` is
//   ElevenLabs' character timing, which the app turns into word timing for captions.
//
// Paid in agent credits like the agent (see _shared/elevenlabs.ts): it costs what ElevenLabs
// charges for its characters at list price, 1 credit = US$0.01, taken after the audio is made.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { charge, cors, elevenLabsFailed, fail, gate, isDevice, isVoice, json, summary, voiceIdFor } from "../_shared/elevenlabs.ts";

const MODEL = Deno.env.get("ELEVENLABS_MODEL") ?? "eleven_v4";
/** ElevenLabs' list price for MODEL, US$ per 1,000 characters (v4: 0.08, v4 Turbo: 0.04). */
const USD_PER_1K_CHARS = Number(Deno.env.get("ELEVENLABS_USD_PER_1K_CHARS") ?? "0.08");
/** Longest text read at once (one narration take). */
const MAX_CHARS = 5000;

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const apiKey = Deno.env.get("ELEVENLABS_API_KEY");
  if (!apiKey) return fail("ElevenLabs voices are not configured on the server yet.", 503);
  const { device, voice, text, speed } = await req.json().catch(() => ({}));
  if (!isDevice(device)) return fail("device required", 400);
  if (!isVoice(voice)) return fail(`unknown voice ${voice}`, 400);
  const script = typeof text === "string" ? text.trim() : "";
  if (!script) return fail("text required", 400);
  if (script.length > MAX_CHARS) return fail(`A take can be at most ${MAX_CHARS.toLocaleString("en-US")} characters; split it into shorter ones.`, 400);

  const gated = await gate(device);
  if (gated instanceof Response) return gated;
  const { db, payer } = gated;
  const voiceId = await voiceIdFor(db, payer, voice);
  if (voiceId instanceof Response) return voiceId;

  // ElevenLabs takes 0.7–1.2
  const pace = Math.min(Math.max(Number(speed) || 1, 0.7), 1.2);
  const r = await fetch(`https://api.elevenlabs.io/v1/text-to-speech/${voiceId}/with-timestamps?output_format=pcm_24000`, {
    method: "POST",
    headers: { "xi-api-key": apiKey, "Content-Type": "application/json" },
    body: JSON.stringify({ text: script, model_id: MODEL, voice_settings: { speed: pace } }),
  }).catch((e) => {
    console.error("elevenlabs", e);
    return null;
  });
  if (!r || !r.ok) return elevenLabsFailed(r, "read it");
  const out = await r.json();

  const usd = (script.length / 1000) * USD_PER_1K_CHARS;
  const credits = await charge(db, device, payer, usd, `voice:${r.headers.get("request-id") ?? crypto.randomUUID()}`);
  return json({ audio: out.audio_base64, alignment: out.alignment ?? null, credits, sf_credits: summary(payer) });
});

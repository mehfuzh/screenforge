// A voice-over the user recorded, made better by ElevenLabs, keeping the user's own delivery,
// timing and accent:
//   "clean"   — Voice Isolator: background noise, echo and hum taken out; still the user's voice.
//   "revoice" — Voice Changer: the same performance in one of the ElevenLabs voices (VOICES,
//               or "my_voice": a clean studio version of the user's own), with the
//               background noise taken out too.
// { device, mode, voice?, audio: base64 WAV (16-bit PCM mono) } → { audio: base64 MP3, credits, sf_credits }
//
// The app sends a take in pieces of a minute or less (split where it is quiet), so a request
// stays small and well inside ElevenLabs' 5-minute limit; it puts the pieces back together.
// Paid in agent credits like the narration voices (see _shared/elevenlabs.ts): ElevenLabs bills
// both by audio length (1,000 characters a minute), and so does this, by the WAV's length.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { decode, encode } from "https://deno.land/std@0.190.0/encoding/base64.ts";
import { charge, cors, elevenLabsFailed, fail, gate, isDevice, isVoice, json, summary, voiceIdFor, wavSeconds } from "../_shared/elevenlabs.ts";

/** Voice Changer model: multilingual (29 languages) keeps accents better than the English one. */
const STS_MODEL = Deno.env.get("ELEVENLABS_STS_MODEL") ?? "eleven_multilingual_sts_v2";
/** What a minute of audio costs us, US$ (1,000 characters at the narration's list price). */
const USD_PER_MINUTE = Number(Deno.env.get("ELEVENLABS_USD_PER_AUDIO_MINUTE") ?? "0.08");
/** Longest piece taken at once (the app sends about 40 s). */
const MAX_SECONDS = 120;

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const apiKey = Deno.env.get("ELEVENLABS_API_KEY");
  if (!apiKey) return fail("ElevenLabs is not configured on the server yet.", 503);
  const { device, mode, voice, audio } = await req.json().catch(() => ({}));
  if (!isDevice(device)) return fail("device required", 400);
  if (mode !== "clean" && mode !== "revoice") return fail("mode must be clean or revoice", 400);
  if (mode === "revoice" && !isVoice(voice)) return fail(`unknown voice ${voice}`, 400);
  if (typeof audio !== "string" || !audio) return fail("audio required", 400);
  let wav: Uint8Array;
  try {
    wav = decode(audio);
  } catch {
    return fail("audio must be base64", 400);
  }
  const seconds = wavSeconds(wav);
  if (typeof seconds === "string") return fail(seconds, 400);
  if (seconds < 0.3) return fail("The piece is too short.", 400);
  if (seconds > MAX_SECONDS) return fail(`A piece can be at most ${MAX_SECONDS} seconds.`, 400);

  const gated = await gate(device);
  if (gated instanceof Response) return gated;
  const { db, payer } = gated;
  let voiceId: string | undefined;
  if (mode === "revoice") {
    const id = await voiceIdFor(db, payer, voice);
    if (id instanceof Response) return id;
    voiceId = id;
  }

  const form = new FormData();
  form.append("audio", new Blob([wav as Uint8Array<ArrayBuffer>], { type: "audio/wav" }), "take.wav");
  let url: string;
  if (mode === "clean") {
    url = "https://api.elevenlabs.io/v1/audio-isolation";
  } else {
    url = `https://api.elevenlabs.io/v1/voice-changer/${voiceId}?output_format=mp3_44100_128`;
    form.append("model_id", STS_MODEL);
    form.append("remove_background_noise", "true");
  }
  const r = await fetch(url, { method: "POST", headers: { "xi-api-key": apiKey }, body: form }).catch((e) => {
    console.error("elevenlabs", e);
    return null;
  });
  if (!r || !r.ok) return elevenLabsFailed(r, mode === "clean" ? "clean up the voice-over" : "re-voice the voice-over");
  const mp3 = await r.arrayBuffer();

  const usd = (seconds / 60) * USD_PER_MINUTE;
  const credits = await charge(db, device, payer, usd, `voice-enhance:${r.headers.get("request-id") ?? crypto.randomUUID()}`);
  return json({ audio: encode(mp3), credits, sf_credits: summary(payer) });
});

// ElevenLabs voices for Narration and the agent, so the ElevenLabs key never ships in the app: it
// is the ELEVENLABS_API_KEY secret here.
// { device, voice, text, speed } → { audio: base64 16-bit PCM mono at 24 kHz, alignment, sf_credits }
//   `voice` is one of VOICES (the app's ids, "el_brian" …; the ElevenLabs voice ids stay here so
//   they can change without an app release); `alignment` is ElevenLabs' character timing, which
//   the app turns into word timing for captions.
//
// Paid in agent credits (_shared/credits.ts) like the agent: a reading needs a positive balance
// and Pro (or staff, or trial credits); it costs what ElevenLabs charges for its characters at
// list price, 1 credit = US$0.01, taken after the audio is made. Its cost also counts towards
// the agent's daily caps (agent_usage), which are a backstop on everything we pay for.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { MONTHLY_CREDITS, Payer, payerFor, remaining, summary, TRIAL_CREDITS } from "../_shared/credits.ts";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });
const fail = (error: string, status: number, extra: Record<string, unknown> = {}) => json({ error, ...extra }, status);

/** The app's ids → ElevenLabs' premade voices; keep the ids in step with sf_tts::CLOUD_VOICES. */
const VOICES: Record<string, string> = {
  el_brian: "nPczCjzI2devNBz1zQrb",
  el_george: "JBFqnCBsd6RMkjVDRZzb",
  el_eric: "cjVigY5qzO86Huf0OWal",
  el_daniel: "onwK4e9ZLuTAKqWW03F9",
  el_sarah: "EXAVITQu4vr4xnSDxMaL",
  el_matilda: "XrExE9yKIg1WjnnlVkGX",
  el_alice: "Xb7hH8MSUJpSbSDYk0k2",
  el_jessica: "cgSgspJ2msm6clMCkdW9",
};
const MODEL = Deno.env.get("ELEVENLABS_MODEL") ?? "eleven_v4";
/** ElevenLabs' list price for MODEL, US$ per 1,000 characters (v4: 0.08, v4 Turbo: 0.04). */
const USD_PER_1K_CHARS = Number(Deno.env.get("ELEVENLABS_USD_PER_1K_CHARS") ?? "0.08");
/** Longest text read at once (one narration take). */
const MAX_CHARS = 5000;
const DEVICE_DAILY_USD = Number(Deno.env.get("AGENT_DEVICE_DAILY_USD") ?? "0");
const TOTAL_DAILY_USD = Number(Deno.env.get("AGENT_TOTAL_DAILY_USD") ?? "20");

const outOfCredits = (p: Payer) =>
  p.pro
    ? "This month's credits are used up. Top up to keep using ElevenLabs voices, or pick a voice that runs on this computer."
    : `You're out of credits. Top up, or upgrade to Pro for ${MONTHLY_CREDITS.toLocaleString("en-US")} credits a month.`;

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const apiKey = Deno.env.get("ELEVENLABS_API_KEY");
  if (!apiKey) return fail("ElevenLabs voices are not configured on the server yet.", 503);
  const { device, voice, text, speed } = await req.json().catch(() => ({}));
  if (typeof device !== "string" || !/^[0-9a-f]{64}$/.test(device)) return fail("device required", 400);
  const voiceId = VOICES[String(voice)];
  if (!voiceId) return fail(`unknown voice ${voice}`, 400);
  const script = typeof text === "string" ? text.trim() : "";
  if (!script) return fail("text required", 400);
  if (script.length > MAX_CHARS) return fail(`A take can be at most ${MAX_CHARS.toLocaleString("en-US")} characters; split it into shorter ones.`, 400);

  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const { data: today, error: usageError } = await db.rpc("agent_usage_today", { p_device: device }).maybeSingle();
  if (usageError) return fail(usageError.message, 500);
  // deno-lint-ignore no-explicit-any
  const spent = today as any;
  if (DEVICE_DAILY_USD > 0 && Number(spent?.device_usd ?? 0) >= DEVICE_DAILY_USD) return fail("Today's allowance on this computer is used up. It starts again tomorrow.", 402);
  if (TOTAL_DAILY_USD > 0 && Number(spent?.total_usd ?? 0) >= TOTAL_DAILY_USD) return fail("ElevenLabs voices are very busy today. Please try again tomorrow.", 503);
  const payer = await payerFor(db, device);
  if (!payer.allowed) {
    return fail(`ElevenLabs voices are part of Pro: US$20/month for ${MONTHLY_CREDITS.toLocaleString("en-US")} credits a month, shared with the agent. Already on Pro? Activate it with your email in Export.`, 402, { code: "pro_required", sf_credits: summary(payer) });
  }
  if (remaining(payer) <= 0) return fail(outOfCredits(payer), 402, { code: "no_credits", sf_credits: summary(payer) });

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
  if (!r || !r.ok) {
    const detail = r ? await r.text().catch(() => "") : "";
    console.error("elevenlabs", r?.status, detail.slice(0, 500));
    if (r?.status === 429) return fail("Too many readings right now. Wait a moment and try again.", 429);
    return fail("ElevenLabs could not read it right now. Try again in a moment.", 502);
  }
  const out = await r.json();

  const usd = (script.length / 1000) * USD_PER_1K_CHARS;
  const { error } = await db.rpc("agent_usage_add", { p_device: device, p_cost: usd });
  if (error) console.error("agent_usage_add", error.message);
  const credits = usd * 100;
  if (payer.email) {
    const ref = `voice:${r.headers.get("request-id") ?? crypto.randomUUID()}`;
    const { data, error: spendError } = await db.rpc("credits_spend", { p_email: payer.email, p_credits: credits, p_ref: ref }).maybeSingle();
    if (spendError) console.error("credits_spend", spendError.message);
    // deno-lint-ignore no-explicit-any
    const b = data as any;
    if (b) Object.assign(payer, { monthly: Number(b.monthly), topup: Number(b.topup) });
  } else {
    const { data, error: trialError } = await db.rpc("credits_trial_spend", { p_device: device, p_credits: credits });
    if (trialError) console.error("credits_trial_spend", trialError.message);
    payer.trial = Math.max(TRIAL_CREDITS - Number(data ?? 0), 0);
  }
  return json({ audio: out.audio_base64, alignment: out.alignment ?? null, credits, sf_credits: summary(payer) });
});

// What `voice` (narration read from a script) and `voice-enhance` (a recorded voice-over cleaned
// up or re-voiced) share: the app's ElevenLabs voice ids, and the gate and charge every
// ElevenLabs request goes through. Paid in agent credits (credits.ts): a request needs Pro (or
// staff, or trial credits) and a positive balance, its cost is taken after ElevenLabs answers,
// and it counts towards the agent's daily caps (agent_usage), the backstop on everything we pay.
import { createClient, SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { MONTHLY_CREDITS, Payer, payerFor, remaining, summary, TRIAL_CREDITS } from "./credits.ts";

export const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
export const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });
export const fail = (error: string, status: number, extra: Record<string, unknown> = {}) => json({ error, ...extra }, status);

/** The app's ids → ElevenLabs' premade voices; keep the ids in step with sf_tts::CLOUD_VOICES. */
export const VOICES: Record<string, string> = {
  el_brian: "nPczCjzI2devNBz1zQrb",
  el_george: "JBFqnCBsd6RMkjVDRZzb",
  el_eric: "cjVigY5qzO86Huf0OWal",
  el_daniel: "onwK4e9ZLuTAKqWW03F9",
  el_sarah: "EXAVITQu4vr4xnSDxMaL",
  el_matilda: "XrExE9yKIg1WjnnlVkGX",
  el_alice: "Xb7hH8MSUJpSbSDYk0k2",
  el_jessica: "cgSgspJ2msm6clMCkdW9",
};

/** The app's id for the user's own voice ("My voice"), resolved per license by [`voiceIdFor`]. */
export const MY_VOICE = "my_voice";

/** A voice the app may ask for: one of VOICES, or the user's own. */
export const isVoice = (voice: unknown): voice is string => typeof voice === "string" && (voice in VOICES || voice === MY_VOICE);

/** ElevenLabs' id for `voice` (see [`isVoice`]) as `payer` may use it; or the response refusing it. */
export async function voiceIdFor(db: SupabaseClient, payer: Payer, voice: string): Promise<string | Response> {
  if (voice !== MY_VOICE) return VOICES[voice];
  if (!payer.email) return fail("My voice needs ScreenForge Pro activated on this computer.", 402, { code: "pro_required" });
  const { data } = await db.from("voice_clones").select("voice_id").eq("email", payer.email.toLowerCase()).maybeSingle();
  return data?.voice_id ?? fail("You have not made your voice yet. Create it under Narration → My voice.", 404, { code: "no_clone" });
}

const DEVICE_DAILY_USD = Number(Deno.env.get("AGENT_DEVICE_DAILY_USD") ?? "0");
const TOTAL_DAILY_USD = Number(Deno.env.get("AGENT_TOTAL_DAILY_USD") ?? "20");

export const isDevice = (device: unknown): device is string => typeof device === "string" && /^[0-9a-f]{64}$/.test(device);

const outOfCredits = (p: Payer) =>
  p.pro
    ? "This month's credits are used up. Top up to keep using ElevenLabs, or pick a voice that runs on this computer."
    : `You're out of credits. Top up, or upgrade to Pro for ${MONTHLY_CREDITS.toLocaleString("en-US")} credits a month.`;

/** The database, and who pays for `device`'s request; or the response refusing it. */
export async function gate(device: string): Promise<{ db: SupabaseClient; payer: Payer } | Response> {
  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const { data: today, error: usageError } = await db.rpc("agent_usage_today", { p_device: device }).maybeSingle();
  if (usageError) return fail(usageError.message, 500);
  // deno-lint-ignore no-explicit-any
  const spent = today as any;
  if (DEVICE_DAILY_USD > 0 && Number(spent?.device_usd ?? 0) >= DEVICE_DAILY_USD) return fail("Today's allowance on this computer is used up. It starts again tomorrow.", 402);
  if (TOTAL_DAILY_USD > 0 && Number(spent?.total_usd ?? 0) >= TOTAL_DAILY_USD) return fail("ElevenLabs is very busy today. Please try again tomorrow.", 503);
  const payer = await payerFor(db, device);
  if (!payer.allowed) {
    return fail(`ElevenLabs is part of Pro: US$20/month for ${MONTHLY_CREDITS.toLocaleString("en-US")} credits a month, shared with the agent. Already on Pro? Activate it with your email in Export.`, 402, { code: "pro_required", sf_credits: summary(payer) });
  }
  if (remaining(payer) <= 0) return fail(outOfCredits(payer), 402, { code: "no_credits", sf_credits: summary(payer) });
  return { db, payer };
}

/** Take `usd` (at 1 credit = US$0.01) from `payer` for `device`; `ref` keeps a retried
 * request from being charged twice. Returns the credits charged; `payer` is updated. */
export async function charge(db: SupabaseClient, device: string, payer: Payer, usd: number, ref: string): Promise<number> {
  const { error } = await db.rpc("agent_usage_add", { p_device: device, p_cost: usd });
  if (error) console.error("agent_usage_add", error.message);
  const credits = usd * 100;
  if (payer.email) {
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
  return credits;
}

/** An ElevenLabs reply that is not 2xx, as the app's error. */
export async function elevenLabsFailed(r: Response | null, what: string): Promise<Response> {
  const detail = r ? await r.text().catch(() => "") : "";
  console.error("elevenlabs", r?.status, detail.slice(0, 500));
  if (r?.status === 429) return fail("Too many requests right now. Wait a moment and try again.", 429);
  return fail(`ElevenLabs could not ${what} right now. Try again in a moment.`, 502);
}

/** Seconds of 16-bit mono PCM in a WAV file, or an error for anything else. */
export function wavSeconds(b: Uint8Array): number | string {
  const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const tag = (at: number) => String.fromCharCode(...b.subarray(at, at + 4));
  if (b.length < 44 || tag(0) !== "RIFF" || tag(8) !== "WAVE") return "audio must be a WAV file";
  let rate = 0;
  for (let at = 12; at + 8 <= b.length;) {
    const id = tag(at);
    const size = v.getUint32(at + 4, true);
    if (id === "fmt ") {
      if (v.getUint16(at + 8, true) !== 1 || v.getUint16(at + 10, true) !== 1 || v.getUint16(at + 22, true) !== 16) return "audio must be 16-bit PCM mono";
      rate = v.getUint32(at + 12, true);
    } else if (id === "data") {
      if (!rate) return "audio has no format";
      return Math.min(size, b.length - at - 8) / (rate * 2);
    }
    at += 8 + size + (size % 2);
  }
  return "audio has no samples";
}

export { summary };

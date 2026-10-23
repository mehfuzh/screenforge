// Claude for the editor's agent panel, so the Anthropic key never ships in the app: it is the
// ANTHROPIC_API_KEY secret here. The app runs the agent loop and its tools itself and sends
// each Messages API request as { device, request }; the reply is the API's message as is.
//
// Only what the agent needs is forwarded (known models, its own tools, capped max_tokens), so
// this is not a general Claude proxy. Each request is paid in credits (_shared/credits.ts):
// the account of the license the computer is activated for (Pro's monthly credits, then
// top-ups), or a few trial credits on a computer without one. A request needs a positive
// balance; its cost is taken after Claude answers and the new balance is returned with the
// message (`sf_credits`). AGENT_TOTAL_DAILY_USD still caps everyone together per UTC day as a
// backstop; AGENT_DEVICE_DAILY_USD per computer (0 turns a cap off).
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import Anthropic from "npm:@anthropic-ai/sdk@0.132.0";
import { MONTHLY_CREDITS, Payer, payerFor, remaining, summary, TRIAL_CREDITS } from "../_shared/credits.ts";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });
const fail = (message: string, status: number) => json({ error: { message } }, status);

/** Why a request was refused for lack of credits, in the app's words. */
const outOfCredits = (p: Payer) =>
  !p.email
    ? `Your free agent credits are used up. Upgrade to Pro for ${MONTHLY_CREDITS.toLocaleString("en-US")} credits a month (and export).`
    : p.pro
    ? "This month's credits are used up. Top up to keep going, or wait for your next billing date."
    : `You're out of agent credits. Top up, or upgrade to Pro for ${MONTHLY_CREDITS.toLocaleString("en-US")} credits a month.`;

// The models the app offers, with US$ per million tokens [input, output, cache read]; keep in
// step with MODELS in crates/sf-agent/src/client.rs. Models that accept server-side fallbacks
// are marked.
// (Opus is not in the app's list: its Auto choice sends complex requests to it.)
const MODELS: Record<string, { price: [number, number, number]; fallbacks: boolean }> = {
  "claude-sonnet-5-5": { price: [2, 10, 0.2], fallbacks: true },
  "claude-haiku-5-5": { price: [0.1, 0.5, 0.01], fallbacks: false },
  "claude-opus-5-5": { price: [4, 20, 0.2], fallbacks: true },
};
// Never asked for, but a declined request can be answered by it through the fallback.
const FALLBACK_PRICES: Record<string, [number, number, number]> = {
  "claude-fable-5-1": [10, 50, 0.25],
};
const MAX_TOKENS = 16000;
const DEVICE_DAILY_USD = Number(Deno.env.get("AGENT_DEVICE_DAILY_USD") ?? "0");
const TOTAL_DAILY_USD = Number(Deno.env.get("AGENT_TOTAL_DAILY_USD") ?? "20");

// deno-lint-ignore no-explicit-any
const cost = (model: string, usage: any): number => {
  const price = MODELS[model]?.price ?? FALLBACK_PRICES[model];
  if (!price || !usage) return 0;
  const [input, output, cacheRead] = price;
  const n = (k: string) => Number(usage[k] ?? 0);
  // cache writes (5 minute) cost 1.25× the input price
  return (n("input_tokens") * input + n("output_tokens") * output + n("cache_read_input_tokens") * cacheRead + n("cache_creation_input_tokens") * input * 1.25) / 1_000_000;
};

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const apiKey = Deno.env.get("ANTHROPIC_API_KEY");
  if (!apiKey) return fail("The agent is not configured on the server yet.", 503);
  const { device, request } = await req.json().catch(() => ({}));
  if (typeof device !== "string" || !/^[0-9a-f]{64}$/.test(device)) return fail("device required", 400);
  if (!request || typeof request !== "object" || !Array.isArray(request.messages)) return fail("request required", 400);
  const model = String(request.model ?? "");
  if (!MODELS[model]) return fail(`The agent does not offer ${model || "that model"}.`, 400);

  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const { data: today, error: usageError } = await db.rpc("agent_usage_today", { p_device: device }).maybeSingle();
  if (usageError) return fail(usageError.message, 500);
  // deno-lint-ignore no-explicit-any
  const spent = today as any;
  if (DEVICE_DAILY_USD > 0 && Number(spent?.device_usd ?? 0) >= DEVICE_DAILY_USD) return fail("Today's agent allowance on this computer is used up. It starts again tomorrow.", 402);
  if (TOTAL_DAILY_USD > 0 && Number(spent?.total_usd ?? 0) >= TOTAL_DAILY_USD) return fail("The agent is very busy today. Please try again tomorrow.", 503);
  const payer = await payerFor(db, device);
  if (remaining(payer) <= 0) return json({ error: { message: outOfCredits(payer), code: "no_credits" }, sf_credits: summary(payer) }, 402);

  // the agent's own tools only: server tools (web search, code execution) bill extra
  // deno-lint-ignore no-explicit-any
  const tools = Array.isArray(request.tools) ? request.tools.filter((t: any) => t && typeof t.name === "string" && t.input_schema && !t.type) : undefined;
  const body = {
    model,
    max_tokens: Math.min(Number(request.max_tokens) || MAX_TOKENS, MAX_TOKENS),
    system: request.system,
    tools,
    messages: request.messages,
    output_config: request.output_config,
    cache_control: request.cache_control,
    ...(MODELS[model].fallbacks ? { betas: ["server-side-fallback-2026-07-01"], fallbacks: "default" } : {}),
  };

  const client = new Anthropic({ apiKey, maxRetries: 1, timeout: 140_000 });
  try {
    // fields newer than this SDK's types (fallbacks) are passed through as they are
    // deno-lint-ignore no-explicit-any
    const message = await client.beta.messages.create(body as any);
    // deno-lint-ignore no-explicit-any
    const m = message as any;
    const usd = cost(m.model ?? model, m.usage);
    const { error } = await db.rpc("agent_usage_add", { p_device: device, p_cost: usd });
    if (error) console.error("agent_usage_add", error.message);
    const credits = usd * 100;
    if (payer.email) {
      const { data, error: spendError } = await db.rpc("credits_spend", { p_email: payer.email, p_credits: credits, p_ref: m.id ?? null }).maybeSingle();
      if (spendError) console.error("credits_spend", spendError.message);
      // deno-lint-ignore no-explicit-any
      const b = data as any;
      if (b) Object.assign(payer, { monthly: Number(b.monthly), topup: Number(b.topup) });
    } else {
      const { data, error: trialError } = await db.rpc("credits_trial_spend", { p_device: device, p_credits: credits });
      if (trialError) console.error("credits_trial_spend", trialError.message);
      payer.trial = Math.max(TRIAL_CREDITS - Number(data ?? 0), 0);
    }
    return json({ ...m, sf_credits: summary(payer) });
  } catch (e) {
    if (e instanceof Anthropic.RateLimitError) return fail("Too many requests right now. Wait a moment and try again.", 429);
    if (e instanceof Anthropic.BadRequestError) return fail(e.message, 400);
    if (e instanceof Anthropic.APIError) {
      console.error("claude", e.status, e.message);
      return fail("Claude could not answer right now. Try again in a moment.", e.status === 529 ? 529 : 502);
    }
    console.error("agent", e);
    return fail("The agent failed on the server.", 500);
  }
});

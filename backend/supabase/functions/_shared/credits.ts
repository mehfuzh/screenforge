// Agent credits shared by `agent`, `credits`, `create-checkout` and `stripe-webhook` (see the
// agent_credits migration). 1 credit = US$0.01 of Claude usage at list price; prices are set
// for a 50% margin: Pro (US$20/month) includes 1,000 credits a month, packs sell 500 credits
// per US$10.
import { SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { newestLicense } from "./activation.ts";

/** Pro's credits per billing period. */
export const MONTHLY_CREDITS = Number(Deno.env.get("AGENT_MONTHLY_CREDITS") ?? "1000");
/** Credits a computer without a license may use to try the agent. */
export const TRIAL_CREDITS = Number(Deno.env.get("AGENT_TRIAL_CREDITS") ?? "50");

/** Top-up packs: the checkout `plan`, the Stripe price (secret), the credits it adds. */
export const PACKS: Record<string, { priceEnv: string; credits: number; usd: number }> = {
  topup_10: { priceEnv: "STRIPE_PRICE_TOPUP_10", credits: 500, usd: 10 },
  topup_25: { priceEnv: "STRIPE_PRICE_TOPUP_25", credits: 1250, usd: 25 },
  topup_50: { priceEnv: "STRIPE_PRICE_TOPUP_50", credits: 2500, usd: 50 },
};

/** Who pays for a computer's agent requests. */
export interface Payer {
  /** The license email the computer is activated for; null: trial credits. */
  email: string | null;
  /** An active Pro (monthly) license. */
  pro: boolean;
  monthly: number;
  topup: number;
  /** Trial credits left (computers without a license). */
  trial: number;
}

export function remaining(p: Payer): number {
  return p.email ? Math.max(p.monthly, 0) + p.topup : p.trial;
}

/** The account a computer spends from, and its balance. */
export async function payerFor(db: SupabaseClient, device: string): Promise<Payer> {
  const { data: act } = await db.from("license_activations").select("email").eq("device_id", device).order("last_seen_at", { ascending: false }).limit(1).maybeSingle();
  if (act?.email) {
    const lic = await newestLicense(db, act.email);
    const today = new Date().toISOString().slice(0, 10);
    const pro = lic?.plan === "monthly" && (lic.expires_at === null || lic.expires_at >= today);
    const { data: bal } = await db.rpc("credits_balance", { p_email: act.email }).maybeSingle();
    // deno-lint-ignore no-explicit-any
    const b = bal as any;
    const live = b?.monthly_until && new Date(b.monthly_until) > new Date();
    return { email: act.email, pro, monthly: live ? Number(b.monthly) : 0, topup: Number(b?.topup ?? 0), trial: 0 };
  }
  const { data: t } = await db.from("credit_trials").select("used").eq("device_id", device).maybeSingle();
  return { email: null, pro: false, monthly: 0, topup: 0, trial: Math.max(TRIAL_CREDITS - Number(t?.used ?? 0), 0) };
}

/** What the app shows: the balance and what can be bought. */
export function summary(p: Payer) {
  return {
    account: p.email,
    pro: p.pro,
    monthly: Math.max(p.monthly, 0),
    topup: p.topup,
    trial: p.trial,
    credits: remaining(p),
    monthlyCredits: MONTHLY_CREDITS,
    packs: Object.entries(PACKS).map(([plan, x]) => ({ plan, credits: x.credits, usd: x.usd })),
  };
}

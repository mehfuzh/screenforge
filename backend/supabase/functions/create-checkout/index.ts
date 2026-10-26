import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import Stripe from "https://esm.sh/stripe@18.5.0";
import { cleanEmail, db } from "../_shared/activation.ts";
import { PACKS, proOrStaff } from "../_shared/credits.ts";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });

// Plans:
//   lifetime   US$49 once: the export license.
//   pro        US$20 a month: export (a monthly key, issued by stripe-webhook) and 1,000 agent
//              credits each billing period (granted on every paid invoice).
//   topup_NN   a pack of agent credits (_shared/credits.ts PACKS), for a Pro subscriber's email:
//              the agent is a Pro feature, and computers spend the credits of the license they
//              are activated for.
// Older apps still ask for "monthly" (the retired US$10 plan): refused, and they fall back to
// opening the pricing page; existing subscriptions keep renewing through stripe-webhook.
serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY") || "", { apiVersion: "2025-08-27.basil" });
  const { plan = "lifetime", email } = await req.json().catch(() => ({}));
  const urls = {
    success_url: Deno.env.get("CHECKOUT_SUCCESS_URL") ?? "https://screenforge.dev/thanks",
    cancel_url: Deno.env.get("CHECKOUT_CANCEL_URL") ?? "https://screenforge.dev/buy",
  };
  // the success page shows the text for what was bought (site/success.html)
  // (the URL may end in a #fragment, e.g. /#thanks: the query goes before it)
  const thanks = (kind: string) => {
    const u = new URL(urls.success_url);
    u.searchParams.set("plan", kind);
    return { ...urls, success_url: u.toString() };
  };

  if (plan === "lifetime") {
    const session = await stripe.checkout.sessions.create({
      mode: "payment",
      line_items: [{ price: Deno.env.get("STRIPE_PRICE_LIFETIME"), quantity: 1 }],
      customer_email: email,
      allow_promotion_codes: true,
      ...thanks("creator"),
      metadata: { plan: "lifetime" },
    });
    return json({ url: session.url });
  }

  if (plan === "pro") {
    const price = Deno.env.get("STRIPE_PRICE_PRO");
    if (!price) return json({ error: "Pro is not on sale yet." }, 503);
    const session = await stripe.checkout.sessions.create({
      mode: "subscription",
      line_items: [{ price, quantity: 1 }],
      customer_email: email,
      allow_promotion_codes: true,
      ...thanks("pro"),
      // stripe-webhook issues a monthly key for plan "monthly"
      metadata: { plan: "monthly", kind: "pro" },
      subscription_data: { metadata: { kind: "pro" } },
    });
    return json({ url: session.url });
  }

  const pack = PACKS[plan];
  if (pack) {
    const address = cleanEmail(email);
    if (!address) return json({ error: "Enter the email of your ScreenForge license." }, 400);
    if (!(await proOrStaff(db(), address))) return json({ error: "Credit packs top up Pro. Upgrade to Pro first, or use the email of your Pro subscription." }, 403);
    const price = Deno.env.get(pack.priceEnv);
    if (!price) return json({ error: "Credit packs are not on sale yet." }, 503);
    const session = await stripe.checkout.sessions.create({
      mode: "payment",
      line_items: [{ price, quantity: 1 }],
      customer_email: address,
      ...thanks("topup"),
      metadata: { kind: "topup", plan, credits: String(pack.credits), email: address },
      payment_intent_data: { metadata: { kind: "topup", plan, credits: String(pack.credits), email: address } },
    });
    return json({ url: session.url });
  }

  if (plan === "monthly") {
    return json({ error: "The monthly plan is no longer offered; the license is US$49 once." }, 410);
  }
  return json({ error: `unknown plan ${plan}` }, 400);
});

import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import Stripe from "https://esm.sh/stripe@18.5.0";
import { cleanEmail, db, isDevice } from "../_shared/activation.ts";
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
// License plans also take, all optional:
//   email    prefills Checkout (Stripe asks for it otherwise)
//   promo    a promotion code typed in the app, applied to the session (an unknown or expired
//            code is a 400 with the reason, so the app shows it before opening the browser)
//   device + claim   the computer and the SHA-256 of a secret it keeps: stripe-webhook records
//            the license for them, and `license-claim` activates that computer (no email code)
// Older apps still ask for "monthly" (the retired US$10 plan): refused, and they fall back to
// opening the pricing page; existing subscriptions keep renewing through stripe-webhook.
serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY") || "", { apiVersion: "2025-08-27.basil" });
  const { plan = "lifetime", email, promo, device, claim } = await req.json().catch(() => ({}));
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

  // the app's computer and claim, carried to stripe-webhook in the session's metadata
  const claimMeta = isDevice(device) && isDevice(claim) ? { device, claim } : {};
  // a typed promotion code, or Stripe's own code box on the page
  let discount: Partial<Pick<Stripe.Checkout.SessionCreateParams, "discounts" | "allow_promotion_codes">> = { allow_promotion_codes: true };
  if (typeof promo === "string" && promo.trim()) {
    const found = await stripe.promotionCodes.list({ code: promo.trim(), active: true, limit: 1 });
    const code = found.data[0];
    if (!code) return json({ error: `The code ${promo.trim()} is not valid or has expired.` }, 400);
    discount = { discounts: [{ promotion_code: code.id }] };
  }
  // Stripe refuses a code the session cannot take (first purchase only, minimum amount …)
  const create = async (params: Stripe.Checkout.SessionCreateParams) => {
    try {
      return json({ url: (await stripe.checkout.sessions.create(params)).url });
    } catch (e) {
      if (discount.discounts && e instanceof Stripe.errors.StripeInvalidRequestError) return json({ error: `That code cannot be used here: ${(e as Error).message}` }, 400);
      throw e;
    }
  };
  const customer_email = cleanEmail(email) ?? undefined;

  if (plan === "lifetime") {
    return await create({
      mode: "payment",
      line_items: [{ price: Deno.env.get("STRIPE_PRICE_LIFETIME"), quantity: 1 }],
      customer_email,
      ...discount,
      ...thanks("creator"),
      metadata: { plan: "lifetime", ...claimMeta },
    });
  }

  if (plan === "pro") {
    const price = Deno.env.get("STRIPE_PRICE_PRO");
    if (!price) return json({ error: "Pro is not on sale yet." }, 503);
    return await create({
      mode: "subscription",
      line_items: [{ price, quantity: 1 }],
      customer_email,
      ...discount,
      ...thanks("pro"),
      // stripe-webhook issues a monthly key for plan "monthly"
      metadata: { plan: "monthly", kind: "pro", ...claimMeta },
      subscription_data: { metadata: { kind: "pro" } },
    });
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

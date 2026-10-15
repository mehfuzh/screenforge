import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import Stripe from "https://esm.sh/stripe@18.5.0";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY") || "", { apiVersion: "2025-08-27.basil" });
  const { plan = "lifetime", email } = await req.json().catch(() => ({}));
  // Only the one-time lifetime key is sold. Older apps still ask for "monthly": refuse, and
  // they fall back to opening the pricing page. Existing subscriptions keep renewing through
  // stripe-webhook.
  if (plan !== "lifetime") {
    return new Response(JSON.stringify({ error: "The monthly plan is no longer offered; the license is US$29 once." }), { status: 410, headers: { ...cors, "Content-Type": "application/json" } });
  }
  const price = Deno.env.get("STRIPE_PRICE_LIFETIME");
  const session = await stripe.checkout.sessions.create({
    mode: "payment",
    line_items: [{ price, quantity: 1 }],
    customer_email: email,
    allow_promotion_codes: true,
    success_url: Deno.env.get("CHECKOUT_SUCCESS_URL") ?? "https://screenforge.dev/thanks",
    cancel_url: Deno.env.get("CHECKOUT_CANCEL_URL") ?? "https://screenforge.dev/buy",
    metadata: { plan: "lifetime" },
  });
  return new Response(JSON.stringify({ url: session.url }), { headers: { ...cors, "Content-Type": "application/json" } });
});

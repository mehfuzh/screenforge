import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import Stripe from "https://esm.sh/stripe@18.5.0";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { emailLicense, isoDate, signLicense } from "../_shared/license.ts";

serve(async (req) => {
  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY") || "", { apiVersion: "2025-08-27.basil" });
  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const body = await req.text();
  let event: Stripe.Event;
  try {
    event = stripe.webhooks.constructEvent(body, req.headers.get("stripe-signature") ?? "", Deno.env.get("STRIPE_WEBHOOK_SECRET") ?? "");
  } catch (e) {
    return new Response(`bad signature: ${e}`, { status: 400 });
  }

  const issue = async (email: string, plan: "monthly" | "lifetime", customer: string | null, subscription: string | null) => {
    const issued = new Date();
    const expires = plan === "monthly" ? new Date(issued.getTime() + 35 * 86400_000) : null; // 35 d: grace over a 31-day cycle
    const key = await signLicense({ email, plan, expires: expires ? isoDate(expires) : null, issued: isoDate(issued) });
    await db.from("licenses").insert({ email, plan, stripe_customer_id: customer, stripe_subscription_id: subscription, issued_at: isoDate(issued), expires_at: expires ? isoDate(expires) : null, key });
    await emailLicense(email, key, plan);
  };

  if (event.type === "checkout.session.completed") {
    const s = event.data.object as Stripe.Checkout.Session;
    const email = s.customer_details?.email ?? s.customer_email;
    if (email) await issue(email, (s.metadata?.plan as "monthly" | "lifetime") ?? "monthly", (s.customer as string) ?? null, (s.subscription as string) ?? null);
  } else if (event.type === "invoice.paid") {
    // renewal: issue a fresh monthly key (the app's Refresh button fetches it)
    const inv = event.data.object as Stripe.Invoice;
    const email = inv.customer_email;
    if (email && inv.billing_reason === "subscription_cycle") await issue(email, "monthly", (inv.customer as string) ?? null, (inv.subscription as string) ?? null);
  }
  return new Response("ok");
});

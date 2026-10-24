import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import Stripe from "https://esm.sh/stripe@18.5.0";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { emailLicense, isoDate, signLicense } from "../_shared/license.ts";
import { MONTHLY_CREDITS } from "../_shared/credits.ts";

// Stripe events (endpoint set up by setup.sh / setup-credits.sh):
//   checkout.session.completed / .async_payment_succeeded
//       license checkout: sign, store and email the key (lifetime, or monthly for Pro);
//       top-up pack: add its credits (only once paid: delayed methods arrive later as
//       async_payment_succeeded).
//   invoice.paid             Pro's agent credits for the paid period (the first invoice and
//                            every renewal), and a fresh monthly key on each renewal.
//   customer.subscription.deleted   Pro ended: its monthly credits stop now (top-ups stay).
//   charge.refunded          a refunded top-up takes its credits back (in proportion).
//   invoice.payment_failed, charge.dispute.created   logged for follow-up.
// Credit changes are keyed by the Stripe object id (credit_ledger), so a redelivered event
// changes nothing.
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
  const rpc = async (name: string, args: Record<string, unknown>) => {
    const { error } = await db.rpc(name, args);
    if (error) throw new Error(`${name}: ${error.message}`);
  };

  const fulfil = async (s: Stripe.Checkout.Session) => {
    // delayed payment methods complete the session unpaid; they arrive again when paid
    if (s.payment_status === "unpaid") return;
    const email = (s.customer_details?.email ?? s.customer_email ?? "").toLowerCase();
    if (!email) return;
    if (s.metadata?.kind === "topup") {
      await rpc("credits_topup", { p_email: s.metadata.email ?? email, p_credits: Number(s.metadata.credits ?? 0), p_ref: s.id });
      return;
    }
    await issue(email, (s.metadata?.plan as "monthly" | "lifetime") ?? "monthly", (s.customer as string) ?? null, (s.subscription as string) ?? null);
  };

  try {
    switch (event.type) {
      case "checkout.session.completed":
      case "checkout.session.async_payment_succeeded":
        await fulfil(event.data.object as Stripe.Checkout.Session);
        break;
      case "invoice.paid": {
        const inv = event.data.object as Stripe.Invoice;
        const email = inv.customer_email?.toLowerCase();
        const subscription = inv.parent?.subscription_details?.subscription ?? null;
        if (!email || !subscription) break;
        // renewal: a fresh monthly key (the app's check-in fetches it)
        if (inv.billing_reason === "subscription_cycle") await issue(email, "monthly", (inv.customer as string) ?? null, typeof subscription === "string" ? subscription : subscription.id);
        if (inv.billing_reason === "subscription_create" || inv.billing_reason === "subscription_cycle") {
          const end = inv.lines?.data?.[0]?.period?.end ?? Math.floor(Date.now() / 1000) + 31 * 86400;
          await rpc("credits_grant_monthly", { p_email: email, p_credits: MONTHLY_CREDITS, p_until: new Date(end * 1000).toISOString(), p_ref: inv.id });
        }
        break;
      }
      case "customer.subscription.deleted": {
        const sub = event.data.object as Stripe.Subscription;
        const customer = await stripe.customers.retrieve(sub.customer as string);
        const email = "deleted" in customer && customer.deleted ? null : (customer as Stripe.Customer).email?.toLowerCase();
        if (email) await db.from("credit_accounts").update({ monthly: 0, monthly_until: new Date().toISOString(), updated_at: new Date().toISOString() }).eq("email", email);
        break;
      }
      case "charge.refunded": {
        const charge = event.data.object as Stripe.Charge;
        if (!charge.payment_intent || !charge.amount) break;
        const pi = await stripe.paymentIntents.retrieve(charge.payment_intent as string);
        if (pi.metadata?.kind !== "topup") break;
        const credits = Number(pi.metadata.credits ?? 0) * (charge.amount_refunded / charge.amount);
        await rpc("credits_refund", { p_email: pi.metadata.email, p_credits: credits, p_ref: `${charge.id}:${charge.amount_refunded}` });
        break;
      }
      case "invoice.payment_failed":
      case "charge.dispute.created":
        console.error(`stripe ${event.type}`, (event.data.object as { id: string }).id);
        break;
    }
  } catch (e) {
    // a 500 makes Stripe retry the event later
    console.error(`stripe ${event.type}`, e);
    return new Response(`failed: ${e}`, { status: 500 });
  }
  return new Response("ok");
});

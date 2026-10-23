// Stripe's customer portal for a Pro subscriber (cancel, change the card, see invoices):
// { email, device } → { url }. Only a computer activated for that email gets a link, so an
// email address alone opens nobody's billing.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import Stripe from "https://esm.sh/stripe@18.5.0";
import { cleanEmail, cors, db, isActivated, isDevice, json } from "../_shared/activation.ts";

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const body = await req.json().catch(() => ({}));
  const email = cleanEmail(body.email);
  if (!email || !isDevice(body.device)) return json({ error: "email and device required" }, 400);
  const client = db();
  if (!(await isActivated(client, email, body.device))) return json({ error: "Sign in on this computer first." }, 403);
  const pattern = email.replace(/[\\%_]/g, (c) => `\\${c}`);
  const { data } = await client.from("licenses").select("stripe_customer_id").ilike("email", pattern).not("stripe_customer_id", "is", null).order("created_at", { ascending: false }).limit(1);
  const customer = data?.[0]?.stripe_customer_id;
  if (!customer) return json({ error: "There is no subscription for this email." }, 404);
  const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY") || "", { apiVersion: "2025-08-27.basil" });
  try {
    const session = await stripe.billingPortal.sessions.create({ customer, return_url: Deno.env.get("PORTAL_RETURN_URL") ?? "https://screenforge.studio/pricing.html" });
    return json({ url: session.url });
  } catch (e) {
    console.error("billing-portal", e);
    return json({ error: "Billing is not available right now. Email license@screenforge.studio." }, 502);
  }
});

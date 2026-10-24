// Sign a computer out of a license: { email, device } removes that computer's activation, so it
// stops spending the license's agent credits and the license can be activated elsewhere
// without a move. The app also deletes its key. Nothing else changes (the license, credits
// and subscription stay with the email).
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { cleanEmail, cors, db, isDevice, json } from "../_shared/activation.ts";

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const body = await req.json().catch(() => ({}));
  const email = cleanEmail(body.email);
  if (!email || !isDevice(body.device)) return json({ error: "email and device required" }, 400);
  const { error } = await db().from("license_activations").delete().eq("email", email).eq("device_id", body.device);
  if (error) return json({ error: "The license server had a problem. Try again in a moment." }, 500);
  return json({ ok: true });
});

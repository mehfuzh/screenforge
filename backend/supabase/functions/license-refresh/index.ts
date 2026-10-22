// Renewed key for the license's computer: { email, device } → { key, plan, expires } (signed
// for that computer) when it is the one the license is activated on (see `license-activate`). An unknown device gets
// { error: "verify" } (403) instead of a key, so knowing a customer's email is not enough.
// Builds before device activation sent { email } only; they are asked to update.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { cleanEmail, cors, db, isActivated, isDevice, json, keyFor, newestLicense } from "../_shared/activation.ts";

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const body = await req.json().catch(() => ({}));
  const email = cleanEmail(body.email);
  if (!email) return json({ error: "email required" }, 400);
  if (!isDevice(body.device)) return json({ error: "Update ScreenForge to activate your license." }, 400);
  try {
    const client = db();
    const lic = await newestLicense(client, email);
    if (!lic) return json({ error: "no license" }, 404);
    if (!(await isActivated(client, lic.email.toLowerCase(), body.device))) return json({ error: "verify" }, 403);
    return json(await keyFor(lic, body.device));
  } catch (e) {
    console.error("license-refresh", e);
    return json({ error: "The license server had a problem. Try again in a moment." }, 500);
  }
});

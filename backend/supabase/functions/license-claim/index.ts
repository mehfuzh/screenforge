// Activate the computer that started a checkout, without an email code. The app sends
// { device, claim } (claim = the secret it kept; create-checkout only ever saw its SHA-256):
// - not paid yet (or unknown)  → { status: "pending" }: the app asks again in a few seconds
// - paid, new address          → { key, plan, expires, email }: the license is moved to this
//                                computer, as confirming an emailed code does
// - paid, address that already had a license → { status: "verify", email }: the app asks for
//   the emailed code (paying is no proof of owning an address typed at checkout)
// A claim works once, from the computer that started the checkout, within a week.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { cors, db, isDevice, json, keyFor, moveLicenseTo, newestLicense } from "../_shared/activation.ts";

const CLAIM_TTL_MS = 7 * 24 * 60 * 60 * 1000;

async function sha256(s: string): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, "0")).join("");
}

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const body = await req.json().catch(() => ({}));
  if (!isDevice(body.device) || !isDevice(body.claim)) return json({ error: "This ScreenForge build cannot identify the computer." }, 400);
  try {
    const client = db();
    const { data: row, error } = await client.from("checkout_claims").select("device_id, email, fresh, created_at, claimed_at").eq("claim_hash", await sha256(body.claim)).maybeSingle();
    if (error) throw new Error(error.message);
    if (!row || row.device_id !== body.device) return json({ status: "pending" });
    if (row.claimed_at || Date.now() - new Date(row.created_at).getTime() > CLAIM_TTL_MS) return json({ error: "This checkout was already used to activate ScreenForge. Sign in with your email instead." }, 410);
    if (!row.fresh) return json({ status: "verify", email: row.email });
    const lic = await newestLicense(client, row.email);
    if (!lic) return json({ status: "pending" });
    // used up first: a second request racing this one finds it claimed
    const used = await client.from("checkout_claims").update({ claimed_at: new Date().toISOString() }).eq("claim_hash", await sha256(body.claim)).is("claimed_at", null).select("claim_hash");
    if (used.error) throw new Error(used.error.message);
    if (!used.data?.length) return json({ error: "This checkout was already used to activate ScreenForge." }, 410);
    await moveLicenseTo(client, lic.email.toLowerCase(), body.device);
    return json({ ...(await keyFor(lic, body.device)), email: lic.email });
  } catch (e) {
    console.error("license-claim", e);
    return json({ error: "The license server had a problem. Try again in a moment." }, 500);
  }
});

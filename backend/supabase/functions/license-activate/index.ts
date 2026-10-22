// Activate a license on a computer. A license is active on one computer at a time. The app
// sends { email, device } (device = SHA-256 hex of the machine's hardware UUID, never the UUID):
// - the license's computer → { key, plan, expires }, the key signed for that computer
// - any other computer: a 6-digit code is emailed to the license address → { status: "code_sent" }
//   (with { send: false }, the app's silent check: { status: "verify" }, no email; an app
//   holding a key gets this once the license moved, and removes the key)
// Then { email, device, code } checks the code, moves the license to that computer and returns
// its key. No key is ever returned for an email alone.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { checkCode, cleanEmail, cors, db, isActivated, isDevice, json, keyFor, moveLicenseTo, newestLicense, sendCode } from "../_shared/activation.ts";

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const body = await req.json().catch(() => ({}));
  const email = cleanEmail(body.email);
  if (!email) return json({ error: "Enter the email you used at checkout." }, 400);
  if (!isDevice(body.device)) return json({ error: "This ScreenForge build cannot identify the computer." }, 400);
  const device: string = body.device;
  try {
    const client = db();
    const lic = await newestLicense(client, email);
    if (!lic) return json({ error: "no license" }, 404);
    const owner = lic.email.toLowerCase();

    if (await isActivated(client, owner, device)) return json(await keyFor(lic, device));

    if (typeof body.code === "string") {
      const check = await checkCode(client, owner, device, body.code.trim());
      if (!check.ok) return json({ error: check.error }, check.status);
      await moveLicenseTo(client, owner, device);
      return json(await keyFor(lic, device));
    }

    if (body.send === false) return json({ status: "verify" });
    const sent = await sendCode(client, owner, device, lic.email);
    if (!sent.ok) return json({ error: sent.error }, sent.status);
    return json({ status: "code_sent" });
  } catch (e) {
    console.error("license-activate", e);
    return json({ error: "The license server had a problem. Try again in a moment." }, 500);
  }
});

// Agent credit balance for a computer: { device } → the summary from _shared/credits.ts
// (account, pro, monthly, topup, trial, credits, monthlyCredits, packs). The app shows it in
// the agent panel and offers Pro and the top-up packs from it.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { cors, db, isDevice, json } from "../_shared/activation.ts";
import { payerFor, summary } from "../_shared/credits.ts";

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const { device } = await req.json().catch(() => ({}));
  if (!isDevice(device)) return json({ error: "device required" }, 400);
  try {
    return json(summary(await payerFor(db(), device)));
  } catch (e) {
    return json({ error: String(e) }, 500);
  }
});

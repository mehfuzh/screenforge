// Free exports without a license: FREE_EXPORTS per computer, counted here by device (SHA-256
// of the hardware UUID) so a reinstall cannot reset them. The app asks { device } before
// enabling Export → { used, free }, and sends { device, record: true } after a free export
// finished (exports recorded while offline are sent on the next check). Without an answer
// from here the app offers no free export, so going offline does not unlock export.
// (`free-export` stays for builds up to 0.1.39 and always answers "all used": this is a
// separate endpoint so those builds do not get their old three back.)
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { cors, db, isDevice, json } from "../_shared/activation.ts";

const FREE_EXPORTS = 2;

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const body = await req.json().catch(() => ({}));
  if (!isDevice(body.device)) return json({ error: "This ScreenForge build cannot identify the computer." }, 400);
  try {
    const client = db();
    if (body.record === true) {
      const { data, error } = await client.rpc("record_free_export", { p_device: body.device });
      if (error) throw new Error(error.message);
      return json({ used: Number(data ?? 0), free: FREE_EXPORTS });
    }
    const { data, error } = await client.from("free_exports").select("used").eq("device_id", body.device).maybeSingle();
    if (error) throw new Error(error.message);
    return json({ used: data?.used ?? 0, free: FREE_EXPORTS });
  } catch (e) {
    console.error("export-allowance", e);
    return json({ error: "The license server had a problem. Try again in a moment." }, 500);
  }
});

// Free-export counter per device. The app calls this with { device } to read the count
// (device = SHA-256 hex of the machine's hardware UUID, never the UUID itself) and with
// { device, record: true } after a successful unlicensed export to add one. Returns { used }.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const { device, record } = await req.json().catch(() => ({}));
  if (typeof device !== "string" || !/^[0-9a-f]{64}$/.test(device)) return json({ error: "device required" }, 400);
  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  if (record === true) {
    const { data, error } = await db.rpc("record_free_export", { p_device: device });
    if (error) return json({ error: error.message }, 500);
    return json({ used: data ?? 0 });
  }
  const { data, error } = await db.from("free_exports").select("used").eq("device_id", device).maybeSingle();
  if (error) return json({ error: error.message }, 500);
  return json({ used: data?.used ?? 0 });
});

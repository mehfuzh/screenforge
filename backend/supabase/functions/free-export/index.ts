// Free-export counter per device, kept only for builds up to 0.1.39, which allowed three
// exports without a license and ask here how many this device has used ({ device } to read,
// { device, record: true } after an unlicensed export). There are no free exports any more,
// so every device is answered as having used all three: those builds take the higher of
// this and their local count, and stop exporting without a license the next time the
// export dialog opens. Builds from 0.1.40 never call this. Nothing touches the database
// (builds up to 0.1.29 read on every repaint), and nothing is stored.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";

/// The free exports old builds allowed; reporting them all used leaves none.
const RETIRED_FREE_EXPORTS = 3;

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const { device } = await req.json().catch(() => ({}));
  if (typeof device !== "string" || !/^[0-9a-f]{64}$/.test(device)) return json({ error: "device required" }, 400);
  return json({ used: RETIRED_FREE_EXPORTS });
});

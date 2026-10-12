// The app calls this with { email } when a monthly key has expired: returns the newest key
// for that email if the subscription is still active (a renewal was paid).
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const { email } = await req.json().catch(() => ({}));
  if (!email) return new Response(JSON.stringify({ error: "email required" }), { status: 400, headers: cors });
  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const { data } = await db.from("licenses").select("key, expires_at, plan").ilike("email", email).order("created_at", { ascending: false }).limit(1);
  const row = data?.[0];
  if (!row) return new Response(JSON.stringify({ error: "no license" }), { status: 404, headers: cors });
  return new Response(JSON.stringify({ key: row.key, plan: row.plan, expires: row.expires_at }), { headers: { ...cors, "Content-Type": "application/json" } });
});

// Device activation shared by `license-activate` and `license-refresh` (see the
// license_activations migration). A license is active on one computer at a time. A key is only
// returned to that computer, signed for it (the app rejects keys bound to another computer);
// activating a computer needs the 6-digit code emailed to the license address, and moves the
// license there.
import { createClient, SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { signLicense } from "./license.ts";

const CODE_TTL_MS = 15 * 60 * 1000;
const MAX_ATTEMPTS = 5;
const SEND_GAP_MS = 60 * 1000;
const SENDS_PER_HOUR = 5;

export const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
export const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });

export function db(): SupabaseClient {
  return createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
}

/** Normalised email, or null when it is not a plausible address. */
export function cleanEmail(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const e = v.trim().toLowerCase();
  return e.length <= 254 && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(e) ? e : null;
}

export const isDevice = (v: unknown): v is string => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);

export interface LicenseRow { key: string; plan: string; expires_at: string | null; issued_at: string; email: string }

/** Newest license issued to `email` (exact, case-insensitive: `%` and `_` are escaped, so
 *  they cannot act as wildcards and match someone else's address). */
export async function newestLicense(client: SupabaseClient, email: string): Promise<LicenseRow | null> {
  const pattern = email.replace(/[\\%_]/g, (c) => `\\${c}`);
  const { data, error } = await client.from("licenses").select("key, plan, expires_at, issued_at, email").ilike("email", pattern).order("created_at", { ascending: false }).limit(1);
  if (error) throw new Error(error.message);
  return data?.[0] ?? null;
}

/** True when `device` is activated for `email` (and marks it seen). */
export async function isActivated(client: SupabaseClient, email: string, device: string): Promise<boolean> {
  const { data, error } = await client.from("license_activations").select("device_id").eq("email", email).eq("device_id", device).maybeSingle();
  if (error) throw new Error(error.message);
  if (!data) return false;
  await client.from("license_activations").update({ last_seen_at: new Date().toISOString() }).eq("email", email).eq("device_id", device);
  return true;
}

/** The license's key re-signed for `device`, as the app expects it. */
export async function keyFor(lic: LicenseRow, device: string) {
  const key = await signLicense({ email: lic.email, plan: lic.plan as "monthly" | "lifetime", expires: lic.expires_at, issued: lic.issued_at, device });
  return { key, plan: lic.plan, expires: lic.expires_at };
}

async function sha256(s: string): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, "0")).join("");
}

const codeHash = (email: string, device: string, code: string) => sha256(`${email}:${device}:${code}`);

function sameString(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

export type SendResult = { ok: true } | { ok: false; error: string; status: number };

/** Email a new code for `device` to the license's address, within the rate limits. */
export async function sendCode(client: SupabaseClient, email: string, device: string, to: string): Promise<SendResult> {
  const hourAgo = new Date(Date.now() - 60 * 60 * 1000).toISOString();
  const { data: sends, error } = await client.from("license_code_sends").select("sent_at").eq("email", email).gte("sent_at", hourAgo).order("sent_at", { ascending: false });
  if (error) throw new Error(error.message);
  if (sends && sends.length > 0 && Date.now() - new Date(sends[0].sent_at).getTime() < SEND_GAP_MS) {
    return { ok: false, status: 429, error: "A code was just sent. Wait a minute before asking for another." };
  }
  if ((sends?.length ?? 0) >= SENDS_PER_HOUR) return { ok: false, status: 429, error: "Too many codes requested. Try again in an hour." };
  // 6 digits from the CSPRNG (the modulo bias of 2^32 % 10^6 is negligible)
  const code = String(crypto.getRandomValues(new Uint32Array(1))[0] % 1_000_000).padStart(6, "0");
  const up = await client.from("license_codes").upsert({ email, device_id: device, code_hash: await codeHash(email, device, code), expires_at: new Date(Date.now() + CODE_TTL_MS).toISOString(), attempts: 0, created_at: new Date().toISOString() });
  if (up.error) throw new Error(up.error.message);
  await client.from("license_code_sends").insert({ email });
  // old send rows only matter for an hour
  await client.from("license_code_sends").delete().lt("sent_at", new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString());
  await emailCode(to, code);
  return { ok: true };
}

export type CheckResult = { ok: true } | { ok: false; error: string; status: number };

/** Check `code` for `device`; a wrong code counts an attempt, the right one is used up. */
export async function checkCode(client: SupabaseClient, email: string, device: string, code: string): Promise<CheckResult> {
  const { data: row, error } = await client.from("license_codes").select("code_hash, expires_at, attempts").eq("email", email).eq("device_id", device).maybeSingle();
  if (error) throw new Error(error.message);
  if (!row || new Date(row.expires_at).getTime() < Date.now()) return { ok: false, status: 400, error: "That code has expired. Ask for a new one." };
  if (row.attempts >= MAX_ATTEMPTS) return { ok: false, status: 429, error: "Too many wrong codes. Ask for a new one." };
  if (!/^\d{6}$/.test(code) || !sameString(await codeHash(email, device, code), row.code_hash)) {
    await client.from("license_codes").update({ attempts: row.attempts + 1 }).eq("email", email).eq("device_id", device);
    return { ok: false, status: 400, error: "That code is not right. Check the email and try again." };
  }
  await client.from("license_codes").delete().eq("email", email).eq("device_id", device);
  return { ok: true };
}

/** Make `device` the license's computer: any other computer is deactivated (its key stops
 *  working the next time it checks in online). */
export async function moveLicenseTo(client: SupabaseClient, email: string, device: string) {
  const del = await client.from("license_activations").delete().eq("email", email).neq("device_id", device);
  if (del.error) throw new Error(del.error.message);
  const { error } = await client.from("license_activations").upsert({ email, device_id: device, last_seen_at: new Date().toISOString() });
  if (error) throw new Error(error.message);
}

async function emailCode(to: string, code: string) {
  const api = Deno.env.get("RESEND_API_KEY");
  if (!api) throw new Error("RESEND_API_KEY not set");
  const r = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: { Authorization: `Bearer ${api}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      from: Deno.env.get("LICENSE_FROM_EMAIL") ?? "ScreenForge <licenses@screenforge.dev>",
      to: [to],
      subject: `${code} is your ScreenForge activation code`,
      text: `Your code to activate ScreenForge on a computer:\n\n${code}\n\nEnter it in ScreenForge → Export. It expires in 15 minutes.\n\nIf you did not ask for this, ignore this email: nobody can use your license without the code.\n`,
    }),
  });
  if (!r.ok) throw new Error(`email failed: ${r.status}`);
}

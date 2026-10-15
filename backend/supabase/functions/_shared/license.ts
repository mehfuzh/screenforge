// Ed25519 key signing shared by the functions. Key format: SF1.<payload b64url>.<sig b64url>
import * as ed from "https://esm.sh/@noble/ed25519@2.1.0";
import { sha512 } from "https://esm.sh/@noble/hashes@1.4.0/sha512";
ed.etc.sha512Sync = (...m) => sha512(ed.etc.concatBytes(...m));

const b64url = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const fromB64url = (s: string) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (s.length % 4)) % 4)), (c) => c.charCodeAt(0));

export interface Payload { email: string; plan: "monthly" | "lifetime"; expires: string | null; issued: string }

export async function signLicense(payload: Payload): Promise<string> {
  const secret = Deno.env.get("SF_LICENSE_SIGNING_KEY");
  if (!secret) throw new Error("SF_LICENSE_SIGNING_KEY not set");
  // serde_json field order must match the Rust Payload struct
  const json = new TextEncoder().encode(JSON.stringify({ email: payload.email, plan: payload.plan, expires: payload.expires, issued: payload.issued }));
  const sig = await ed.signAsync(json, fromB64url(secret));
  return `SF1.${b64url(json)}.${b64url(sig)}`;
}

export function isoDate(d: Date): string { return d.toISOString().slice(0, 10); }

export async function emailLicense(email: string, key: string, plan: string) {
  const api = Deno.env.get("RESEND_API_KEY");
  if (!api) { console.warn("RESEND_API_KEY not set; key not emailed", key); return; }
  await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: { Authorization: `Bearer ${api}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      from: Deno.env.get("LICENSE_FROM_EMAIL") ?? "ScreenForge <licenses@screenforge.dev>",
      to: [email],
      subject: "Your ScreenForge license key",
      text: `Thanks for supporting ScreenForge!\n\nYour ${plan} license key:\n\n${key}\n\nOpen ScreenForge → Export, enter this email address and press Activate.${plan === "monthly" ? " Monthly keys renew automatically while the subscription is active." : " The key never expires."}\n`,
    }),
  });
}

// Ed25519 key signing shared by the functions. Key format: SF1.<payload b64url>.<sig b64url>
import * as ed from "https://esm.sh/@noble/ed25519@2.1.0";
import { sha512 } from "https://esm.sh/@noble/hashes@1.4.0/sha512";
ed.etc.sha512Sync = (...m) => sha512(ed.etc.concatBytes(...m));

const b64url = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const fromB64url = (s: string) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (s.length % 4)) % 4)), (c) => c.charCodeAt(0));

// `device` binds the key to one computer (SHA-256 hex of its hardware UUID); the app only
// accepts a key bound to itself. Keys emailed at purchase have none.
export interface Payload { email: string; plan: "monthly" | "lifetime"; expires: string | null; issued: string; device?: string }

export async function signLicense(payload: Payload): Promise<string> {
  const secret = Deno.env.get("SF_LICENSE_SIGNING_KEY");
  if (!secret) throw new Error("SF_LICENSE_SIGNING_KEY not set");
  // serde_json field order must match the Rust Payload struct
  const fields: Record<string, unknown> = { email: payload.email, plan: payload.plan, expires: payload.expires, issued: payload.issued };
  if (payload.device) fields.device = payload.device;
  const json = new TextEncoder().encode(JSON.stringify(fields));
  const sig = await ed.signAsync(json, fromB64url(secret));
  return `SF1.${b64url(json)}.${b64url(sig)}`;
}

export function isoDate(d: Date): string { return d.toISOString().slice(0, 10); }

// The purchase email: how to activate with this address. The key itself is never shown; the
// app fetches one bound to the computer once the emailed 6-digit code is entered.
export async function emailLicense(email: string, plan: string) {
  const api = Deno.env.get("RESEND_API_KEY");
  if (!api) { console.warn("RESEND_API_KEY not set; purchase email not sent", email); return; }
  const pro = plan === "monthly";
  const steps = pro
    ? `1. In ScreenForge, open the AI agent panel.\n2. Enter this address (${email}) under "Email used at checkout" and press Activate.\n3. Type the 6-digit code we email you.\n\nYour 1,000 agent credits for this month then show in the agent panel, and export is unlocked too. Pro renews every month until you cancel; Manage subscription in the agent panel cancels it or changes the card.`
    : `1. In ScreenForge, open any project and press Export.\n2. Enter this address (${email}) and press Activate.\n3. Type the 6-digit code we email you.\n\nExport is then unlocked for good: the license never expires, and every update is included.`;
  await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: { Authorization: `Bearer ${api}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      from: Deno.env.get("LICENSE_FROM_EMAIL") ?? "ScreenForge <licenses@screenforge.dev>",
      to: [email],
      subject: pro ? "Your ScreenForge AI editor is ready" : "Unlimited export is yours",
      text: `Thanks for ${pro ? "subscribing to ScreenForge Pro" : "buying ScreenForge"}!\n\nThere is no key to copy: ScreenForge unlocks with this email address.\n\n${steps}\n\nIt works on one computer at a time; activating another computer with this email moves it there.\n\nAnything wrong? Write to license@screenforge.studio and a person answers, usually the same day.\n`,
    }),
  });
}

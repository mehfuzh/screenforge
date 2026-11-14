// Cloud share links, a Pro feature: the app uploads a video to R2 and gets a link to the page
// at screenforge.studio/s/<id> (Cloudflare Pages Function in functions/s/[id].js) that plays it.
//
// The owner's actions carry { device, key }: the license key this computer holds, signed for it
// (see `verifyKey`), and the computer must still be the license's active one. A device id alone
// is not enough, since device ids are not secret.
//
//   { action: "create", device, key, project?, title, bytes, poster?, width?, height?, duration_ms? }
//       Pro only → { id, url, upload: { video, poster? } }: presigned PUT URLs (video/mp4, and
//       image/jpeg for the poster) the app uploads to, signed for exactly `bytes` / `poster` bytes.
//       A project has one link: sharing it again uploads over the same link (same URL), whose
//       page keeps the old video until the new one is complete.
//   { action: "complete", device, key, id }  → { id, url } once the upload is in R2 at its size
//   { action: "allowed", device, key }       → { allowed }: may make links (Pro or staff)
//   { action: "list", device, key }          → { links: [...] } newest first (also after Pro ends)
//   { action: "delete", device, key, id }    → { ok: true }: the link and its files are gone
//   { action: "get", id }                    → public: what the share page shows (404: none)
//   { action: "view", id }                   → public: counts a view
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";
import { cors, db, isActivated, isDevice, json } from "../_shared/activation.ts";
import { proOrStaff } from "../_shared/credits.ts";
import { verifyKey } from "../_shared/license.ts";
import { configured, deleteObject, MEDIA_URL, objectSize, posterKey, presignPut, videoKey } from "../_shared/r2.ts";

const PAGE_URL = (Deno.env.get("SHARE_PAGE_URL") ?? "https://screenforge.studio/s/").replace(/\/?$/, "/");
/** Largest video one link may hold (default 2 GiB). */
const MAX_BYTES = Number(Deno.env.get("SHARE_MAX_BYTES") ?? String(2 * 1024 ** 3));
/** Storage one account may use across its links (default 100 GiB). */
const MAX_ACCOUNT_BYTES = Number(Deno.env.get("SHARE_MAX_ACCOUNT_BYTES") ?? String(100 * 1024 ** 3));
const MAX_POSTER_BYTES = 2 * 1024 * 1024;

const fail = (error: string, status: number, code?: string) => json({ error, ...(code ? { code } : {}) }, status);
const isId = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9]{12}$/.test(v);
const isProject = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(v);
const urlFor = (id: string) => PAGE_URL + id;

/** 12 characters of base62 from the CSPRNG (about 71 bits: links are unlisted, not guessable). */
function newId(): string {
  const abc = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  let out = "";
  while (out.length < 12) {
    for (const b of crypto.getRandomValues(new Uint8Array(16))) {
      if (b < 248 && out.length < 12) out += abc[b % 62]; // 248 = 4 × 62: no modulo bias
    }
  }
  return out;
}

/**
 * A project's link id, the same every time: 12 base62 characters of
 * SHA-256("screenforge-share/" + project). The app shows it before anything is uploaded
 * (`share::link_id`; "iv5Pww9n2l" → "v3BwvryVLJ5B", "abc" → "LyVS9w6Jh3S9").
 */
async function linkIdFor(project: string): Promise<string> {
  const abc = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`screenforge-share/${project}`)));
  return Array.from(d.slice(0, 12), (b) => abc[b % 62]).join("");
}

/** The license email of the computer making the request, or an error reply. */
async function owner(client: SupabaseClient, device: unknown, key: unknown): Promise<string | Response> {
  if (!isDevice(device)) return fail("device required", 400);
  const p = await verifyKey(key);
  if (!p || p.device !== device) return fail("Sign in to ScreenForge on this computer first (Export → Sign in).", 401, "sign_in");
  const email = p.email.toLowerCase();
  if (!(await isActivated(client, email, device))) return fail("This license is active on another computer now. Sign in again to use it here.", 401, "sign_in");
  return email;
}

async function removeLink(client: SupabaseClient, id: string) {
  await deleteObject(videoKey(id));
  await deleteObject(posterKey(id));
  const { error } = await client.from("share_links").delete().eq("id", id);
  if (error) throw new Error(error.message);
}

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  if (!configured()) return fail("Share links are not set up on the server yet.", 503);
  const body = await req.json().catch(() => ({}));
  const client = db();
  try {
    switch (body.action) {
      case "get": {
        if (!isId(body.id)) return fail("not found", 404);
        const { data } = await client.from("share_links").select("id, title, width, height, duration_ms, has_poster, views, created_at, rev").eq("id", body.id).eq("status", "ready").maybeSingle();
        if (!data) return fail("not found", 404);
        // ?v=: a replaced video gets a new address, past the CDN's cached copy of the old one
        const v = data.rev ? `?v=${data.rev}` : "";
        return json({
          id: data.id, title: data.title, url: urlFor(data.id), video: `${MEDIA_URL}/${videoKey(data.id)}${v}`,
          poster: data.has_poster ? `${MEDIA_URL}/${posterKey(data.id)}${v}` : null,
          width: data.width, height: data.height, duration_ms: data.duration_ms, views: data.views, created_at: data.created_at,
        });
      }
      case "view": {
        if (isId(body.id)) await client.rpc("share_view", { p_id: body.id });
        return json({ ok: true });
      }
      case "allowed": {
        // may this computer's account make links (Pro or staff)? The app asks once per license
        const email = await owner(client, body.device, body.key);
        if (email instanceof Response) return email;
        return json({ allowed: await proOrStaff(client, email) });
      }
      case "create": {
        const email = await owner(client, body.device, body.key);
        if (email instanceof Response) return email;
        if (!(await proOrStaff(client, email))) return fail("Share links are part of Pro. Upgrade to get a link for every recording.", 402, "pro_required");
        const bytes = Number(body.bytes);
        if (!Number.isInteger(bytes) || bytes <= 0) return fail("bytes required", 400);
        if (bytes > MAX_BYTES) return fail(`That video is ${(bytes / 1024 ** 3).toFixed(1)} GB; a share link holds up to ${(MAX_BYTES / 1024 ** 3).toFixed(0)} GB. Export at a lower resolution or compression and share again.`, 413);
        const poster = body.poster == null ? 0 : Number(body.poster);
        if (!Number.isInteger(poster) || poster < 0 || poster > MAX_POSTER_BYTES) return fail("poster too large", 400);
        // uploads that never finished (the app quit mid-upload) are dropped after a day
        const { data: stale } = await client.from("share_links").select("id").eq("email", email).eq("status", "uploading").lt("created_at", new Date(Date.now() - 24 * 3600 * 1000).toISOString());
        for (const s of stale ?? []) await removeLink(client, s.id);
        const project = isProject(body.project) ? body.project : null;
        // the project's link id is derived from the project (the link the app shows); one
        // made before that, with a random id, is dropped for it: the project keeps one link
        const own = project ? await linkIdFor(project) : null;
        let { data: existing } = project
          ? await client.from("share_links").select("id").eq("email", email).eq("project", project).maybeSingle()
          : { data: null };
        if (existing && own && existing.id !== own) {
          await removeLink(client, existing.id);
          existing = null;
        }
        const { data: used } = await client.from("share_links").select("id, bytes").eq("email", email);
        // a replaced video's old size no longer counts
        const total = (used ?? []).filter((r) => r.id !== existing?.id).reduce((n, r) => n + Number(r.bytes), 0);
        if (total + bytes > MAX_ACCOUNT_BYTES) return fail(`Your share links use ${(total / 1024 ** 3).toFixed(1)} GB of ${(MAX_ACCOUNT_BYTES / 1024 ** 3).toFixed(0)} GB. Contact ScreenForge support to remove old links, then try again.`, 413);
        const int = (v: unknown) => (Number.isInteger(v) && (v as number) > 0 && (v as number) < 2 ** 31 ? v as number : null);
        const fields = { device_id: body.device, title: String(body.title ?? "").slice(0, 200), width: int(body.width), height: int(body.height), duration_ms: int(body.duration_ms), pending_bytes: bytes, updated_at: new Date().toISOString() };
        let id: string;
        if (existing) {
          // the page keeps playing the old video (status stays) until `complete`
          id = existing.id;
          const { error } = await client.from("share_links").update(fields).eq("id", id);
          if (error) throw new Error(error.message);
        } else {
          // the project's own id, unless another account holds it (a copied project file)
          const { data: taken } = own ? await client.from("share_links").select("id").eq("id", own).maybeSingle() : { data: null };
          id = own && !taken ? own : newId();
          const { error } = await client.from("share_links").insert({ id, email, project, bytes, has_poster: poster > 0, ...fields });
          if (error) throw new Error(error.message);
        }
        const upload: Record<string, string> = { video: await presignPut(videoKey(id), "video/mp4", bytes) };
        if (poster > 0) upload.poster = await presignPut(posterKey(id), "image/jpeg", poster);
        return json({ id, url: urlFor(id), upload });
      }
      case "complete": {
        const email = await owner(client, body.device, body.key);
        if (email instanceof Response) return email;
        if (!isId(body.id)) return fail("not found", 404);
        const { data: link } = await client.from("share_links").select("id, bytes, pending_bytes, rev").eq("id", body.id).eq("email", email).maybeSingle();
        if (!link) return fail("not found", 404);
        const expected = Number(link.pending_bytes ?? link.bytes);
        const size = await objectSize(videoKey(link.id));
        if (size !== expected) return fail("The upload did not arrive complete. Try sharing again.", 409);
        const hasPoster = (await objectSize(posterKey(link.id))) !== null;
        const { error } = await client.from("share_links").update({ status: "ready", bytes: expected, pending_bytes: null, has_poster: hasPoster, rev: Number(link.rev ?? 0) + 1, updated_at: new Date().toISOString() }).eq("id", link.id);
        if (error) throw new Error(error.message);
        return json({ id: link.id, url: urlFor(link.id) });
      }
      case "list": {
        const email = await owner(client, body.device, body.key);
        if (email instanceof Response) return email;
        const { data, error } = await client.from("share_links").select("id, project, title, bytes, duration_ms, views, created_at, updated_at").eq("email", email).eq("status", "ready").order("updated_at", { ascending: false }).limit(200);
        if (error) throw new Error(error.message);
        return json({ links: (data ?? []).map((l) => ({ ...l, url: urlFor(l.id) })) });
      }
      case "delete": {
        const email = await owner(client, body.device, body.key);
        if (email instanceof Response) return email;
        if (!isId(body.id)) return fail("not found", 404);
        const { data: link } = await client.from("share_links").select("id").eq("id", body.id).eq("email", email).maybeSingle();
        if (!link) return fail("not found", 404);
        await removeLink(client, link.id);
        return json({ ok: true });
      }
      default:
        return fail("unknown action", 400);
    }
  } catch (e) {
    console.error("share", body.action, e);
    return fail("Something went wrong on our side. Try again in a moment.", 500);
  }
});

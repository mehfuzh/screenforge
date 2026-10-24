// Unsplash proxy for the Background → Unsplash picker, so the access key never ships in the app.
// { query, page }  → { results: [{ id, thumb, full, color, author, authorUrl, photoUrl, downloadLocation }], totalPages }
//                    (an empty query lists Unsplash's wallpaper topic)
// { download }     → calls the photo's download_location as the API guidelines require when a
//                    photo is used; returns { url } (the image to fetch)
//
// Search replies are cached for every user in `unsplash_cache` (see the migration): fresh rows
// answer without calling Unsplash, and a stale row is served when Unsplash refuses, so the
// app's one rate limit (50 requests an hour in demo mode) is spent on new searches only.
// Download tracking is never cached: Unsplash counts each use of a photo.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });
const API = "https://api.unsplash.com";
const PER_PAGE = 24;
// Search replies younger than this are served from the cache.
const TTL_MS = 6 * 60 * 60 * 1000;
// Rows not refreshed for this long are deleted.
const PRUNE_MS = 7 * 24 * 60 * 60 * 1000;

// deno-lint-ignore no-explicit-any
const photo = (p: any) => ({
  id: p.id,
  thumb: p.urls?.small,
  // large enough for a 4K canvas; Unsplash resizes on its CDN
  full: `${p.urls?.raw}&w=3840&fm=jpg&q=85`,
  color: p.color,
  author: p.user?.name,
  authorUrl: p.user?.links?.html,
  photoUrl: p.links?.html,
  downloadLocation: p.links?.download_location,
  // what the photo shows, so the editor's agent can pick one by its content
  description: p.alt_description ?? p.description ?? null,
});

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const key = Deno.env.get("UNSPLASH_ACCESS_KEY");
  if (!key) return json({ error: "Unsplash is not configured on the server" }, 503);
  const headers = { Authorization: `Client-ID ${key}`, "Accept-Version": "v1" };
  const { query, page, download } = await req.json().catch(() => ({}));

  if (typeof download === "string") {
    // only Unsplash's own download endpoints, so this cannot be used as an open proxy
    if (!/^https:\/\/api\.unsplash\.com\/photos\/[A-Za-z0-9_-]+\/download(\?.*)?$/.test(download)) return json({ error: "bad download location" }, 400);
    const r = await fetch(download, { headers });
    if (!r.ok) return json({ error: `unsplash returned ${r.status}` }, 502);
    const { url } = await r.json();
    return json({ url });
  }

  const p = Math.max(1, Math.min(50, Number(page) || 1));
  // normalised so "Pastel  Gradient" and "pastel gradient" share a row
  const q = typeof query === "string" ? query.trim().toLowerCase().replace(/\s+/g, " ").slice(0, 100) : "";
  const cacheKey = `${q}|${p}`;
  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const { data: cached } = await db.from("unsplash_cache").select("body, fetched_at").eq("key", cacheKey).maybeSingle();
  if (cached && Date.now() - new Date(cached.fetched_at).getTime() < TTL_MS) return json({ ...cached.body, cached: true });

  const url = q
    ? `${API}/search/photos?query=${encodeURIComponent(q)}&page=${p}&per_page=${PER_PAGE}&orientation=landscape&content_filter=high`
    : `${API}/topics/wallpapers/photos?page=${p}&per_page=${PER_PAGE}&orientation=landscape`;
  const r = await fetch(url, { headers });
  if (!r.ok) {
    // rate limited or down: an old answer beats none
    if (cached) return json({ ...cached.body, cached: true, stale: true });
    return json({ error: r.status === 403 || r.status === 429 ? "Unsplash is busy right now, try again in a few minutes" : `unsplash returned ${r.status}` }, 502);
  }
  const body = await r.json();
  const list = Array.isArray(body) ? body : body.results ?? [];
  const totalPages = Array.isArray(body) ? (list.length < PER_PAGE ? p : p + 1) : body.total_pages ?? 1;
  const reply = { results: list.map(photo), totalPages };
  await db.from("unsplash_cache").upsert({ key: cacheKey, body: reply, fetched_at: new Date().toISOString() });
  await db.from("unsplash_cache").delete().lt("fetched_at", new Date(Date.now() - PRUNE_MS).toISOString());
  return json(reply);
});

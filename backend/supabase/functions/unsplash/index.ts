// Unsplash proxy for the Background → Unsplash picker, so the access key never ships in the app.
// { query, page }  → { results: [{ id, thumb, full, color, author, authorUrl, photoUrl, downloadLocation }], totalPages }
//                    (an empty query lists Unsplash's wallpaper topic)
// { download }     → calls the photo's download_location as the API guidelines require when a
//                    photo is used; returns { url } (the image to fetch)
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });
const API = "https://api.unsplash.com";
const PER_PAGE = 24;

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
  const q = typeof query === "string" ? query.trim().slice(0, 100) : "";
  const url = q
    ? `${API}/search/photos?query=${encodeURIComponent(q)}&page=${p}&per_page=${PER_PAGE}&orientation=landscape&content_filter=high`
    : `${API}/topics/wallpapers/photos?page=${p}&per_page=${PER_PAGE}&orientation=landscape`;
  const r = await fetch(url, { headers });
  if (!r.ok) return json({ error: `unsplash returned ${r.status}` }, 502);
  const body = await r.json();
  const list = Array.isArray(body) ? body : body.results ?? [];
  const totalPages = Array.isArray(body) ? (list.length < PER_PAGE ? p : p + 1) : body.total_pages ?? 1;
  return json({ results: list.map(photo), totalPages });
});

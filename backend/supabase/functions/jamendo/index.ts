// Jamendo proxy for Audio → Find music, so the client id is not shipped in the app and searches
// are shared through a cache.
// { query, page, instrumental, noncommercial }
//   → { results: [{ id, name, artist, duration, image, download, licenseUrl, shareUrl }], totalPages }
//     (an empty query lists the month's most popular tracks)
//
// Licences: background music is synchronised with a video, which every Creative Commons licence
// counts as an Adaptation (CC 3.0 §1(a), "synching"). So NoDerivs tracks (ND) can never be used
// and ShareAlike tracks (SA) would force the user's video under CC BY-SA: neither is returned.
// NonCommercial tracks (NC) only with `noncommercial: true`, for videos that earn nothing.
// Tracks whose licence Jamendo does not give are dropped too.
//
// Only tracks Jamendo allows to be downloaded are returned; `download` is the MP3 the app fetches
// straight from Jamendo's storage. Replies are cached in `jamendo_cache` like `unsplash_cache`:
// fresh rows answer without calling Jamendo, a stale row is served when Jamendo refuses.
import { serve } from "https://deno.land/std@0.190.0/http/server.ts";
import { createClient } from "https://esm.sh/@supabase/supabase-js@2.57.2";

const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { ...cors, "Content-Type": "application/json" } });
const API = "https://api.jamendo.com/v3.0";
const PER_PAGE = 20;
// Search replies younger than this are served from the cache.
const TTL_MS = 6 * 60 * 60 * 1000;
// Rows not refreshed for this long are deleted.
const PRUNE_MS = 7 * 24 * 60 * 60 * 1000;

// deno-lint-ignore no-explicit-any
const track = (t: any) => ({
  id: String(t.id),
  name: t.name,
  artist: t.artist_name,
  duration: Number(t.duration) || 0,
  image: t.album_image || t.image || null,
  download: t.audiodownload,
  licenseUrl: t.license_ccurl || null,
  shareUrl: t.shareurl || null,
});

// The licence part of a Creative Commons URL ("by", "by-nc" …), whatever its version or port.
const ccKind = (url: unknown) => (typeof url === "string" ? /creativecommons\.org\/licenses\/([a-z-]+)\//.exec(url)?.[1] : undefined);
// Jamendo's own filters are checked again here: a track reaches the app only with a licence that
// allows it under a video.
const usable = (url: unknown, nc: boolean) => {
  const kind = ccKind(url);
  return kind === "by" || (nc && kind === "by-nc");
};

serve(async (req) => {
  if (req.method === "OPTIONS") return new Response(null, { headers: cors });
  const clientId = Deno.env.get("JAMENDO_CLIENT_ID");
  if (!clientId) return json({ error: "Jamendo is not configured on the server" }, 503);
  const { query, page, instrumental, noncommercial } = await req.json().catch(() => ({}));

  const p = Math.max(1, Math.min(50, Number(page) || 1));
  // normalised so "Lo  Fi" and "lo fi" share a row
  const q = typeof query === "string" ? query.trim().toLowerCase().replace(/\s+/g, " ").slice(0, 100) : "";
  const inst = instrumental !== false;
  const nc = noncommercial === true;
  const cacheKey = `${q}|${p}|${inst ? "i" : "a"}|${nc ? "by,by-nc" : "by"}`;
  const db = createClient(Deno.env.get("SUPABASE_URL") ?? "", Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "", { auth: { persistSession: false } });
  const { data: cached } = await db.from("jamendo_cache").select("body, fetched_at").eq("key", cacheKey).maybeSingle();
  if (cached && Date.now() - new Date(cached.fetched_at).getTime() < TTL_MS) return json({ ...cached.body, cached: true });

  const params = new URLSearchParams({
    client_id: clientId,
    format: "json",
    limit: String(PER_PAGE),
    offset: String((p - 1) * PER_PAGE),
    audiodlformat: "mp32",
    imagesize: "100",
    order: q ? "relevance" : "popularity_month",
  });
  if (q) params.set("search", q);
  if (inst) params.set("vocalinstrumental", "instrumental");
  params.set("ccnd", "false");
  params.set("ccsa", "false");
  if (!nc) params.set("ccnc", "false");
  // Jamendo now and then answers "success" with no results for a query that has plenty (about
  // one call in three, measured October 2026), so an empty page is asked for again
  let r: Response | null = null;
  // deno-lint-ignore no-explicit-any
  let body: any = null;
  for (let attempt = 0; attempt < 3; attempt++) {
    r = await fetch(`${API}/tracks/?${params}`);
    body = r.ok ? await r.json().catch(() => null) : null;
    if (!body || body.headers?.status !== "success" || (body.results?.length ?? 0) > 0) break;
  }
  if (!body || body.headers?.status !== "success") {
    // rate limited or down: an old answer beats none
    if (cached) return json({ ...cached.body, cached: true, stale: true });
    return json({ error: body?.headers?.error_message || `jamendo returned ${r?.status}` }, 502);
  }
  // deno-lint-ignore no-explicit-any
  const list = (body.results ?? []).filter((t: any) => usable(t.license_ccurl, nc) && t.audiodownload_allowed !== false && typeof t.audiodownload === "string" && t.audiodownload.startsWith("https://"));
  // no fullcount: with fullcount=true Jamendo almost always answers empty. A full page means
  // there may be more.
  const raw = body.results?.length ?? 0;
  const totalPages = raw < PER_PAGE ? p : Math.min(50, p + 1);
  const reply = { results: list.map(track), totalPages };
  // an empty page may be one of Jamendo's blank answers: never keep it
  if (raw === 0) return json(reply);
  await db.from("jamendo_cache").upsert({ key: cacheKey, body: reply, fetched_at: new Date().toISOString() });
  await db.from("jamendo_cache").delete().lt("fetched_at", new Date(Date.now() - PRUNE_MS).toISOString());
  return json(reply);
});

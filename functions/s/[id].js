// screenforge.studio/s/<id>: the page a ScreenForge share link opens (Cloudflare Pages Function;
// `wrangler pages deploy site` picks up this functions/ folder from the repo root). It asks the
// `share` Supabase function for the link and renders the player server side, so Slack, X,
// iMessage and others unfurl the link with its title and poster. The video itself is served
// from the R2 bucket's public domain (media.screenforge.studio).

const SUPABASE_URL = "https://kxpppjdcqkxpybdawxum.supabase.co";
// the public anon key (also in crates/sf-license); the share function only answers `get` and
// `view` without a device-bound license key
const SUPABASE_ANON_KEY = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6Imt4cHBwamRjcWt4cHliZGF3eHVtIiwicm9sZSI6ImFub24iLCJpYXQiOjE3OTE4MTc3NjAsImV4cCI6MjEwNzM5Mzc2MH0.FT7EFB8DG8WW49NllafP4v6GfrGQ4ELYB-8NuvY5Jmw";

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function duration(ms) {
  if (!ms) return "";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

const STYLE = `
:root{--ground:#15161e;--ink:#1e1f29;--line:rgba(248,248,242,.10);--fg:#c9cbdc;--white:#f8f8f2;--dim:#8e90a6;--accent:#ff8a5c;--on-accent:#1e1f29;color-scheme:dark}
@media (prefers-color-scheme:light){:root{--ground:#ececf1;--ink:#f6f6fa;--line:rgba(30,31,41,.12);--fg:#2e2f3b;--white:#1a1b24;--dim:#5f6175;--accent:#d4442a;--on-accent:#fff;color-scheme:light}}
*{margin:0;padding:0;box-sizing:border-box}
body{background:var(--ground);color:var(--fg);font:15px/1.5 Inter,-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;min-height:100vh;display:flex;flex-direction:column}
header,main,footer{width:100%;max-width:1100px;margin:0 auto;padding:0 16px}
header{display:flex;align-items:center;justify-content:space-between;height:60px}
.brand{display:flex;align-items:center;gap:8px;color:var(--white);text-decoration:none;font-weight:600}
.brand img{width:22px;height:22px}
.btn{display:inline-flex;align-items:center;height:34px;padding:0 14px;border-radius:2px;border:1px solid var(--line);background:var(--ink);color:var(--white);font:inherit;font-size:13.5px;cursor:pointer;text-decoration:none}
.btn.accent{background:var(--accent);border-color:var(--accent);color:var(--on-accent);font-weight:600}
.player{background:#000;border-radius:10px;overflow:hidden;border:1px solid var(--line)}
video{display:block;width:100%;max-height:78vh;background:#000}
h1{color:var(--white);font-size:22px;font-weight:600;letter-spacing:-.01em;margin:18px 0 4px;overflow-wrap:anywhere}
.meta{color:var(--dim);font-size:13.5px;display:flex;gap:14px;flex-wrap:wrap;align-items:center}
.row{display:flex;justify-content:space-between;align-items:flex-start;gap:16px;flex-wrap:wrap}
.actions{display:flex;gap:8px;margin-top:18px}
footer{margin-top:auto;padding-top:40px;padding-bottom:28px;color:var(--dim);font-size:13px;text-align:center}
footer a{color:var(--fg)}
.gone{text-align:center;padding:18vh 0}
.gone h1{font-size:26px}
.btn svg{width:15px;height:15px;margin-right:7px;flex:none}
.qr{display:block;background:#fff;border-radius:2px}
.qr svg{display:block;width:100%;height:100%}
#qr-view{position:fixed;inset:0;z-index:10;display:none;flex-direction:column;align-items:center;justify-content:center;gap:18px;padding:16px;background:rgba(10,10,14,.9);cursor:zoom-out}
#qr-view.open{display:flex}
#qr-view .qr{width:min(72vmin,460px);height:min(72vmin,460px)}
#qr-view p{color:#f8f8f2;font-size:clamp(16px,2.8vmin,24px);text-align:center;overflow-wrap:anywhere}
`;

function page({ title, head = "", body }) {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(title)}</title><link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="preconnect" href="https://fonts.googleapis.com"><link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&display=swap" rel="stylesheet">
${head}<style>${STYLE}</style></head><body>
<header><a class="brand" href="/?ref=share"><img src="/favicon.svg" alt="">ScreenForge</a><a class="btn accent" href="/download.html?ref=share">Try ScreenForge</a></header>
${body}
<footer>Recorded and shared with <a href="/?ref=share">ScreenForge</a>, the screen recorder with an AI editor.</footer>
</body></html>`;
}

function notFound() {
  const body = `<main class="gone"><h1>This video is not available</h1><p class="meta" style="justify-content:center;margin-top:8px">Its video may not be uploaded yet, or the link was deleted or mistyped.</p></main>`;
  return new Response(page({ title: "Video not available · ScreenForge", head: `<meta name="robots" content="noindex">`, body }), { status: 404, headers: { "content-type": "text/html; charset=utf-8", "cache-control": "public, max-age=30" } });
}

// Pages Functions are not cached by Cloudflare on their own: the rendered page is kept in the
// edge cache for this long, so a visit does not wait on Supabase each time. A replaced video
// (new `rev`) shows within this time.
const PAGE_TTL = 30;

export async function onRequestGet({ params, request, waitUntil }) {
  const id = String(params.id ?? "");
  if (!/^[A-Za-z0-9]{12}$/.test(id)) return notFound();
  const key = new Request(new URL(`/s/${id}`, request.url).toString());
  const cached = await caches.default.match(key);
  if (cached) return cached;
  const res = await render(id);
  if (res.status === 200) waitUntil(caches.default.put(key, res.clone()));
  return res;
}

async function render(id) {
  let s;
  try {
    const r = await fetch(`${SUPABASE_URL}/functions/v1/share`, {
      method: "POST",
      headers: { apikey: SUPABASE_ANON_KEY, Authorization: `Bearer ${SUPABASE_ANON_KEY}`, "Content-Type": "application/json" },
      body: JSON.stringify({ action: "get", id }),
    });
    if (r.status === 404) return notFound();
    if (!r.ok) return new Response("This video could not be loaded. Try again in a moment.", { status: 502 });
    s = await r.json();
  } catch {
    return new Response("This video could not be loaded. Try again in a moment.", { status: 502 });
  }
  const title = s.title || "Screen recording";
  const date = s.created_at ? new Date(s.created_at).toLocaleDateString("en-US", { year: "numeric", month: "short", day: "numeric" }) : "";
  const og = [
    `<meta name="robots" content="noindex">`,
    `<meta property="og:type" content="video.other">`,
    `<meta property="og:site_name" content="ScreenForge">`,
    `<meta property="og:title" content="${esc(title)}">`,
    `<meta property="og:url" content="${esc(s.url)}">`,
    `<meta property="og:video" content="${esc(s.video)}">`,
    `<meta property="og:video:secure_url" content="${esc(s.video)}">`,
    `<meta property="og:video:type" content="video/mp4">`,
    s.width ? `<meta property="og:video:width" content="${s.width}"><meta property="og:video:height" content="${s.height}">` : "",
    s.poster ? `<meta property="og:image" content="${esc(s.poster)}"><meta name="twitter:image" content="${esc(s.poster)}">` : "",
    `<meta name="twitter:card" content="${s.poster ? "summary_large_image" : "summary"}">`,
    `<meta name="twitter:title" content="${esc(title)}">`,
  ].join("\n");
  const aspect = s.width && s.height ? ` style="aspect-ratio:${s.width}/${s.height}"` : "";
  const views = `${s.views + 1} view${s.views + 1 === 1 ? "" : "s"}`;
  const body = `<main>
<div class="player"><video controls playsinline preload="metadata"${aspect}${s.poster ? ` poster="${esc(s.poster)}"` : ""} src="${esc(s.video)}"></video></div>
<div class="row"><div><h1>${esc(title)}</h1><p class="meta">${[date, duration(s.duration_ms), views].filter(Boolean).map(esc).join("<span>·</span>")}</p></div>
<div class="actions"><button class="btn" id="copy" type="button">Copy link</button><button class="btn" id="qr" type="button" title="Show a QR code to scan"><svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="1.5" y="1.5" width="5" height="5"/><rect x="9.5" y="1.5" width="5" height="5"/><rect x="1.5" y="9.5" width="5" height="5"/><path d="M9.5 9.5h2.5v2.5h-2.5zM12.5 12.5h2v2h-2z" fill="currentColor" stroke="none"/></svg>QR code</button><a class="btn" href="${esc(s.video)}" download>Download</a></div></div>
</main>
<div id="qr-view" role="dialog" aria-modal="true" aria-label="QR code for this video"><span class="qr" id="qr-big"></span><p>${esc(String(s.url).replace(/^https?:\/\//, ""))}</p><button class="btn" id="qr-close" type="button">Close</button></div>
<script src="https://cdn.jsdelivr.net/npm/qrcode-generator@1.4.4/qrcode.min.js" integrity="sha384-lQXOAyZwHXE55JFyrOMB7nY2Wv+m5ZWNtJcHrd1rceRQXAYNLak8ukN5TjBTcIwz" crossorigin="anonymous"></script>
<script>
(function(){
  // the link as a QR code (SVG, black on white with its quiet zone), large over the page from
  // the QR code button, for a room to scan off a projector
  var view=document.getElementById("qr-view");
  if(typeof qrcode!=="function"){document.getElementById("qr").style.display="none";return}
  var q=qrcode(0,"M");q.addData(${JSON.stringify(s.url)});q.make();
  var n=q.getModuleCount(),z=4,d="";
  for(var y=0;y<n;y++)for(var x=0;x<n;x++)if(q.isDark(y,x))d+="M"+(x+z)+" "+(y+z)+"h1v1h-1z";
  var svg='<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 '+(n+2*z)+" "+(n+2*z)+'" shape-rendering="crispEdges" role="img" aria-label="QR code"><path fill="#000" d="'+d+'"/></svg>';
  document.getElementById("qr-big").innerHTML=svg;
  function open(){view.classList.add("open")}function close(){view.classList.remove("open")}
  document.getElementById("qr").onclick=open;
  view.onclick=close;document.addEventListener("keydown",function(e){if(e.key==="Escape")close()});
})();
document.getElementById("copy").onclick=function(){var b=this;navigator.clipboard.writeText(${JSON.stringify(s.url)}).then(function(){b.textContent="Copied";setTimeout(function(){b.textContent="Copy link"},1500)})};
try{fetch(${JSON.stringify(`${SUPABASE_URL}/functions/v1/share`)},{method:"POST",keepalive:true,headers:{apikey:${JSON.stringify(SUPABASE_ANON_KEY)},Authorization:"Bearer "+${JSON.stringify(SUPABASE_ANON_KEY)},"Content-Type":"application/json"},body:JSON.stringify({action:"view",id:${JSON.stringify(id)}})})}catch(e){}
</script>`;
  return new Response(page({ title: `${title} · ScreenForge`, head: og, body }), {
    headers: { "content-type": "text/html; charset=utf-8", "cache-control": `public, max-age=${PAGE_TTL}` },
  });
}

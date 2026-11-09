#!/usr/bin/env python3
"""Release notes pages for the site, built from the GitHub releases.

Writes into site/releases/ (generated, not committed):
  index.html          the digest: every release, newest first, grouped by month
  v<X.Y.Z>/index.html one page per release, with its downloads
  feed.xml            Atom feed of recent releases
  sitemap.xml         the pages above (listed in site/robots.txt)

The pages reuse site/terms.html's head, navigation and footer, so they match the site. Run from
anywhere; .github/workflows/site.yml runs it before every deploy, and the release workflow
starts that deploy once a release is published. GH_TOKEN or GITHUB_TOKEN, when set, raises the
GitHub API rate limit.

  python3 scripts/gen-releases.py
"""
import html
import json
import os
import re
import shutil
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

REPO = os.environ.get("REPO", "mehfuzh/screenforge")
SITE = "https://screenforge.studio"
ROOT = Path(__file__).resolve().parent.parent
SITE_DIR = ROOT / "site"
OUT = SITE_DIR / "releases"
# commit subjects that are not changes to the app
SKIP = re.compile(r"^(Release \d|Merge |Cargo\.lock|Site:|Site |CI\b|ci:|Update screencast\b|README)", re.I)
# how many changes the digest shows per release before "and n more"
DIGEST_ITEMS = 6
FEED_ITEMS = 30


def fetch_releases():
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    out, page = [], 1
    while True:
        req = urllib.request.Request(f"https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}", headers={"Accept": "application/vnd.github+json", "User-Agent": "screenforge-site"})
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        with urllib.request.urlopen(req, timeout=30) as r:
            batch = json.load(r)
        if not batch:
            break
        out += [r for r in batch if not r.get("draft") and not r.get("prerelease") and r.get("published_at")]
        page += 1
    out.sort(key=lambda r: r["published_at"], reverse=True)
    return out


def changes(body):
    """The bullets under the release's "## Changes" heading (the commit subjects)."""
    items, inside = [], False
    for line in (body or "").splitlines():
        s = line.strip()
        if s.startswith("## "):
            if inside:
                break
            inside = s.lower().startswith("## changes")
            continue
        if s.startswith("<!--") and inside:
            break
        if inside and s.startswith("- "):
            text = s[2:].strip()
            if text and not SKIP.match(text):
                items.append(text)
    return items


def inline(text):
    """Escape, with `code` spans."""
    return re.sub(r"`([^`]+)`", r"<code>\1</code>", html.escape(text))


def version(r):
    return r["tag_name"].lstrip("v")


def date(r):
    return datetime.strptime(r["published_at"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def long_date(d):
    return f"{d.day} {d.strftime('%B %Y')}"


def summary(items, limit=155):
    s = "; ".join(items) or "Maintenance release."
    return s if len(s) <= limit else s[: limit - 1].rsplit(" ", 1)[0] + "…"


class Template:
    """site/terms.html cut into head, navigation and footer."""

    def __init__(self):
        t = (SITE_DIR / "terms.html").read_text(encoding="utf-8")
        self.head = t[: t.index("</head>")]
        self.nav = t[t.index("<nav>") : t.index("</nav>") + len("</nav>")]
        self.foot = t[t.index("<footer>") :]
        if 'href="releases/"' not in self.foot:
            self.foot = self.foot.replace('<a href="terms.html">terms</a>', '<a href="terms.html">terms</a>\n      <a href="releases/">releases</a>', 1)

    def page(self, depth, url, title, description, body, extra_head=""):
        head = self.head
        e = html.escape
        head = re.sub(r"<title>.*?</title>", f"<title>{e(title)}</title>", head, count=1)
        head = re.sub(r'<meta name="description" content="[^"]*">', f'<meta name="description" content="{e(description)}">', head, count=1)
        head = re.sub(r'<link rel="canonical" href="[^"]*">', f'<link rel="canonical" href="{url}">', head, count=1)
        head = re.sub(r'<meta property="og:url" content="[^"]*">', f'<meta property="og:url" content="{url}">', head, count=1)
        head = re.sub(r'<meta property="og:title" content="[^"]*">', f'<meta property="og:title" content="{e(title)}">', head, count=1)
        head = re.sub(r'<meta property="og:description" content="[^"]*">', f'<meta property="og:description" content="{e(description)}">', head, count=1)
        head = re.sub(r'<link rel="alternate" hreflang="[^"]*" href="[^"]*">\n', "", head)
        # the template's links are relative to the site root; `body`'s to the page itself
        up = lambda part: re.sub(r'((?:href|src)=")(?!https?:|/|#|mailto:|data:)', r"\g<1>" + "../" * depth, part)
        return up(head) + STYLE + extra_head + "</head>\n<body>\n\n" + up(self.nav) + "\n\n<main class=\"doc\">\n" + body + "\n</main>\n\n" + up(self.foot)


STYLE = """<style>
  .rel-month { margin-top: 56px; }
  .rel { border-top: 1px solid var(--line); padding: 22px 0 6px; }
  .rel h3 { display: flex; align-items: baseline; gap: 14px; margin: 0 0 10px; font-size: 19px; letter-spacing: -.01em; }
  .rel h3 a { color: var(--white); text-decoration: none; }
  .rel h3 a:hover { color: var(--jade); }
  .rel time, .rel-meta { font-family: var(--mono); font-size: 12.5px; color: var(--dim); font-weight: 400; }
  .rel ul, .doc .changes { margin: 0 0 8px; }
  .rel .more { font-size: 14px; }
  .dl { display: flex; flex-wrap: wrap; gap: 10px; margin: 18px 0 8px; }
  .dl a { border: 1px solid var(--line-2); border-radius: 8px; padding: 9px 14px; font-size: 14px; text-decoration: none; color: var(--white); }
  .dl a:hover { border-color: var(--jade); }
  .pager { display: flex; justify-content: space-between; gap: 16px; margin-top: 56px; font-size: 14.5px; }
</style>
"""

# the release files worth offering, by platform (versioned names)
DOWNLOADS = [("macOS", re.compile(r"^ScreenForge-[\d.]+\.dmg$")), ("Windows installer", re.compile(r"^ScreenForge-[\d.]+-windows-.*-setup\.exe$")), ("Windows .zip", re.compile(r"^ScreenForge-[\d.]+-windows-.*\.zip$")), ("Linux .deb", re.compile(r"^screenforge_[\d.]+_.*\.deb$")), ("Linux .tar.gz", re.compile(r"^ScreenForge-[\d.]+-linux-.*\.tar\.gz$"))]


def downloads(r):
    links = []
    for label, pat in DOWNLOADS:
        a = next((a for a in r.get("assets", []) if pat.match(a["name"])), None)
        if a:
            links.append(f'<a href="{html.escape(a["browser_download_url"])}" rel="nofollow">{label}</a>')
    return links


def release_page(t, r, newer, older):
    v, d, items = version(r), date(r), changes(r["body"])
    url = f"{SITE}/releases/v{v}/"
    title = f"ScreenForge {v} release notes"
    lis = "\n".join(f"    <li>{inline(i)}</li>" for i in items) or "    <li>Maintenance release: fixes and internal changes.</li>"
    dl = downloads(r)
    ld = {"@context": "https://schema.org", "@type": "SoftwareApplication", "name": "ScreenForge", "softwareVersion": v, "datePublished": d.strftime("%Y-%m-%d"), "operatingSystem": "macOS, Windows, Linux", "applicationCategory": "MultimediaApplication", "url": url, "releaseNotes": url, "downloadUrl": r["html_url"]}
    pager = '<nav class="pager" aria-label="Other releases">'
    pager += f'<a href="../v{version(older)}/">← {version(older)}</a>' if older else "<span></span>"
    pager += f'<a href="../v{version(newer)}/">{version(newer)} →</a>' if newer else "<span></span>"
    pager += "</nav>"
    body = f"""  <div class="eyebrow"><a href="../">Release notes</a></div>
  <h1>ScreenForge {v}</h1>
  <p class="stamp">Released <time datetime="{d.strftime('%Y-%m-%d')}">{long_date(d)}</time></p>

  <h2>What changed</h2>
  <ul class="changes">
{lis}
  </ul>

  <h2>Download</h2>
  {'<div class="dl">' + ''.join(dl) + '</div>' if dl else ''}
  <p>Or get the newest version from the <a href="../../download.html">download page</a>. Every file and the full changelog are on <a href="{html.escape(r['html_url'])}">GitHub</a>.</p>
  {pager}"""
    extra = f'<script type="application/ld+json">{json.dumps(ld)}</script>\n<link rel="alternate" type="application/atom+xml" title="ScreenForge releases" href="{SITE}/releases/feed.xml">\n'
    return t.page(2, url, title, f"ScreenForge {v} ({long_date(d)}): " + summary(items, 120), body, extra)


def digest_page(t, releases):
    url = f"{SITE}/releases/"
    latest = releases[0]
    months, order = {}, []
    for r in releases:
        k = date(r).strftime("%B %Y")
        if k not in months:
            months[k] = []
            order.append(k)
        months[k].append(r)
    parts = []
    for k in order:
        rs = months[k]
        parts.append(f'  <h2 class="rel-month">{k} <span class="n">{len(rs)} release{"s" if len(rs) != 1 else ""}</span></h2>')
        for r in rs:
            v, items = version(r), changes(r["body"])
            shown = "\n".join(f"      <li>{inline(i)}</li>" for i in items[:DIGEST_ITEMS]) or "      <li>Maintenance release.</li>"
            more = f'\n    <p class="more"><a href="v{v}/">and {len(items) - DIGEST_ITEMS} more changes</a></p>' if len(items) > DIGEST_ITEMS else ""
            d = date(r)
            parts.append(f"""  <article class="rel">
    <h3><a href="v{v}/">ScreenForge {v}</a><time datetime="{d.strftime('%Y-%m-%d')}">{d.day} {d.strftime('%b')}</time></h3>
    <ul>
{shown}
    </ul>{more}
  </article>""")
    body = f"""  <div class="eyebrow">Changelog</div>
  <h1>Release notes</h1>
  <p class="stamp">Latest: <a href="v{version(latest)}/">ScreenForge {version(latest)}</a>, {long_date(date(latest))} · {len(releases)} releases · <a href="feed.xml">Atom feed</a></p>
  <p>What changed in every version of ScreenForge, the screen recorder and editor for macOS, Windows and Linux. Newest first; each version has its own page with its downloads.</p>
{chr(10).join(parts)}"""
    extra = f'<link rel="alternate" type="application/atom+xml" title="ScreenForge releases" href="{SITE}/releases/feed.xml">\n'
    desc = f"What changed in each ScreenForge version, newest first. Latest: {version(latest)} ({long_date(date(latest))}): " + summary(changes(latest["body"]), 90)
    return t.page(1, url, "ScreenForge release notes", desc, body, extra)


def feed(releases):
    e = html.escape
    entries = []
    for r in releases[:FEED_ITEMS]:
        v, items = version(r), changes(r["body"])
        content = "<ul>" + "".join(f"<li>{inline(i)}</li>" for i in items) + "</ul>"
        entries.append(f"""  <entry>
    <title>ScreenForge {v}</title>
    <link href="{SITE}/releases/v{v}/"/>
    <id>{SITE}/releases/v{v}/</id>
    <updated>{r['published_at']}</updated>
    <summary>{e(summary(items, 200))}</summary>
    <content type="html">{e(content)}</content>
  </entry>""")
    return f"""<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>ScreenForge releases</title>
  <link href="{SITE}/releases/"/>
  <link rel="self" href="{SITE}/releases/feed.xml"/>
  <id>{SITE}/releases/</id>
  <updated>{releases[0]['published_at']}</updated>
  <author><name>ScreenForge</name></author>
{chr(10).join(entries)}
</feed>
"""


def sitemap(releases):
    urls = [(f"{SITE}/releases/", date(releases[0]))] + [(f"{SITE}/releases/v{version(r)}/", date(r)) for r in releases]
    rows = "\n".join(f"  <url><loc>{u}</loc><lastmod>{d.strftime('%Y-%m-%d')}</lastmod></url>" for u, d in urls)
    return f'<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n{rows}\n</urlset>\n'


def main():
    releases = fetch_releases()
    if not releases:
        sys.exit("no published releases found")
    t = Template()
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    (OUT / "index.html").write_text(digest_page(t, releases), encoding="utf-8")
    for i, r in enumerate(releases):
        d = OUT / f"v{version(r)}"
        d.mkdir()
        newer = releases[i - 1] if i > 0 else None
        older = releases[i + 1] if i + 1 < len(releases) else None
        (d / "index.html").write_text(release_page(t, r, newer, older), encoding="utf-8")
    (OUT / "feed.xml").write_text(feed(releases), encoding="utf-8")
    (OUT / "sitemap.xml").write_text(sitemap(releases), encoding="utf-8")
    print(f"wrote {len(releases)} release pages, the digest, feed and sitemap to {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""The site's shared footer: one source, filled into every page.

scripts/site/footer.en.html and footer.es.html hold the footer with links relative to the
site root. Every page under site/ gets the footer of its language (pages under site/es/ the
Spanish one), with those links rewritten relative to the page, between its <footer> tags.
scripts/gen-releases.py uses the same footer for the release notes pages.

  python3 scripts/site_partials.py           # update the pages in place
  python3 scripts/site_partials.py --check   # list pages whose footer differs; exit 1 if any

Edit the footer in scripts/site/, never in a page: the site workflow runs this before every
deploy, so a page's own copy is replaced.
"""
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
PARTIALS = ROOT / "scripts" / "site"
FOOTER = re.compile(r"<footer>.*?</footer>", re.S)
RELATIVE = re.compile(r'((?:href|src)=")(?!https?:|/|#|mailto:|data:)([^"]*)"')


def lang(page: Path) -> str:
    return "es" if page.relative_to(SITE).parts[0] == "es" else "en"


def footer(page: Path) -> str:
    """The footer for `page`, its links relative to the page's folder."""
    text = (PARTIALS / f"footer.{lang(page)}.html").read_text(encoding="utf-8").strip()
    here = page.parent.relative_to(SITE)

    def rel(m):
        target = m.group(2)
        slash = target.endswith("/")
        out = os.path.relpath(target.rstrip("/") or ".", here).replace(os.sep, "/")
        return f'{m.group(1)}{out}{"/" if slash else ""}"'

    return RELATIVE.sub(rel, text)


def apply(page: Path, html: str) -> str:
    if not FOOTER.search(html):
        return html
    f = footer(page)
    return FOOTER.sub(lambda _: f, html, count=1)


def main():
    check = "--check" in sys.argv[1:]
    changed = []
    for page in sorted(SITE.rglob("*.html")):
        html = page.read_text(encoding="utf-8")
        new = apply(page, html)
        if new != html:
            changed.append(page.relative_to(ROOT))
            if not check:
                page.write_text(new, encoding="utf-8")
    if check:
        for p in changed:
            print(f"footer differs: {p}")
        sys.exit(1 if changed else 0)
    print(f"footer updated on {len(changed)} page(s)")


if __name__ == "__main__":
    main()

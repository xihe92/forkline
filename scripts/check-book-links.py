#!/usr/bin/env python3
"""Check the built book's internal links: every relative link must point at a
page that exists and, when it names an anchor, at a heading that exists.

    mdbook build docs && python3 scripts/check-book-links.py docs/book

External links (http, https, mailto) are not fetched.
"""
import html.parser
import pathlib
import sys
import urllib.parse


class Page(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.links, self.ids = [], set()

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.add(attrs["id"])
        if tag == "a" and attrs.get("href"):
            self.links.append(attrs["href"])


def parse(path):
    page = Page()
    page.feed(path.read_text(encoding="utf-8"))
    return page


def main(root):
    root = pathlib.Path(root).resolve()
    pages = {p: parse(p) for p in root.rglob("*.html")}
    problems = []
    for path, page in pages.items():
        if path.name in ("print.html", "404.html", "toc.html"):
            continue
        for href in page.links:
            url = urllib.parse.urlsplit(href)
            if url.scheme or href.startswith("//"):
                continue
            target = path if not url.path else (path.parent / url.path).resolve()
            if target.is_dir():
                target = target / "index.html"
            if target not in pages:
                problems.append(f"{path.relative_to(root)}: {href}: no such page")
            elif url.fragment and url.fragment not in pages[target].ids:
                problems.append(f"{path.relative_to(root)}: {href}: no heading #{url.fragment}")
    for p in problems:
        print(p)
    print(f"{len(pages)} pages, {len(problems)} broken links")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "docs/book"))

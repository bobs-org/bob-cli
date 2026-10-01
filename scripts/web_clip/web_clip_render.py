"""Placeholder reader renderer for the bob web-clip adapter.

Reads the ``article.json`` the adapter wrote, assembles a bare HTML page
(like the planning prototype), and prints it with a separate headless
browser supplied by the adapter's ``launcher``. The ``reader-template``
phase replaces this module with the real Bob-owned print template; the
``render(article_json_path, out_pdf, launcher)`` signature stays.
"""

from __future__ import annotations

import html
import json
import os


def assemble(article: dict) -> str:
    """Build a bare reader page from an ``article.json`` dict."""
    title = html.escape(article.get("title") or "Untitled")
    author = html.escape(article.get("author") or "")
    published = html.escape(article.get("published") or "")
    source_url = html.escape(article.get("source_url") or "")
    body = article.get("html") or ""
    byline = " · ".join(part for part in (author, published) if part)
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>body {{ max-width: 40em; margin: 2em auto; font-family: serif; }}</style>
</head>
<body>
<p><a href="{source_url}">{source_url}</a></p>
<h1>{title}</h1>
<p>{byline}</p>
{body}
</body>
</html>
"""


def render(article_json_path: str, out_pdf: str, launcher) -> dict:
    """Print ``article_json_path`` to ``out_pdf``; return ``{"pdf_bytes"}``."""
    with open(article_json_path, encoding="utf-8") as handle:
        article = json.load(handle)
    workdir = os.path.dirname(os.path.abspath(article_json_path))
    page_path = os.path.join(workdir, "placeholder.html")
    with open(page_path, "w", encoding="utf-8") as handle:
        handle.write(assemble(article))
    browser = launcher(headless=True)
    try:
        page = browser.new_page()
        try:
            page.goto("file://" + page_path, wait_until="load", timeout=30000)
            page.pdf(path=out_pdf, format="Letter", print_background=True)
        finally:
            page.close()
    finally:
        browser.close()
    return {"pdf_bytes": os.path.getsize(out_pdf)}

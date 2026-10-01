"""Bob-owned reader renderer for the web-clip adapter.

Reads the ``article.json`` the adapter wrote, normalizes images with
Pillow, assembles a Bob-owned HTML/CSS print page (``template/reader.css``
plus bundled ``fonts/``), and prints it with a separate headless browser
supplied by the adapter's ``launcher``.

``render(article_json_path, out_pdf, launcher)`` returns
``{"pdf_bytes", "images", "warnings"}``.
"""

from __future__ import annotations

import datetime
import html
import os
import re
from urllib.parse import urlsplit

RENDER_DIR = os.path.dirname(os.path.abspath(__file__))
FONTS_DIR = os.path.join(RENDER_DIR, "fonts")
TEMPLATE_DIR = os.path.join(RENDER_DIR, "template")
READER_CSS_PATH = os.path.join(TEMPLATE_DIR, "reader.css")

RENDER_HOST = "bob-clip.invalid"
ARTICLE_PATH = "/article.html"

CSP = (
    "default-src 'none'; "
    "img-src https://bob-clip.invalid data:; "
    "font-src https://bob-clip.invalid; "
    "style-src https://bob-clip.invalid 'unsafe-inline'"
)

PDF_WARN_BYTES = 50 * 1024 * 1024
PDF_MAX_BYTES = 95 * 1024 * 1024
IMAGE_LONG_EDGE_MAX = 1600
IMAGE_SMALL_EDGE = 64


class RenderError(Exception):
    """Printing or assembly failed; the adapter maps this to ``render``."""


# ---------------------------------------------------------------------------
# Image normalization (Pillow) into ``render/assets/``.
# ---------------------------------------------------------------------------

_IMG_TAG_RE = re.compile(r"<img\b[^>]*>", re.IGNORECASE)
_SRC_ATTR_RE = re.compile(
    r"""\bsrc\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)""", re.IGNORECASE)

_CONTENT_TYPE = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".woff2": "font/woff2",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".avif": "image/avif",
    ".svg": "image/svg+xml",
}


def _clean_inline_svg(markup: str) -> str:
    """Strip active content from an SVG before it is printed."""
    markup = re.sub(r"<script\b.*?</script\s*>", "", markup,
                    flags=re.IGNORECASE | re.DOTALL)
    markup = re.sub(r"<foreignObject\b.*?</foreignObject\s*>", "", markup,
                    flags=re.IGNORECASE | re.DOTALL)
    markup = re.sub(r"""\s+on[a-zA-Z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)""",
                    "", markup)
    markup = re.sub(r"""\s+(?:xlink:)?href\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)""",
                    "", markup)
    return markup


def _normalize_raster(data: bytes) -> tuple[bytes, tuple[int, int]] | None:
    """Downscale, flatten, and JPEG-encode one raster; ``None`` when small.

    Returns ``(jpeg_bytes, (width, height))``, or ``None`` when both
    dimensions are under ``IMAGE_SMALL_EDGE``. Raises on undecodable bytes.
    """
    from PIL import Image

    with Image.open(__import__("io").BytesIO(data)) as img:
        try:
            img.seek(0)
        except Exception:
            pass
        width, height = int(img.width), int(img.height)
        if width < IMAGE_SMALL_EDGE and height < IMAGE_SMALL_EDGE:
            return None
        frame = img.convert("RGB") if img.mode in ("RGB", "L") else None
        if frame is None:
            canvas = Image.new("RGB", (width, height), (255, 255, 255))
            alpha = None
            if "A" in img.getbands():
                alpha = img.getchannel("A")
            elif img.mode == "P" and "transparency" in img.info:
                try:
                    alpha = img.convert("RGBA").getchannel("A")
                except Exception:  # noqa: BLE001 - fall back to opaque
                    alpha = None
            base = img.convert("RGB")
            if alpha is not None:
                canvas.paste(base, mask=alpha)
            else:
                canvas = base
            frame = canvas
        long_edge = max(frame.width, frame.height)
        if long_edge > IMAGE_LONG_EDGE_MAX:
            frame.thumbnail((IMAGE_LONG_EDGE_MAX, IMAGE_LONG_EDGE_MAX),
                            Image.LANCZOS)
        out = __import__("io").BytesIO()
        frame.save(out, "JPEG", quality=82, progressive=True)
        return out.getvalue(), (frame.width, frame.height)


def normalize_images(markup: str, assets_dir: str,
                     render_assets_dir: str,
                     warnings: list[str]) -> tuple[str, dict]:
    """Copy/normalize ``assets/`` images into ``render/assets/``.

    Drops (and warns about) images that fail to decode; skips tiny ones.
    Returns ``(rewritten_markup, manifest)``.
    """
    manifest = {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0}
    os.makedirs(render_assets_dir, exist_ok=True)

    def _replace(match: re.Match) -> str:
        tag = match.group(0)
        manifest["total"] += 1
        src_match = _SRC_ATTR_RE.search(tag)
        raw_src = (src_match.group(1).strip("\"'") if src_match else "")
        raw_src = html.unescape(raw_src)
        if raw_src.startswith("data:"):
            # Localization already decoded data: URIs; anything left is
            # printable as-is under the CSP.
            manifest["kept"] += 1
            return tag
        if not raw_src.startswith("assets/") or "/" in raw_src[len("assets/"):]:
            manifest["failed"] += 1
            warnings.append(f"dropped image with unexpected src: {raw_src[:80]}")
            return ""
        name = raw_src[len("assets/"):]
        src_path = os.path.join(assets_dir, name)
        try:
            with open(src_path, "rb") as handle:
                data = handle.read()
        except OSError:
            manifest["failed"] += 1
            warnings.append(f"dropped missing image asset: {name}")
            return ""
        if name.lower().endswith(".svg"):
            try:
                cleaned = _clean_inline_svg(
                    data.decode("utf-8", errors="replace"))
            except Exception as exc:  # noqa: BLE001 - drop and continue
                manifest["failed"] += 1
                warnings.append(f"dropped unreadable SVG {name}: {exc}")
                return ""
            with open(os.path.join(render_assets_dir, name), "w",
                     encoding="utf-8") as handle:
                handle.write(cleaned)
            manifest["kept"] += 1
            return tag
        try:
            result = _normalize_raster(data)
        except Exception as exc:  # noqa: BLE001 - drop and continue
            manifest["failed"] += 1
            warnings.append(f"dropped undecodable image {name}: {exc}")
            return ""
        if result is None:
            manifest["skipped_small"] += 1
            return ""
        jpeg, _dims = result
        stem = name.rsplit(".", 1)[0] if "." in name else name
        out_name = f"{stem}.jpg"
        with open(os.path.join(render_assets_dir, out_name), "wb") as handle:
            handle.write(jpeg)
        manifest["kept"] += 1
        return _SRC_ATTR_RE.sub(f'src="assets/{out_name}"', tag, count=1)

    return _IMG_TAG_RE.sub(_replace, markup), manifest


# ---------------------------------------------------------------------------
# HTML assembly.
# ---------------------------------------------------------------------------

_HEADING_RE = re.compile(r"<(/?)h([1-6])\b", re.IGNORECASE)
_FIRST_PARA_RE = re.compile(r"<p\b[^>]*>(.*?)</p>",
                            re.IGNORECASE | re.DOTALL)
_TAG_RE = re.compile(r"<[^>]*>")


def _visible_text(markup: str) -> str:
    """Best-effort visible text of an HTML fragment."""
    return html.unescape(_TAG_RE.sub(" ", markup))


def _squash(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip().casefold()


def format_long_date(raw: str | None) -> str | None:
    """``YYYY-MM-DD`` to ``Month D, YYYY``; ``None`` when unknown."""
    if not raw:
        return None
    try:
        parsed = datetime.date(int(raw[0:4]), int(raw[5:7]), int(raw[8:10]))
    except (ValueError, IndexError):
        return None
    return f"{parsed.strftime('%B')} {parsed.day}, {parsed.year}"


def short_footer_title(title: str, limit: int = 60) -> str:
    """Truncate the footer title to about ``limit`` characters."""
    collapsed = re.sub(r"\s+", " ", title).strip()
    if len(collapsed) <= limit:
        return collapsed
    cut = collapsed[:limit].rsplit(" ", 1)[0] or collapsed[:limit]
    return cut + "…"


def css_escape_string(text: str) -> str:
    """Escape ``text`` for a double-quoted CSS string."""
    cleaned = re.sub(r"[\x00-\x1f\x7f]", " ", text)
    return cleaned.replace("\\", "\\\\").replace('"', '\\"')


def shift_headings(markup: str) -> str:
    """Shift headings so the article's highest heading renders as ``h2``."""

    levels = [int(match.group(2)) for match in _HEADING_RE.finditer(markup)
              if not match.group(1)]
    if not levels:
        return markup
    shift = 2 - min(levels)
    if shift <= 0:
        return markup

    def _shift(match: re.Match) -> str:
        closing, level = match.group(1), int(match.group(2))
        return f"<{closing}h{min(level + shift, 6)}"

    return _HEADING_RE.sub(_shift, markup)


def show_dek(description: str | None, body_html: str) -> str | None:
    """Return the dek, or ``None`` when it repeats the first paragraph."""
    if not description or not description.strip():
        return None
    dek = description.strip()
    first = _FIRST_PARA_RE.search(body_html)
    if first and _squash(dek) and _squash(_visible_text(first.group(1))):
        if _squash(_visible_text(first.group(1))).startswith(_squash(dek)):
            return None
    return dek


def assemble(article: dict) -> str:
    """Build the full reader page from an ``article.json`` dict."""
    title = article.get("title") or "Untitled"
    author = article.get("author")
    published = format_long_date(article.get("published"))
    captured = format_long_date(article.get("captured"))
    site = article.get("site")
    description = article.get("description")
    source_url = article.get("source_url") or ""
    word_count = article.get("word_count") or 0
    body = shift_headings(article.get("html") or "")

    byline = " · ".join(part for part in (author, published) if part)
    provenance = [f'<a href="{html.escape(source_url)}">'
                  f"{html.escape(source_url)}</a>"]
    if captured:
        provenance.append(f"Captured {html.escape(captured)}")
    if word_count:
        provenance.append(f"{word_count} words")
    dek = show_dek(description, body)

    masthead = [ '<header class="masthead">']
    if site:
        masthead.append(
            f'<p class="masthead-site">{html.escape(site)}</p>')
    masthead.append(
        f'<h1 class="masthead-title">{html.escape(title)}</h1>')
    if dek:
        masthead.append(
            f'<p class="masthead-dek">{html.escape(dek)}</p>')
    if byline:
        masthead.append(
            f'<p class="masthead-byline">{html.escape(byline)}</p>')
    masthead.append(
        '<p class="masthead-provenance">'
        + " · ".join(f"<span>{part}</span>" for part in provenance)
        + "</p>")
    masthead.append("</header>")

    footer_title = css_escape_string(short_footer_title(title))

    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{html.escape(CSP)}">
<title>{html.escape(title)}</title>
<link rel="stylesheet" href="/template/reader.css">
<style>
@page {{
  @bottom-left {{
    content: "{footer_title}";
  }}
}}
@page :first {{
  @bottom-left {{
    content: none;
  }}
}}
</style>
</head>
<body>
{"".join(masthead)}
<div class="article-body">
{body}
</div>
</body>
</html>
"""


# ---------------------------------------------------------------------------
# Offline print.
# ---------------------------------------------------------------------------

def _route_handler(render_dir: str):
    """Serve ``render/`` plus bundled fonts/template; abort everything else."""

    def _handle(route) -> None:
        url = route.request.url
        try:
            parts = urlsplit(url)
        except ValueError:
            route.abort()
            return
        if parts.hostname != RENDER_HOST:
            route.abort()
            return
        path = parts.path or "/"
        base: str | None = None
        if path == ARTICLE_PATH:
            base = os.path.join(render_dir, "article.html")
        elif path.startswith("/assets/"):
            name = path[len("/assets/"):]
            if not name or "/" in name or "\\" in name:
                route.abort()
                return
            base = os.path.join(render_dir, "assets", name)
        elif path.startswith("/fonts/"):
            name = path[len("/fonts/"):]
            if not name or "/" in name or "\\" in name:
                route.abort()
                return
            base = os.path.join(FONTS_DIR, name)
        elif path.startswith("/template/"):
            name = path[len("/template/"):]
            if not name or "/" in name or "\\" in name:
                route.abort()
                return
            base = os.path.join(TEMPLATE_DIR, name)
        else:
            route.abort()
            return
        try:
            with open(base, "rb") as handle:
                body = handle.read()
        except OSError:
            route.abort()
            return
        ext = os.path.splitext(base)[1].lower()
        route.fulfill(status=200,
                      headers={"content-type": _CONTENT_TYPE.get(
                          ext, "application/octet-stream")},
                      body=body)

    return _handle


def print_pdf(render_dir: str, out_pdf: str, launcher) -> None:
    """Print ``render/article.html`` offline to ``out_pdf``."""
    try:
        with open(READER_CSS_PATH, encoding="utf-8") as handle:
            handle.read()
    except OSError as exc:
        raise RenderError(f"reader template is missing: {exc}") from exc
    browser = launcher(headless=True)
    try:
        context = browser.new_context(locale="en-US",
                                      color_scheme="light",
                                      reduced_motion="reduce",
                                      service_workers="block")
        try:
            context.route(f"https://{RENDER_HOST}/**",
                          _route_handler(render_dir))
            page = context.new_page()
            try:
                page.goto(f"https://{RENDER_HOST}{ARTICLE_PATH}",
                          wait_until="load", timeout=30000)
                page.emulate_media(media="print")
                page.evaluate(
                    "Promise.race([document.fonts.ready.then(() => 1), "
                    "new Promise((r) => setTimeout(() => r(0), 10000))])"
                )
                page.pdf(path=out_pdf, format="Letter",
                         print_background=True, outline=True, tagged=True,
                         prefer_css_page_size=True,
                         display_header_footer=False)
            finally:
                page.close()
        finally:
            context.close()
    except RenderError:
        raise
    except Exception as exc:
        raise RenderError(f"printing failed: {exc}") from exc
    finally:
        try:
            browser.close()
        except Exception:  # noqa: BLE001 - best effort
            pass


# ---------------------------------------------------------------------------
# Entry point (adapter protocol calls this).
# ---------------------------------------------------------------------------

def render(article_json_path: str, out_pdf: str, launcher) -> dict:
    """Normalize, assemble, and print; return pdf/images/warnings."""
    import json

    with open(article_json_path, encoding="utf-8") as handle:
        article = json.load(handle)
    workdir = os.path.dirname(os.path.abspath(article_json_path))
    assets_dir = os.path.join(workdir, "assets")
    render_dir = os.path.join(workdir, "render")
    render_assets_dir = os.path.join(render_dir, "assets")
    os.makedirs(render_assets_dir, exist_ok=True)

    warnings: list[str] = []
    markup, images = normalize_images(article.get("html") or "",
                                     assets_dir, render_assets_dir, warnings)
    article = dict(article, html=markup)
    page_path = os.path.join(render_dir, "article.html")
    with open(page_path, "w", encoding="utf-8") as handle:
        handle.write(assemble(article))

    print_pdf(render_dir, out_pdf, launcher)

    pdf_bytes = os.path.getsize(out_pdf)
    if pdf_bytes > PDF_WARN_BYTES:
        warnings.append(
            f"printed PDF is {pdf_bytes / 1048576:.1f} MiB "
            "(vault sync refuses 95 MiB or more)")
    return {"pdf_bytes": pdf_bytes, "images": images, "warnings": warnings}

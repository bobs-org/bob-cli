"""Bob-owned reader renderer for the web-clip adapter.

Reads the ``article.json`` the adapter wrote, normalizes images with
Pillow, assembles a Bob-owned HTML/CSS print page (``template/reader.css``
plus bundled ``fonts/``), and prints it with a separate headless browser
supplied by the adapter's ``launcher``.

``render(article_json_path, out_pdf, launcher)`` returns
``{"pdf_bytes", "images", "warnings", "source_html", "code_anchors"}``.

``render_collection(parts, collection, workdir, out_pdf, launcher)``
typesets ordered article.json parts into one PDF: an opening page with
linked contents, then a fresh page and bookmark per part.
"""

from __future__ import annotations

import atexit
import datetime
import hashlib
import html
import json
import os
import re
import shutil
import sys
import tempfile
from urllib.parse import urljoin, urlsplit

RENDER_DIR = os.path.dirname(os.path.abspath(__file__))
FONTS_DIR = os.path.join(RENDER_DIR, "fonts")
# Byte-identical payloads moved to the `bob-obsidian-assets` crate. Installed
# copies live in `fonts/` next to this renderer; direct source execution
# falls back to the crate's repository locations.
ASSETS_CRATE_DIR = os.path.normpath(
    os.path.join(RENDER_DIR, "..", "..", "crates", "bob-obsidian-assets"))
ASSETS_FONTS_DIR = os.path.join(
    ASSETS_CRATE_DIR, "scripts", "web_clip", "fonts")
PRINT_FONTS_DIR = os.path.normpath(
    os.path.join(ASSETS_CRATE_DIR, "assets", "print_fonts"))
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


class BrowserLaunchError(RenderError):
    """A renderer browser could not launch; compose maps this to ``browser``."""


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
    ".otf": "font/otf",
    ".ttf": "font/ttf",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".avif": "image/avif",
    ".svg": "image/svg+xml",
}


def resolve_font_path(name: str) -> str | None:
    """Return a readable font path, or ``None`` when ``name`` is unsafe.

    Installed copies live in ``fonts/`` next to this renderer. The source
    tree falls back to the ``bob-obsidian-assets`` crate so the root package
    does not ship a second copy of the static faces.
    """
    if not name or name in (".", "..") or "/" in name or "\\" in name:
        return None
    for candidate in (
        os.path.join(FONTS_DIR, name),
        os.path.join(ASSETS_FONTS_DIR, name),
        os.path.join(PRINT_FONTS_DIR, name),
    ):
        if os.path.isfile(candidate):
            return candidate
    return None


_TTF_CACHE: dict[str, str] = {}
_TTF_DIR: str | None = None


def _ttf_cache_dir() -> str:
    global _TTF_DIR
    if _TTF_DIR is None:
        _TTF_DIR = tempfile.mkdtemp(prefix="bob-clip-ttf-")
        # Converted faces are per-process scratch; never leave them in /tmp.
        atexit.register(shutil.rmtree, _TTF_DIR, ignore_errors=True)
    return _TTF_DIR


def ensure_truetype(src: str) -> str:
    """Return a glyf TrueType path Chromium can embed as CID TrueType.

    Source Serif 4 Text and Inter are CFF OpenType. Skia embeds those as
    Type 3, which is what splits Highlights quote words in PDFKit.
    Convert cubic outlines to quadratic glyf once per process. Faces that
    are already TrueType (JetBrains Mono, DejaVu) are returned unchanged.
    """
    cached = _TTF_CACHE.get(src)
    if cached and os.path.isfile(cached):
        return cached
    try:
        served = _convert_cff_to_ttf(src)
    except Exception as exc:  # noqa: BLE001 - fall back to the source face
        print(f"web-clip font convert failed for {src}: {exc}",
              file=sys.stderr)
        served = src
    _TTF_CACHE[src] = served
    return served


def _convert_cff_to_ttf(src: str) -> str:
    from fontTools.pens.cu2quPen import Cu2QuPen
    from fontTools.pens.ttGlyphPen import TTGlyphPen
    from fontTools.ttLib import TTFont, newTable

    dest = src
    with TTFont(src) as font:
        if "CFF " not in font:
            return src
        glyph_order = font.getGlyphOrder()
        charstrings = font["CFF "].cff.topDictIndex[0].CharStrings
        glyf = newTable("glyf")
        glyf.glyphs = {}
        glyf.glyphOrder = glyph_order
        for name in glyph_order:
            tt_pen = TTGlyphPen(glyf.glyphs)
            cu_pen = Cu2QuPen(tt_pen, 1.0, reverse_direction=True)
            try:
                charstrings[name].draw(cu_pen)
            except Exception:  # noqa: BLE001 - empty glyf keeps the name
                pass
            glyf.glyphs[name] = tt_pen.glyph()
        del font["CFF "]
        font["glyf"] = glyf
        font["loca"] = newTable("loca")
        font.sfntVersion = "\x00\x01\x00\x00"
        maxp = newTable("maxp")
        maxp.tableVersion = 0x00010000
        maxp.numGlyphs = len(glyph_order)
        maxp.maxZones = 2
        maxp.maxTwilightPoints = 0
        maxp.maxStorage = 0
        maxp.maxFunctionDefs = 0
        maxp.maxInstructionDefs = 0
        maxp.maxStackElements = 0
        maxp.maxSizeOfInstructions = 0
        maxp.maxComponentElements = 0
        maxp.maxComponentDepth = 0
        font["maxp"] = maxp
        digest = hashlib.sha256(
            f"{src}:{os.path.getsize(src)}:{os.path.getmtime(src)}".encode()
        ).hexdigest()[:16]
        dest = os.path.join(_ttf_cache_dir(), digest + ".ttf")
        font.save(dest)
    return dest



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
_URL_HASH_RE = re.compile(r"""url\(\s*#([^)]+?)\s*\)""", re.IGNORECASE)

WORDS_PER_MINUTE = 230


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


def shift_headings(markup: str, target_level: int = 2) -> str:
    """Shift headings so the highest heading renders at ``target_level``."""

    levels = [int(match.group(2)) for match in _HEADING_RE.finditer(markup)
              if not match.group(1)]
    if not levels:
        return markup
    shift = target_level - min(levels)
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


def reading_minutes(word_count: int, *, wpm: int = WORDS_PER_MINUTE) -> int:
    """Reading time in minutes: ``ceil(words / wpm)``, minimum 1."""
    words = max(0, int(word_count or 0))
    if wpm <= 0:
        return 1
    return max(1, (words + wpm - 1) // wpm)


def namespace_ids(markup: str, prefix: str) -> str:
    """Prefix ``id`` values and same-document ``#`` / ``url(#)`` fragments."""
    def rewrite(name: str, value: str) -> str:
        if name == "id":
            return value if value.startswith(prefix) else prefix + value
        if name in ("href", "xlink:href") and value.startswith("#"):
            fragment = value[1:]
            return value if fragment.startswith(prefix) else f"#{prefix}{fragment}"

        def prefix_url(match: re.Match) -> str:
            fragment = match.group(1).strip()
            if fragment.startswith(prefix):
                return match.group(0)
            return f"url(#{prefix}{fragment})"

        return _URL_HASH_RE.sub(prefix_url, value)

    return _rewrite_start_tag_attributes(markup, rewrite)


def resolve_relative_hrefs(markup: str, base_url: str) -> str:
    """Resolve relative ``href`` / ``xlink:href`` against ``base_url``."""
    if not base_url:
        return markup

    def rewrite(name: str, value: str) -> str:
        if name not in ("href", "xlink:href"):
            return value
        href = html.unescape(value).strip()
        if not href or href.startswith(("#", "mailto:", "data:",
                                        "javascript:")):
            return value
        scheme = urlsplit(href).scheme.lower()
        if scheme in ("http", "https"):
            return value
        resolved = urljoin(base_url, href)
        return html.escape(resolved, quote=True)

    return _rewrite_start_tag_attributes(markup, rewrite)


def _rewrite_start_tag_attributes(markup: str, rewrite) -> str:
    """Rewrite real start-tag attributes while preserving every text node.

    The scanner tracks quoted values when finding a tag's closing ``>``. It
    then walks only that tag's attributes, so markup-like prose and code are
    left byte-for-byte intact and a ``>`` inside a quoted value is harmless.
    """
    output: list[str] = []
    cursor = 0
    length = len(markup)
    while cursor < length:
        start = markup.find("<", cursor)
        if start < 0:
            output.append(markup[cursor:])
            break
        output.append(markup[cursor:start])
        if markup.startswith("<!--", start):
            end = markup.find("-->", start + 4)
            if end < 0:
                output.append(markup[start:])
                break
            end += 3
            output.append(markup[start:end])
            cursor = end
            continue
        if (markup.startswith("</", start)
                or markup.startswith("<!", start)
                or markup.startswith("<?", start)
                or start + 1 >= length
                or not markup[start + 1].isalpha()):
            end = _markup_token_end(markup, start)
            if end is None:
                output.append(markup[start:])
                break
            output.append(markup[start:end])
            cursor = end
            continue
        end = _markup_token_end(markup, start)
        if end is None:
            output.append(markup[start:])
            break
        output.append(_rewrite_start_tag(markup[start:end], rewrite))
        cursor = end
    return "".join(output)


def _markup_token_end(markup: str, start: int) -> int | None:
    """Return the index after a tag-like token, respecting quoted values."""
    quote: str | None = None
    index = start + 1
    while index < len(markup):
        char = markup[index]
        if quote:
            if char == quote:
                quote = None
        elif char in ("'", '"'):
            quote = char
        elif char == ">":
            return index + 1
        index += 1
    return None


def _rewrite_start_tag(tag: str, rewrite) -> str:
    """Rewrite attribute values in one already-tokenized start tag."""
    index = 1
    length = len(tag)
    while index < length and not tag[index].isspace() and tag[index] not in "/>":
        index += 1
    pieces: list[str] = []
    copied_to = 0
    while index < length:
        while index < length and tag[index].isspace():
            index += 1
        if index >= length or tag[index] in "/>" or tag[index] == ">":
            break
        attr_start = index
        while (index < length and not tag[index].isspace()
               and tag[index] not in "=/>\""):
            index += 1
        if index == attr_start:
            index += 1
            continue
        name = tag[attr_start:index].lower()
        while index < length and tag[index].isspace():
            index += 1
        if index >= length or tag[index] != "=":
            continue
        index += 1
        while index < length and tag[index].isspace():
            index += 1
        if index >= length:
            break
        quote = tag[index] if tag[index] in ("'", '"') else None
        if quote:
            value_start = index + 1
            value_end = tag.find(quote, value_start)
            if value_end < 0:
                break
            value = tag[value_start:value_end]
            next_index = value_end + 1
        else:
            value_start = index
            while index < length and not tag[index].isspace() and tag[index] != ">":
                index += 1
            value_end = index
            value = tag[value_start:value_end]
            next_index = value_end
        rewritten = rewrite(name, value)
        if rewritten != value:
            pieces.extend((tag[copied_to:value_start], rewritten))
            copied_to = value_end
        index = next_index
    if not pieces:
        return tag
    pieces.append(tag[copied_to:])
    return "".join(pieces)


def prefix_part_assets(markup: str, assets_dir: str, prefix: str,
                       dest_dir: str) -> str:
    """Copy part assets under ``prefix`` and rewrite ``src="assets/…"``."""
    os.makedirs(dest_dir, exist_ok=True)

    def _replace(match: re.Match) -> str:
        tag = match.group(0)
        src_match = _SRC_ATTR_RE.search(tag)
        if not src_match:
            return tag
        raw_src = html.unescape(src_match.group(1).strip("\"'"))
        if (not raw_src.startswith("assets/")
                or "/" in raw_src[len("assets/"):]):
            return tag
        name = raw_src[len("assets/"):]
        new_name = f"{prefix}{name}"
        src_path = os.path.join(assets_dir, name)
        if os.path.isfile(src_path):
            shutil.copy2(src_path, os.path.join(dest_dir, new_name))
        return _SRC_ATTR_RE.sub(f'src="assets/{new_name}"', tag, count=1)

    return _IMG_TAG_RE.sub(_replace, markup)


def _masthead_fields(article: dict, body: str | None = None) -> dict:
    """Return the shared title, dek, byline, and provenance fields."""
    title = article.get("title") or "Untitled"
    author = article.get("author")
    published = format_long_date(article.get("published"))
    captured = format_long_date(article.get("captured"))
    site = article.get("site")
    description = article.get("description")
    source_url = article.get("source_url") or ""
    word_count = article.get("word_count") or 0
    if body is None:
        body = shift_headings(article.get("html") or "")

    byline = " · ".join(part for part in (author, published) if part)
    provenance = [f'<a href="{html.escape(source_url)}">'
                  f"{html.escape(source_url)}</a>"]
    if captured:
        provenance.append(f"Captured {html.escape(captured)}")
    if word_count:
        provenance.append(f"{word_count} words")
    dek = show_dek(description, body)
    return {
        "title": title,
        "site": site,
        "dek": dek,
        "byline": byline,
        "provenance": provenance,
    }


def assemble_masthead(article: dict, body: str | None = None) -> str:
    """Build the ``<header class="masthead">`` for one article."""
    fields = _masthead_fields(article, body)

    masthead = ['<header class="masthead">']
    if fields["site"]:
        masthead.append(
            f'<p class="masthead-site">{html.escape(fields["site"])}</p>')
    masthead.append(
        f'<h1 class="masthead-title">{html.escape(fields["title"])}</h1>')
    if fields["dek"]:
        masthead.append(
            f'<p class="masthead-dek">{html.escape(fields["dek"])}</p>')
    if fields["byline"]:
        masthead.append(
            f'<p class="masthead-byline">{html.escape(fields["byline"])}</p>')
    masthead.append(
        '<p class="masthead-provenance">'
        + " · ".join(f"<span>{part}</span>"
                     for part in fields["provenance"])
        + "</p>")
    masthead.append("</header>")
    return "".join(masthead)


_IMG_ATTR_RE = re.compile(
    r'''\s([a-zA-Z_:][\w:.-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))''')


def _img_attr(tag: str, name: str) -> str | None:
    """Read one serialized image attribute, case-insensitively."""
    for match in _IMG_ATTR_RE.finditer(tag):
        if match.group(1).lower() == name:
            return html.unescape(next(
                value for value in match.groups()[1:] if value is not None))
    return None


def source_images(markup: str) -> str:
    """Keep only original remote image URL, alt, and title in source HTML."""
    def _replace(match: re.Match) -> str:
        tag = match.group(0)
        origin = _img_attr(tag, "data-bob-origin")
        if not origin or not origin.startswith(("http://", "https://")):
            return ""
        attrs = [f'src="{html.escape(origin, quote=True)}"']
        for name in ("alt", "title"):
            value = _img_attr(tag, name)
            if value is not None:
                attrs.append(f'{name}="{html.escape(value, quote=True)}"')
        return "<img " + " ".join(attrs) + ">"

    return _IMG_TAG_RE.sub(_replace, markup)


def _source_document(title: str, body: str) -> str:
    """Wrap semantic source content without print CSS or Bob-only markup."""
    return ("<!doctype html>\n<html lang=\"en\">\n<head>"
            "<meta charset=\"utf-8\">"
            f"<title>{html.escape(title)}</title></head>\n<body>\n"
            f"{body}\n</body>\n</html>\n")


def assemble_source(article: dict) -> str:
    """Build the semantic HTML source view for a single printed article."""
    source_url = article.get("source_url") or ""
    body = resolve_relative_hrefs(article.get("html") or "", source_url)
    body = source_images(shift_headings(body))
    fields = _masthead_fields(article, body)
    masthead = [f'<h1>{html.escape(fields["title"])}</h1>']
    if fields["dek"]:
        masthead.append(f'<p>{html.escape(fields["dek"])}</p>')
    if fields["byline"]:
        masthead.append(f'<p>{html.escape(fields["byline"])}</p>')
    masthead.append('<p>' + " · ".join(fields["provenance"]) + "</p>")
    return _source_document(fields["title"], "\n".join(masthead + [body]))


def assemble_collection_source(parts: list[dict], collection: dict) -> str:
    """Build collection source HTML with one semantic heading per part."""
    title = collection.get("title") or "Untitled"
    total_words = sum(int(part.get("word_count") or 0) for part in parts)
    captured = format_long_date(collection.get("captured"))
    collection_byline = collection.get("author")
    opening = [f"<h1>{html.escape(title)}</h1>"]
    if collection_byline:
        opening.append(f"<p>{html.escape(str(collection_byline))}</p>")
    meta = [f"{len(parts)} articles", f"{total_words:,} words"]
    if captured:
        meta.append(f"Captured {html.escape(captured)}")
    opening.append("<p>" + " · ".join(meta) + "</p>")

    sections: list[str] = []
    for part in parts:
        source_url = part.get("source_url") or ""
        body = resolve_relative_hrefs(part.get("html") or "", source_url)
        body = source_images(shift_headings(body, target_level=3))
        fields = _masthead_fields(part, body)
        sections.append(f'<h2>{html.escape(fields["title"])}</h2>')
        if fields["byline"]:
            sections.append(f'<p>{html.escape(fields["byline"])}</p>')
        sections.append("<p>" + " · ".join(fields["provenance"]) + "</p>")
        sections.append(body)
    return _source_document(title, "\n".join(opening + sections))


def assemble(article: dict) -> str:
    """Build the full reader page from an ``article.json`` dict."""
    title = article.get("title") or "Untitled"
    body = shift_headings(article.get("html") or "")
    masthead = assemble_masthead(article, body)

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
{masthead}
<div class="article-body">
{body}
</div>
</body>
</html>
"""


def assemble_collection(parts: list[dict], collection: dict) -> str:
    """Build one reader page from ordered part dicts and collection meta."""
    title = collection.get("title") or "Untitled"
    author = collection.get("author")
    captured = format_long_date(collection.get("captured"))
    n_parts = len(parts)
    total_words = sum(int(part.get("word_count") or 0) for part in parts)
    sites = [part.get("site") for part in parts]
    shared_site = (
        sites[0]
        if sites and all(site and site == sites[0] for site in sites)
        else None
    )

    opening = ['<div class="collection-opening">']
    if shared_site:
        opening.append(
            f'<p class="masthead-site">{html.escape(str(shared_site))}</p>')
    opening.append(
        f'<p class="masthead-title">{html.escape(title)}</p>')
    if author:
        opening.append(
            f'<p class="masthead-byline">{html.escape(str(author))}</p>')
    meta_bits = [
        f"{n_parts} article{'s' if n_parts != 1 else ''}",
        f"{total_words:,} words",
        f"~{reading_minutes(total_words)} min read",
    ]
    if captured:
        meta_bits.append(f"captured {captured}")
    opening.append(
        f'<p class="collection-meta">{html.escape(" · ".join(meta_bits))}</p>')
    opening.append('<p class="contents-label">Contents</p>')
    opening.append('<ol class="contents">')
    for index, part in enumerate(parts, 1):
        part_title = part.get("title") or "Untitled"
        opening.append("<li>")
        opening.append(
            f'<a href="#part-{index}">{html.escape(part_title)}</a>')
        line_bits: list[str] = []
        if part.get("site"):
            line_bits.append(str(part["site"]))
        if part.get("author"):
            line_bits.append(str(part["author"]))
        part_date = format_long_date(part.get("published"))
        if part_date:
            line_bits.append(part_date)
        line_bits.append(
            f"~{reading_minutes(int(part.get('word_count') or 0))} min")
        opening.append(
            f'<p class="contents-meta">{html.escape(" · ".join(line_bits))}</p>')
        opening.append("</li>")
    opening.append("</ol></div>")

    sections: list[str] = []
    for index, part in enumerate(parts, 1):
        body = shift_headings(part.get("html") or "")
        masthead = assemble_masthead(part, body)
        kicker = f"Part {index} of {n_parts}"
        sections.append(
            f'<section class="part" id="part-{index}">'
            f'<p class="part-kicker">{html.escape(kicker)}</p>'
            f"{masthead}"
            f'<div class="article-body">\n{body}\n</div>'
            "</section>"
        )

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
{"".join(opening)}
{"".join(sections)}
</body>
</html>
"""


def prepare_part(article: dict, assets_dir: str, index: int,
                 combined_assets_dir: str, render_assets_dir: str,
                 warnings: list[str]) -> tuple[dict, dict]:
    """Namespace ids, prefix assets, and normalize images for one part."""
    prefix = f"p{index}-"
    markup = article.get("html") or ""
    markup = resolve_relative_hrefs(markup, article.get("source_url") or "")
    markup = namespace_ids(markup, prefix)
    markup = prefix_part_assets(
        markup, assets_dir, prefix, combined_assets_dir)
    markup, images = normalize_images(
        markup, combined_assets_dir, render_assets_dir, warnings)
    return dict(article, html=markup), images


def build_collection_html(part_json_paths: list[str], collection: dict,
                          workdir: str) -> tuple[str, dict, list[str]]:
    """Assemble collection HTML without printing. Returns html, images, warnings."""
    import json

    if not part_json_paths:
        raise RenderError("compose needs at least one part")
    combined_assets = os.path.join(workdir, "assets")
    render_dir = os.path.join(workdir, "render")
    render_assets = os.path.join(render_dir, "assets")
    os.makedirs(combined_assets, exist_ok=True)
    os.makedirs(render_assets, exist_ok=True)

    warnings: list[str] = []
    images = {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0}
    loaded: list[dict] = []
    for index, path in enumerate(part_json_paths, 1):
        with open(path, encoding="utf-8") as handle:
            article = json.load(handle)
        if not isinstance(article, dict):
            raise RenderError(f"part {index} article.json is not an object")
        part_assets = os.path.join(os.path.dirname(os.path.abspath(path)),
                                   "assets")
        article, part_images = prepare_part(
            article, part_assets, index, combined_assets, render_assets,
            warnings)
        for key in images:
            images[key] += int(part_images.get(key, 0) or 0)
        loaded.append(article)

    page = assemble_collection(loaded, collection)
    page_path = os.path.join(render_dir, "article.html")
    with open(page_path, "w", encoding="utf-8") as handle:
        handle.write(page)
    source_path = os.path.join(render_dir, "source.html")
    with open(source_path, "w", encoding="utf-8") as handle:
        handle.write(assemble_collection_source(loaded, collection))
    return page, images, warnings


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
            base = resolve_font_path(name)
            if base is None:
                route.abort()
                return
            base = ensure_truetype(base)
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


def print_pdf(render_dir: str, out_pdf: str, launcher) -> list[dict]:
    """Print ``render/article.html`` and record code blocks at render time."""
    try:
        with open(READER_CSS_PATH, encoding="utf-8") as handle:
            handle.read()
    except OSError as exc:
        raise RenderError(f"reader template is missing: {exc}") from exc
    try:
        browser = launcher(headless=True)
    except Exception as exc:
        raise BrowserLaunchError(f"browser launch failed: {exc}") from exc
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
                code_blocks = page.evaluate("""() => {
                    const blocks = Array.from(
                        document.querySelectorAll('.article-body pre'));
                    const recorded = blocks.map((pre, index) => {
                        const k = index + 1;
                        const text = pre.textContent || '';
                        const start = document.createElement('span');
                        start.id = `bob-code-${k}`;
                        start.className = 'bob-code-anchor';
                        start.setAttribute('aria-hidden', 'true');
                        pre.insertBefore(start, pre.firstChild);
                        const end = document.createElement('span');
                        end.id = `bob-code-${k}-end`;
                        end.className = 'bob-code-anchor';
                        end.setAttribute('aria-hidden', 'true');
                        pre.appendChild(end);
                        return {k, text};
                    });
                    const style = document.createElement('style');
                    style.textContent = '.bob-code-anchor {' +
                        'display:inline-block;width:1px;height:1px;' +
                        'overflow:hidden;font-size:0;line-height:0;}';
                    document.head.appendChild(style);
                    const nav = document.createElement('nav');
                    nav.hidden = true;
                    nav.setAttribute('aria-hidden', 'true');
                    for (const block of recorded) {
                        for (const suffix of ['', '-end']) {
                            const link = document.createElement('a');
                            link.href = `#bob-code-${block.k}${suffix}`;
                            link.textContent = 'anchor';
                            nav.appendChild(link);
                        }
                    }
                    document.body.appendChild(nav);
                    return recorded;
                }""")
                if not isinstance(code_blocks, list):
                    raise RenderError("browser returned invalid code anchors")
                anchors_path = os.path.abspath(os.path.join(
                    render_dir, "code-anchors.json"))
                with open(anchors_path, "w", encoding="utf-8") as handle:
                    json.dump({"version": 1, "blocks": code_blocks}, handle,
                              ensure_ascii=False)
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
    return code_blocks


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
    source_path = os.path.abspath(os.path.join(render_dir, "source.html"))
    with open(source_path, "w", encoding="utf-8") as handle:
        handle.write(assemble_source(article))

    code_blocks = print_pdf(render_dir, out_pdf, launcher)

    pdf_bytes = os.path.getsize(out_pdf)
    if pdf_bytes > PDF_WARN_BYTES:
        warnings.append(
            f"printed PDF is {pdf_bytes / 1048576:.1f} MiB "
            "(vault sync refuses 95 MiB or more)")
    return {
        "pdf_bytes": pdf_bytes,
        "images": images,
        "warnings": warnings,
        "source_html": source_path,
        "code_anchors": os.path.abspath(os.path.join(
            render_dir, "code-anchors.json")),
        "code_blocks": code_blocks,
    }


def render_collection(parts: list[str], collection: dict, workdir: str,
                      out_pdf: str, launcher) -> dict:
    """Normalize, assemble, and print a multi-part collection PDF."""
    _page, images, warnings = build_collection_html(
        parts, collection, workdir)
    render_dir = os.path.join(workdir, "render")
    code_blocks = print_pdf(render_dir, out_pdf, launcher)

    pdf_bytes = os.path.getsize(out_pdf)
    if pdf_bytes > PDF_WARN_BYTES:
        warnings.append(
            f"printed PDF is {pdf_bytes / 1048576:.1f} MiB "
            "(vault sync refuses 95 MiB or more)")
    return {
        "pdf_bytes": pdf_bytes,
        "images": images,
        "warnings": warnings,
        "source_html": os.path.abspath(os.path.join(render_dir, "source.html")),
        "code_anchors": os.path.abspath(os.path.join(
            render_dir, "code-anchors.json")),
        "code_blocks": code_blocks,
    }

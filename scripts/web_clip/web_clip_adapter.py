#!/usr/bin/env python3
"""Pinned web-clip adapter for `bob highlights clip` (adapter protocol v1).

The Rust side writes one compact JSON request to this script's stdin and
closes it. This script writes exactly one JSON response to stdout and logs
to stderr. It exits 0 whenever it wrote a response, whether ``ok`` is true
or false. Every request carries ``"protocol": 1``.

Ops:

- ``ping``: no auth, no network. Reports the interpreter, pinned
  dependency versions, the discovered browser, headed capability, and
  bundled fonts.
- ``capture``: launch Chrome (headless, with a headed Xvfb fallback on bot
  challenges), snapshot a stable DOM, run vendored Defuddle in isolation,
  layer metadata, check fidelity, sanitize with ``nh3``, localize images,
  and print a reader PDF via the placeholder renderer.

Siblings loaded at runtime (same directory, on ``sys.path``):

- ``snapshot.js``: in-page probe and snapshot code (``window.__bobClip``).
- ``web_clip_render.py``: placeholder renderer; ``reader-template`` swaps
  in the real one under the same ``render()`` signature.
- ``vendor/defuddle.full.js``: pinned Defuddle UMD bundle.

Run ``python3 -m py_compile`` plus ``--self-test`` via
``just check-web-clip-adapter``.
"""

# /// script
# requires-python = ">=3.10"
# dependencies = ["playwright==1.62.0", "pillow==12.3.0", "nh3==0.3.7"]
# [tool.uv]
# exclude-newer = "2026-09-01T00:00:00Z"
# ///

from __future__ import annotations

import base64
import datetime
import html as html_module
import ipaddress
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
import traceback
import unicodedata
from urllib.parse import urlsplit

PROTOCOL_VERSION = 1
DEFUDDLE_VERSION = "0.19.4"

WEB_CLIP_DIR = os.path.dirname(os.path.abspath(__file__))
SNAPSHOT_JS_PATH = os.path.join(WEB_CLIP_DIR, "snapshot.js")
DEFUDDLE_JS_PATH = os.path.join(WEB_CLIP_DIR, "vendor", "defuddle.full.js")

OVERALL_TIMEOUT_SECS = 150
GOTO_TIMEOUT_MS = 45000
POLL_MAX_SECS = 20
POLL_INTERVAL_SECS = 0.25
STABLE_SECS = 1.0
LAZY_MAX_SECS = 12
HEADED_WAIT_SECS = 30
REQUEST_TIMEOUT_MS = 15000
IMAGE_MAX_BYTES = 15 * 1024 * 1024
IMAGES_TOTAL_MAX_BYTES = 60 * 1024 * 1024
PDF_MAX_BYTES = 95 * 1024 * 1024
PDF_WARN_BYTES = 50 * 1024 * 1024
FIXTURE_HOST = "fixture.bob-clip.invalid"

BROWSER_CANDIDATE_PATHS = (
    "/opt/google/chrome/chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
)

CHROME_ARGS = ["--disable-blink-features=AutomationControlled"]
# The single automation-related switch exists only because openai.com tears
# down its own DOM when navigator.webdriver is true (measured 2026-10-01).

BLOCKED_NETWORKS = [
    ipaddress.ip_network(cidr)
    for cidr in (
        "127.0.0.0/8",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "169.254.0.0/16",
        "0.0.0.0/8",
        "100.64.0.0/10",
        "192.0.0.0/24",
        "192.0.2.0/24",
        "198.51.100.0/24",
        "203.0.113.0/24",
        "224.0.0.0/4",
        "240.0.0.0/4",
        "255.255.255.255/32",
        "::1/128",
        "::/128",
        "fe80::/10",
        "fc00::/7",
        "ff00::/8",
        "2001:db8::/32",
    )
]


class AdapterFail(Exception):
    """A controlled adapter failure: print an ``ok:false`` response."""

    def __init__(self, kind: str, message: str, hint: str | None = None) -> None:
        super().__init__(message)
        self.kind = kind
        self.message = message
        self.hint = hint


def error_response(
    op: str, kind: str, message: str, hint: str | None = None
) -> dict:
    """Shape an ``ok:false`` protocol response."""
    error: dict = {"kind": kind, "message": message}
    if hint is not None:
        error["hint"] = hint
    return {"protocol": PROTOCOL_VERSION, "ok": False, "op": op, "error": error}


def log(message: str) -> None:
    """Write a debug line to stderr (stdout carries only the response)."""
    sys.stderr.write(f"web-clip: {message}\n")


BLOCKED_HINT = (
    "this site blocks headless browsers; rerun on a host that can run "
    "headed Chrome (Linux with Xvfb such as athena, or the Mac), or save "
    "the page from your browser and pass --html FILE"
)
CHALLENGE_STUCK_HINT = (
    "the bot challenge did not clear; save the page from your browser "
    "and pass --html FILE"
)
LOGIN_HINT = (
    "the page needs a login this tool will not perform; save the page "
    "from your browser (logged in) and pass --html FILE"
)
BROWSER_HINT = "install Google Chrome or set BOB_CHROME=/path/to/chrome"
THIN_HINT = "retry with --html from a browser-saved page, or accept that the page is not an article"


def _package_version(name: str) -> str:
    """Best-effort version lookup: ``"unknown"`` when it fails."""
    try:
        from importlib import metadata

        return metadata.version(name)
    except Exception:  # noqa: BLE001 - version lookup is best effort
        return "unknown"


# ---------------------------------------------------------------------------
# Date parsing and metadata layering (pure; covered by the self-test).
# ---------------------------------------------------------------------------

_DATE_FORMATS = (
    "%B %d, %Y",
    "%b %d, %Y",
    "%d %B %Y",
    "%d %b %Y",
    "%B %d %Y",
    "%Y-%m-%d",
)


def parse_published(raw: object) -> str | None:
    """Normalize a raw date to ``YYYY-MM-DD``, or ``None`` when unknown."""
    if not isinstance(raw, str):
        return None
    text = raw.strip()
    if not text:
        return None
    iso = re.match(r"^(\d{4})-(\d{2})-(\d{2})", text)
    if iso:
        try:
            datetime.date(int(iso.group(1)), int(iso.group(2)), int(iso.group(3)))
        except ValueError:
            return None
        return f"{iso.group(1)}-{iso.group(2)}-{iso.group(3)}"
    text = re.sub(r"\s+", " ", text)
    text = text.replace("Sept.", "Sep").replace("Sept ", "Sep ")
    for fmt in _DATE_FORMATS:
        try:
            parsed = datetime.datetime.strptime(text, fmt)
        except ValueError:
            continue
        return parsed.strftime("%Y-%m-%d")
    return None


def _norm_key(text: str) -> str:
    """Case-folded, punctuation-stripped comparison key for fuzzy matching."""
    folded = text.casefold()
    stripped = "".join(
        ch for ch in folded if not unicodedata.category(ch).startswith("P")
    )
    return re.sub(r"\s+", " ", stripped).strip()


def strip_site_suffix(title: str) -> str:
    """Remove one trailing ``| Site`` or ``- Site`` suffix, if present."""
    cleaned = re.sub(r"\s+[|\-–—]\s*[^|\-–—]+?\s*$", "", title).strip()
    return cleaned or title


def layer_title(
    h1: str | None,
    og_title: str | None,
    defuddle_title: str | None,
    url_slug: str | None,
) -> tuple[str | None, str | None]:
    """Pick the title; overrides are applied by the caller (source override)."""
    base = og_title or defuddle_title or None
    base_source = "og:title" if og_title else ("defuddle" if defuddle_title else None)
    if h1:
        if base and (
            _norm_key(h1) in _norm_key(base) or _norm_key(base) in _norm_key(h1)
        ):
            return h1, "h1"
        if not base:
            return h1, "h1"
    if base and base_source:
        return strip_site_suffix(base), base_source
    if url_slug:
        return re.sub(r"_+", " ", url_slug).strip().capitalize() or None, "url-slug"
    return None, None


def _handle_key(name: str) -> str:
    return name.strip().casefold().lstrip("@").strip()


def layer_author(
    jsonld_author: str | None,
    meta_author: str | None,
    byline: str | None,
    defuddle_author: str | None,
    embed_authors: list[str],
) -> tuple[str | None, str | None]:
    """Pick the author, discarding candidates owned by embedded posts."""
    blocked = {_handle_key(name) for name in embed_authors if name}

    def usable(candidate: str | None) -> str | None:
        if not candidate:
            return None
        text = re.sub(r"^\s*By\s+", "", candidate.strip()).strip()
        if not text or _handle_key(text) in blocked:
            return None
        return text

    for candidate, source in (
        (jsonld_author, "json-ld"),
        (meta_author, "meta"),
        (byline, "byline"),
        (defuddle_author, "defuddle"),
    ):
        picked = usable(candidate)
        if picked:
            return picked, source
    return None, None


def layer_published(
    jsonld_date: str | None,
    article_time: str | None,
    visible_dates: list[str],
    defuddle_published: str | None,
    embed_dates: list[str],
) -> tuple[str | None, str | None]:
    """Pick the publish date; Defuddle loses to dates found inside embeds."""
    for raw, source in (
        [(jsonld_date, "json-ld"), (article_time, "article:published_time")]
        + [(raw, "visible-date") for raw in visible_dates]
    ):
        parsed = parse_published(raw)
        if parsed:
            return parsed, source
    if defuddle_published:
        candidate = parse_published(defuddle_published)
        if candidate:
            for raw in embed_dates:
                if parse_published(raw) == candidate:
                    return None, None
            return candidate, "defuddle"
    return None, None


def layer_site(
    og_site: str | None, defuddle_site: str | None, host: str
) -> tuple[str, str]:
    """Pick the site name, falling back to the URL host."""
    if og_site:
        return og_site, "og:site_name"
    if defuddle_site:
        return defuddle_site, "defuddle"
    return host, "host"


def url_slug(url: str) -> str:
    """Last resort title words: snake_case of the last URL path segment."""
    try:
        path = urlsplit(url).path
    except ValueError:
        return ""
    parts = [part for part in path.split("/") if part]
    slug = parts[-1] if parts else ""
    slug = re.sub(r"\.(html?|php|aspx?|shtml)$", "", slug, flags=re.IGNORECASE)
    slug = re.sub(r"[^0-9A-Za-z]+", "_", slug).strip("_").lower()
    return slug[:80]


# ---------------------------------------------------------------------------
# Text cleanup and sanitizing (pure; covered by the self-test).
# ---------------------------------------------------------------------------

_ZERO_WIDTH_RE = re.compile("[\u200b-\u200d\u2060\ufeff\u00ad]")
_OPEN_SUFFIX_RE = re.compile(r" \((opens in a new (?:window|tab))\)", re.IGNORECASE)

_TEXT_TAGS = {
    "p", "h1", "h2", "h3", "h4", "h5", "h6",
    "ul", "ol", "li", "dl", "dt", "dd",
    "table", "thead", "tbody", "tfoot", "tr", "th", "td",
    "caption", "colgroup", "col",
    "pre", "code", "blockquote", "q",
    "figure", "figcaption",
    "details", "summary", "time", "abbr", "mark",
    "sub", "sup", "br", "hr",
    "img", "a",
    "div", "span", "section",
    "kbd", "samp", "var", "cite", "dfn", "small",
    "strong", "em", "b", "i", "u", "s", "del", "ins", "wbr",
}
_MATHML_TAGS = {
    "math", "mi", "mn", "mo", "mrow", "msup", "msub", "msubsup",
    "mfrac", "msqrt", "mroot", "mtable", "mtr", "mtd", "mtext",
    "mspace", "menclose", "maction", "semantics", "annotation",
    "annotation-xml",
}
_SVG_TAGS = {
    "svg", "g", "defs", "use", "symbol", "title", "desc",
    "circle", "ellipse", "line", "path", "polygon", "polyline",
    "rect", "text", "tspan", "stop", "linearGradient",
    "radialGradient", "clipPath", "mask", "pattern", "marker",
}
_SVG_ATTRS = {
    "viewBox", "width", "height", "fill", "stroke", "stroke-width",
    "cx", "cy", "r", "x", "y", "x1", "y1", "x2", "y2", "d",
    "points", "transform", "opacity", "fill-opacity",
    "stroke-opacity", "font-size", "text-anchor", "offset",
    "stop-color", "stroke-linecap", "stroke-linejoin",
}


def sanitize_html(dirty: str) -> str:
    """Filter Defuddle HTML through the ``nh3`` allowlist."""
    import nh3

    tags = _TEXT_TAGS | _MATHML_TAGS | _SVG_TAGS
    attributes: dict[str, set[str]] = {
        "a": {"href", "title"},
        "img": {"src", "alt", "width", "height"},
        "th": {"colspan", "rowspan", "scope"},
        "td": {"colspan", "rowspan"},
        "time": {"datetime"},
        "abbr": {"title"},
        "ol": {"start", "type"},
        "li": {"value"},
        "*": {"id"},
    }
    for tag in _SVG_TAGS:
        attributes[tag] = set(_SVG_ATTRS)
    return nh3.clean(
        dirty,
        tags=tags,
        attributes=attributes,
        url_schemes={"http", "https", "mailto"},
        link_rel=None,
    )


def strip_tags(markup: str) -> str:
    """Best-effort visible text of an HTML fragment for word counts."""
    text = re.sub(r"<[^>]*>", " ", markup)
    return html_module.unescape(text)


def html_word_count(markup: str) -> int:
    """Count whitespace-separated words in an HTML fragment."""
    return len([word for word in strip_tags(markup).split() if word])


def count_tag(markup: str, tag: str) -> int:
    """Count opening tags (case-insensitive) in an HTML fragment."""
    return len(re.findall(rf"<{tag}\b", markup, flags=re.IGNORECASE))


def clean_text_html(content: str, title: str | None) -> tuple[str, bool]:
    """Drop a title-repeating lead heading and invisible characters."""
    dropped = False
    if title:
        lead = re.match(
            r"\s*<(h[1-6])[^>]*>(.*?)</\1>\s*", content, flags=re.DOTALL
        )
        if lead and _norm_key(strip_tags(lead.group(2))) == _norm_key(title):
            content = content[lead.end():]
            dropped = True
    content = _ZERO_WIDTH_RE.sub("", content)
    content = _OPEN_SUFFIX_RE.sub("", content)
    return content, dropped


def fidelity_verdict(
    page_counts: dict, kept_counts: dict
) -> tuple[str, list[str], AdapterFail | None]:
    """Grade the extraction against the live-page counts."""
    warnings: list[str] = []
    page_words = page_counts.get("words", 0)
    kept_words = kept_counts.get("words", 0)
    if kept_words < 300 and page_words >= 400:
        return (
            "thin",
            warnings,
            AdapterFail(
                "thin",
                f"extraction kept only {kept_words} words from a "
                f"{page_words}-word page",
                THIN_HINT,
            ),
        )
    page_media = page_counts.get("large_media", 0)
    kept_media = kept_counts.get("large_media", 0)
    if kept_media < page_media:
        warnings.append(
            f"page showed {page_media} large images below the title; "
            f"extraction kept {kept_media}"
        )
    page_code = page_counts.get("code_blocks", 0)
    kept_code = kept_counts.get("code_blocks", 0)
    if kept_code < page_code:
        warnings.append(
            f"page showed {page_code} code blocks; extraction kept {kept_code}"
        )
    if page_words > 0 and kept_words < 0.4 * page_words:
        warnings.append(
            f"extraction kept {kept_words} of {page_words} page words "
            "(under 40%)"
        )
    return ("warn" if warnings else "ok", warnings, None)


def classify_probe(probe: dict, cf_mitigated: bool) -> tuple[str, str]:
    """Classify a snapshot.js probe dict: challenged, login, ready, loading."""
    if probe.get("challengeDom") or cf_mitigated:
        return ("challenged", "bot-challenge markers present")
    title = probe.get("title") or ""
    if not title.strip():
        return ("loading", "empty title")
    phrases = probe.get("loginPhrases") or []
    words = probe.get("visibleWords") or 0
    if phrases and words < 300:
        return ("login", f"login-wall phrases: {', '.join(phrases)}")
    return ("ready", "page looks navigable")


# ---------------------------------------------------------------------------
# Private-network guard and browser discovery (ping uses these too).
# ---------------------------------------------------------------------------

_host_global_cache: dict[str, bool] = {}


def host_is_global(host: str) -> bool:
    """Whether every resolved address of ``host`` is a global unicast one."""
    key = host.lower()
    if key in _host_global_cache:
        return _host_global_cache[key]
    try:
        infos = socket.getaddrinfo(host, None, type=socket.SOCK_STREAM)
    except socket.gaierror:
        _host_global_cache[key] = False
        return False
    addresses = {info[4][0] for info in infos}
    if not addresses:
        _host_global_cache[key] = False
        return False
    result = True
    for raw in addresses:
        try:
            addr = ipaddress.ip_address(raw)
        except ValueError:
            result = False
            break
        if any(addr in net for net in BLOCKED_NETWORKS):
            result = False
            break
    _host_global_cache[key] = result
    return result


def _chrome_version(executable: str) -> str:
    """Best-effort ``<major>.<minor>.<build>.<patch>`` of a Chrome binary."""
    try:
        proc = subprocess.run(
            [executable, "--version"],
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    match = re.search(r"(\d+\.\d+\.\d+\.\d+)", proc.stdout or "")
    return match.group(1) if match else "unknown"


def discover_browser() -> dict | None:
    """Find a usable browser; ``None`` when there is nothing to launch."""
    override = os.environ.get("BOB_CHROME")
    if override:
        if os.path.isfile(override) and os.access(override, os.X_OK):
            return {
                "kind": "chrome",
                "path": override,
                "version": _chrome_version(override),
                "launch": {"executable_path": override},
            }
        log(f"BOB_CHROME={override} is not an executable file")
    for path in BROWSER_CANDIDATE_PATHS:
        if os.path.isfile(path) and os.access(path, os.X_OK):
            return {
                "kind": "chrome",
                "path": path,
                "version": _chrome_version(path),
                "launch": {"channel": "chrome"},
            }
    for bundled in _bundled_chromium_candidates():
        return {
            "kind": "chromium",
            "path": bundled,
            "version": _chrome_version(bundled),
            "launch": {},
        }
    return None


def _bundled_chromium_candidates() -> list[str]:
    """Installed Playwright bundled-Chromium executables, newest first.

    Read from the browser registry on disk so discovery never starts a
    driver (starting one prints shutdown noise and needs a display).
    """
    import glob

    roots = []
    override = os.environ.get("PLAYWRIGHT_BROWSERS_PATH")
    if override:
        roots.append(override)
    else:
        roots.append(os.path.join(os.path.expanduser("~"), ".cache",
                                  "ms-playwright"))
    found = []
    for root in roots:
        if sys.platform == "darwin":
            pattern = os.path.join(root, "chromium-*",
                                   "chrome-mac", "Chromium.app", "Contents",
                                   "MacOS", "Chromium")
        else:
            pattern = os.path.join(root, "chromium-*", "chrome-linux*",
                                   "chrome")
        for path in sorted(glob.glob(pattern), reverse=True):
            if os.path.isfile(path) and os.access(path, os.X_OK):
                found.append(path)
    return found


def headed_capability() -> str | None:
    """Headed fallback the current host can offer: xvfb, display, macos."""
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("linux"):
        if shutil.which("Xvfb"):
            return "xvfb"
        if os.environ.get("DISPLAY"):
            return "display"
    return None


def available_fonts() -> list[str]:
    """Bundled reader families present next to this script (empty pre-template)."""
    fonts_dir = os.path.join(WEB_CLIP_DIR, "fonts")
    try:
        names = os.listdir(fonts_dir)
    except OSError:
        return []
    lowered = [name.lower() for name in names]
    found = []
    for prefix, family in (
        ("source-serif", "Source Serif 4"),
        ("inter", "Inter"),
        ("jetbrains-mono", "JetBrains Mono"),
    ):
        if any(name.startswith(prefix) for name in lowered):
            found.append(family)
    return found


# ---------------------------------------------------------------------------
def op_ping(request: dict) -> dict:
    """No auth, no network: report versions, browser, and headed support."""
    browser = discover_browser()
    return {
        "protocol": PROTOCOL_VERSION,
        "ok": True,
        "op": "ping",
        "python": ".".join(str(part) for part in sys.version_info[:3]),
        "playwright": _package_version("playwright"),
        "pillow": _package_version("pillow"),
        "nh3": _package_version("nh3"),
        "defuddle": DEFUDDLE_VERSION,
        "browser": (
            None
            if browser is None
            else {
                "kind": browser["kind"],
                "path": browser["path"],
                "version": browser["version"],
            }
        ),
        "headed": headed_capability() if browser is not None else None,
        "fonts": available_fonts(),
    }


# Browser pipeline (needs Playwright; browser paths stay covered offline).
# ---------------------------------------------------------------------------

CONTEXT_OPTIONS: dict = {
    "viewport": {"width": 1280, "height": 1600},
    "color_scheme": "light",
    "reduced_motion": "reduce",
    "service_workers": "block",
    "accept_downloads": False,
    "locale": "en-US",
}

PRIVATE_HINT = (
    "the URL resolves to a private or local address; captures of "
    "intranet URLs are refused"
)


def _launch_with_fallback(pw, launch_kwargs: dict, *, headless: bool,
                           warnings: list[str], env: dict | None = None):
    """Launch Chromium; retry without the sandbox only when it is missing.

    ``env`` is passed through to the browser process explicitly: the
    driver does not reliably pick up ``os.environ`` changes made after it
    started, so the private Xvfb ``DISPLAY`` must travel this way.
    """
    from playwright.sync_api import Error as PlaywrightError

    args = list(CHROME_ARGS)
    extra: dict = {"env": env} if env is not None else {}
    try:
        return pw.chromium.launch(
            headless=headless,
            chromium_sandbox=True,
            args=args,
            **extra,
            **launch_kwargs,
        )
    except PlaywrightError as exc:
        if "sandbox" not in str(exc).lower():
            raise
        log("sandbox launch failed; retrying with chromium_sandbox=False")
        warnings.append("browser sandbox unavailable; ran without it")
        return pw.chromium.launch(
            headless=headless,
            chromium_sandbox=False,
            args=args,
            **extra,
            **launch_kwargs,
        )


def _start_xvfb() -> tuple[object, str]:
    """Start a private Xvfb display; return ``(proc, ":N")``."""
    for number in range(99, 200):
        socket_path = f"/tmp/.X11-unix/X{number}"
        lock_path = f"/tmp/.X{number}-lock"
        if os.path.exists(socket_path) or os.path.exists(lock_path):
            continue
        display = f":{number}"
        try:
            proc = subprocess.Popen(
                ["Xvfb", display, "-nolisten", "tcp",
                 "-screen", "0", "1440x1000x24"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except OSError as exc:
            raise AdapterFail("browser", f"could not start Xvfb: {exc}",
                              BROWSER_HINT) from exc
        for _ in range(50):
            if os.path.exists(socket_path):
                break
            if proc.poll() is not None:
                break
            time.sleep(0.1)
        if not os.path.exists(socket_path) or proc.poll() is not None:
            try:
                proc.terminate()
            except OSError:
                pass
            raise AdapterFail("browser",
                              "Xvfb exited before serving its display",
                              BROWSER_HINT)
        # The socket file appears before Xvfb accepts connections; a blind
        # headed launch loses that race, so wait for a real accept.
        import socket as socket_module

        ready_by = time.monotonic() + 5
        connected = False
        while time.monotonic() < ready_by:
            if proc.poll() is not None:
                break
            try:
                probe = socket_module.socket(socket_module.AF_UNIX,
                                             socket_module.SOCK_STREAM)
                try:
                    probe.settimeout(0.5)
                    probe.connect(socket_path)
                    connected = True
                finally:
                    probe.close()
                if connected:
                    return proc, display
            except OSError:
                time.sleep(0.1)
        try:
            proc.terminate()
        except OSError:
            pass
        raise AdapterFail("browser",
                          "Xvfb did not accept connections in time",
                          BROWSER_HINT)
    raise AdapterFail("browser", "no free X11 display for Xvfb (tried :99-:199)",
                      BROWSER_HINT)


def _stop_xvfb(proc: object) -> None:
    try:
        proc.terminate()  # type: ignore[union-attr]
        try:
            proc.wait(timeout=5)  # type: ignore[union-attr]
        except subprocess.SubprocessError:
            proc.kill()  # type: ignore[union-attr]
    except OSError:
        pass


def _same_document(left: str, right: str) -> bool:
    try:
        a, b = urlsplit(left), urlsplit(right)
    except ValueError:
        return False
    return (a.scheme, a.netloc, a.path, a.query) == (
        b.scheme, b.netloc, b.path, b.query)


def _install_guard(context, *, target_url: str, html_bytes: bytes | None,
                   hook, aborted: list[str]) -> None:
    """Abort private-network requests; fulfill replay/hook documents."""

    def _handle(route) -> None:
        request = route.request
        url = request.url
        try:
            parts = urlsplit(url)
        except ValueError:
            route.abort()
            return
        if parts.scheme not in ("http", "https"):
            route.continue_()
            return
        if hook is not None:
            try:
                if hook(route):
                    return
            except Exception as exc:  # noqa: BLE001 - a bad hook must not hang
                log(f"route hook failed for {url}: {exc}")
        if (html_bytes is not None and request.is_navigation_request()
                and _same_document(url, target_url)):
            route.fulfill(
                status=200,
                headers={"content-type": "text/html; charset=utf-8"},
                body=html_bytes,
            )
            return
        host = parts.hostname or ""
        if not host_is_global(host):
            if request.is_navigation_request():
                aborted.append(url)
            route.abort()
            return
        route.continue_()

    context.route("**/*", _handle)


def _watch_popups(context, keep: list) -> dict:
    """Close popups as they open; returns a guard to disarm around new_page.

    The ``page`` event can fire synchronously inside ``new_page()``, before
    the caller can record the intentional page, so callers set
    ``guard["armed"] = False`` around intentional creations.
    """
    guard = {"armed": True}

    def _on_page(page) -> None:
        if guard["armed"] and page not in keep:
            try:
                page.close()
            except Exception:  # noqa: BLE001 - best effort
                pass

    context.on("page", _on_page)
    return guard


def _settle_page(page, snapshot_js: str, *, poll_secs: float) -> dict:
    """Poll the probe, keep the richest healthy snapshot, lazy-load, finish."""
    page.evaluate(snapshot_js)
    best: dict | None = None
    best_words = -1
    last_gain = time.monotonic()
    end_state = "loading"
    reason = "poll exhausted"
    cf_mitigated = False
    deadline = time.monotonic() + poll_secs
    while time.monotonic() < deadline:
        probe = page.evaluate("window.__bobClip.probe()")
        cf_mitigated = cf_mitigated or bool(
            page.evaluate("window.__bobClipCfMitigated === true"))
        state, reason = classify_probe(probe, cf_mitigated)
        end_state = state
        words = probe.get("visibleWords") or 0
        if state == "ready" and words > best_words:
            best = page.evaluate("window.__bobClip.snapshot()")
            best_words = words
            last_gain = time.monotonic()
        if (state == "ready" and best is not None
                and probe.get("articleCandidate")
                and time.monotonic() - last_gain >= STABLE_SECS):
            break
        time.sleep(POLL_INTERVAL_SECS)
    warnings: list[str] = []
    if best is not None and end_state in ("ready", "loading"):
        _lazy_pass(page, best)
        final = page.evaluate("window.__bobClip.snapshot()")
        final_words = final["facts"]["counts"]["words"]
        probe = page.evaluate("window.__bobClip.probe()")
        final_ok = (
            final_words >= 0.8 * best_words
            and bool(probe.get("articleCandidate"))
            and not probe.get("challengeDom")
        )
        if final_ok:
            best = final
        else:
            warnings.append("page decayed after lazy-load; kept the best snapshot")
        end_state = "ready"
    return {"best": best, "end_state": end_state, "reason": reason,
            "warnings": warnings}


def _lazy_pass(page, best: dict) -> None:
    """Bounded scroll/font/image settle so lazy content materializes."""
    _ = best
    end = time.monotonic() + LAZY_MAX_SECS
    page.evaluate(
        "document.querySelectorAll('img').forEach("
        "(i) => { try { i.loading = 'eager'; } catch (e) {} });"
        "document.querySelectorAll('details').forEach((d) => { d.open = true; });"
    )
    height = page.evaluate("document.body ? document.body.scrollHeight : 0") or 0
    step = 0
    while time.monotonic() < end and step * 800 < (height or 0) + 800:
        page.evaluate("(y) => window.scrollTo(0, y)", step * 800)
        step += 1
        time.sleep(0.3)
    page.evaluate(
        "Promise.race([document.fonts.ready.then(() => 1), "
        "new Promise((r) => setTimeout(() => r(0), 5000))])"
    )
    while time.monotonic() < end:
        done = page.evaluate(
            "Array.from(document.images || []).every((i) => i.complete)")
        if done:
            break
        time.sleep(0.25)


def _extract_isolated(context, keep: list, snapshot_html: str,
                      final_url: str, popup_guard: dict | None = None) -> dict:
    """Run vendored Defuddle against the snapshot in an offline page."""
    with open(DEFUDDLE_JS_PATH, encoding="utf-8") as handle:
        defuddle_js = handle.read()
    if popup_guard is not None:
        popup_guard["armed"] = False
    try:
        page = context.new_page()
        keep.append(page)
    finally:
        if popup_guard is not None:
            popup_guard["armed"] = True
    try:
        page.route("**/*", lambda route: route.abort())
        page.goto("about:blank")
        page.evaluate(
            "(markup) => { document.open(); document.write(markup); "
            "document.close(); }",
            snapshot_html,
        )
        page.add_script_tag(content=defuddle_js)
        result = page.evaluate(
            "(url) => new Defuddle(document, { url }).parse()", final_url)
    except Exception as exc:  # noqa: BLE001 - mapped to thin below
        raise AdapterFail("thin", f"article extraction failed: {exc}",
                          THIN_HINT) from exc
    finally:
        try:
            page.close()
        except Exception:  # noqa: BLE001 - best effort
            pass
    if not isinstance(result, dict):
        raise AdapterFail("thin", "article extraction returned no content",
                          THIN_HINT)
    log("extraction: isolated Defuddle page")
    return result


# ---------------------------------------------------------------------------
# Image localization.
# ---------------------------------------------------------------------------

_IMG_TAG_RE = re.compile(r"<img\b[^>]*>", re.IGNORECASE)
_SRC_ATTR_RE = re.compile(
    r"""\bsrc\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)""", re.IGNORECASE)
_DATA_URI_RE = re.compile(r"^data:([^;,]+)?(;base64)?,(.*)$", re.DOTALL)
_SVG_TAG_RE = re.compile(
    r"""<svg\b[^>]*data-bob-svg="1"[^>]*>.*?</svg>""",
    re.IGNORECASE | re.DOTALL,
)
_CONTENT_TYPE_EXT = {
    "image/jpeg": "jpg",
    "image/png": "png",
    "image/gif": "gif",
    "image/webp": "webp",
    "image/avif": "avif",
    "image/svg+xml": "svg",
}
_PIL_FORMAT_EXT = {
    "JPEG": "jpg",
    "PNG": "png",
    "GIF": "gif",
    "WEBP": "webp",
    "AVIF": "avif",
}


def decode_data_uri(src: str) -> tuple[bytes, str | None] | None:
    """Split a ``data:`` URI into ``(bytes, mime)``; ``None`` when invalid."""
    match = _DATA_URI_RE.match(src.strip())
    if not match:
        return None
    mime, encoding, payload = match.group(1), match.group(2), match.group(3)
    try:
        if encoding:
            return base64.b64decode(payload, validate=False), mime
        from urllib.parse import unquote_to_bytes

        return unquote_to_bytes(payload), mime
    except (ValueError, base64.binascii.Error):
        return None


def sniff_extension(data: bytes, content_type: str | None) -> str | None:
    """Map bytes (plus a header hint) to an image extension, or ``None``."""
    mime = (content_type or "").split(";")[0].strip().lower()
    if mime in _CONTENT_TYPE_EXT:
        return _CONTENT_TYPE_EXT[mime]
    head = data[:1024].lstrip()
    if head.startswith(b"<svg") or (head.startswith(b"<?xml")
                                    and b"<svg" in data[:4096]):
        return "svg"
    try:
        from PIL import Image

        with Image.open(__import__("io").BytesIO(data)) as img:
            return _PIL_FORMAT_EXT.get((img.format or "").upper())
    except Exception:  # noqa: BLE001 - unknown bytes are not images
        return None


def image_dimensions(data: bytes) -> tuple[int, int] | None:
    """Pixel dimensions of raster bytes; ``None`` for SVG or unknowns."""
    try:
        from PIL import Image

        with Image.open(__import__("io").BytesIO(data)) as img:
            return (int(img.width), int(img.height))
    except Exception:  # noqa: BLE001 - SVG and unknowns have no raster size
        return None


def _clean_inline_svg(markup: str) -> str:
    """Strip active content from an inline SVG before saving it as a file."""
    markup = re.sub(r"<script\b.*?</script\s*>", "", markup,
                    flags=re.IGNORECASE | re.DOTALL)
    markup = re.sub(r"<foreignObject\b.*?</foreignObject\s*>", "", markup,
                    flags=re.IGNORECASE | re.DOTALL)
    markup = re.sub(r"""\s+on[a-zA-Z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)""",
                    "", markup)
    markup = re.sub(r"""\s+(?:xlink:)?href\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)""",
                    "", markup)
    return markup


def _localize_images(api, markup: str, assets_dir: str, page_url: str,
                     deadline: float, warnings: list[str]) -> tuple[str, dict]:
    """Download kept images into ``assets_dir``; drop the ones that fail."""
    manifest = {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0}
    total_bytes = 0
    counter = 0

    def _replace_img(match: re.Match) -> str:
        nonlocal counter, total_bytes
        tag = match.group(0)
        manifest["total"] += 1
        src_match = _SRC_ATTR_RE.search(tag)
        # Serialized HTML escapes ampersands; unescape before requesting.
        raw_src = html_module.unescape(
            src_match.group(1).strip("\"'") if src_match else "")
        if time.monotonic() > deadline:
            manifest["failed"] += 1
            warnings.append(f"image skipped past the time budget: {raw_src}")
            return ""
        data: bytes | None = None
        content_type: str | None = None
        if raw_src.startswith("data:"):
            decoded = decode_data_uri(raw_src)
            if decoded is None:
                manifest["failed"] += 1
                warnings.append("dropped an undecodable data: image")
                return ""
            data, content_type = decoded
        elif raw_src.startswith(("http://", "https://")):
            try:
                response = api.get(
                    raw_src,
                    headers={"Referer": page_url},
                    timeout=REQUEST_TIMEOUT_MS,
                )
            except Exception as exc:  # noqa: BLE001 - drop and continue
                manifest["failed"] += 1
                warnings.append(f"dropped image {raw_src}: {exc}")
                return ""
            if response.status >= 400:
                manifest["failed"] += 1
                warnings.append(
                    f"dropped image {raw_src}: HTTP {response.status}")
                return ""
            content_type = (response.headers.get("content-type")
                            or response.headers.get("Content-Type"))
            try:
                data = response.body()
            except Exception as exc:  # noqa: BLE001 - drop and continue
                manifest["failed"] += 1
                warnings.append(f"dropped image {raw_src}: {exc}")
                return ""
        else:
            manifest["failed"] += 1
            warnings.append(f"dropped image with unsupported src: {raw_src}")
            return ""
        if data is None or len(data) > IMAGE_MAX_BYTES:
            manifest["failed"] += 1
            warnings.append(f"dropped oversize image {raw_src[:120]}")
            return ""
        if total_bytes + len(data) > IMAGES_TOTAL_MAX_BYTES:
            manifest["failed"] += 1
            warnings.append("dropped image past the 60 MiB total image budget")
            return ""
        dims = image_dimensions(data)
        if dims is not None and dims[0] < 64 and dims[1] < 64:
            manifest["skipped_small"] += 1
            return ""
        ext = sniff_extension(data, content_type)
        if ext is None:
            manifest["failed"] += 1
            warnings.append(f"dropped image of unknown type {raw_src[:120]}")
            return ""
        counter += 1
        name = f"img-{counter:03d}.{ext}"
        with open(os.path.join(assets_dir, name), "wb") as handle:
            handle.write(data)
        total_bytes += len(data)
        manifest["kept"] += 1
        return _SRC_ATTR_RE.sub(f'src="assets/{name}"', tag, count=1)

    markup = _IMG_TAG_RE.sub(_replace_img, markup)

    svg_counter = 0

    def _replace_svg(match: re.Match) -> str:
        nonlocal svg_counter
        manifest["total"] += 1
        cleaned = _clean_inline_svg(match.group(0))
        svg_counter += 1
        name = f"svg-{svg_counter:03d}.svg"
        with open(os.path.join(assets_dir, name), "w",
                   encoding="utf-8") as handle:
            handle.write(cleaned)
        manifest["kept"] += 1
        return f'<img src="assets/{name}" alt="">'

    markup = _SVG_TAG_RE.sub(_replace_svg, markup)
    return markup, manifest


# ---------------------------------------------------------------------------
# Capture orchestration.
# ---------------------------------------------------------------------------

def _check_deadline(deadline: float) -> None:
    if time.monotonic() > deadline:
        raise AdapterFail("timeout", "capture took longer than 150 s",
                          "retry with --html from a browser-saved page")


def _direct_pdf_response(*, browser_info: dict, mode: str, final_url: str,
                         host: str, overrides: dict, out_pdf: str | None,
                         pdf_bytes: int | None, dry_run: bool) -> dict:
    title = overrides.get("title")
    author = overrides.get("author")
    published = overrides.get("published")
    sources = {}
    if title:
        sources["title"] = "override"
    if author:
        sources["author"] = "override"
    if published:
        sources["published"] = "override"
    return {
        "protocol": PROTOCOL_VERSION,
        "ok": True,
        "op": "capture",
        "kind": "pdf",
        "final_url": final_url,
        "title": title,
        "author": author,
        "published": published,
        "site": host,
        "description": None,
        "metadata_sources": sources,
        "word_count": 0,
        "capture": {
            "browser": browser_info["kind"],
            "browser_version": browser_info["version"],
            "mode": mode,
            "retried_after_challenge": False,
        },
        "fidelity": {
            "status": "ok",
            "page_large_media": 0,
            "kept_large_media": 0,
            "page_code_blocks": 0,
            "kept_code_blocks": 0,
            "page_words": 0,
            "kept_words": 0,
        },
        "images": {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0},
        "pdf_bytes": pdf_bytes,
        "warnings": [],
    }


def _is_pdf_response(response) -> bool:
    try:
        headers = response.headers or {}
    except Exception:  # noqa: BLE001 - treat header failures as non-PDF
        return False
    ctype = headers.get("content-type", "")
    return "application/pdf" in ctype.lower()


def _probe_pdf(api, url: str) -> bool:
    """HEAD (falling back to ranged GET) to spot PDF URLs before navigating."""
    try:
        response = api.head(url, timeout=REQUEST_TIMEOUT_MS)
        if response.status < 400:
            return _is_pdf_response(response)
    except Exception:  # noqa: BLE001 - fall back to a ranged GET
        pass
    try:
        response = api.get(url, headers={"Range": "bytes=0-0"},
                           timeout=REQUEST_TIMEOUT_MS)
        return _is_pdf_response(response)
    except Exception:  # noqa: BLE001 - navigation will surface real errors
        return False


def _download_remote_pdf(api, url: str, out_pdf: str) -> int:
    try:
        response = api.get(url, timeout=60000)
    except Exception as exc:  # noqa: BLE001 - network failure
        raise AdapterFail("network", f"could not download the PDF: {exc}",
                          PRIVATE_HINT) from exc
    if response.status >= 400:
        raise AdapterFail("network", f"server returned HTTP {response.status}",
                          PRIVATE_HINT)
    try:
        body = response.body()
    except Exception as exc:  # noqa: BLE001 - truncated download
        raise AdapterFail("network", f"could not read the PDF: {exc}",
                          PRIVATE_HINT) from exc
    if len(body) > PDF_MAX_BYTES:
        raise AdapterFail(
            "render", "the source PDF is larger than 95 MiB",
            "vault sync refuses files of 95 MiB or more")
    with open(out_pdf, "wb") as handle:
        handle.write(body)
    return len(body)


def run_capture(request: dict, *, allow_headed: bool = True,
                route_hook=None) -> tuple[dict, dict]:
    """Run one capture request; return ``(response, debug)``.

    ``route_hook`` is a test seam: ``hook(route) -> True`` fulfills the
    request itself (fixtures), ``False`` falls through to the guard.
    """
    from playwright.sync_api import (
        Error as PlaywrightError,
        TimeoutError as PlaywrightTimeoutError,
        sync_playwright,
    )

    start = time.monotonic()
    deadline = start + OVERALL_TIMEOUT_SECS
    debug: dict = {"images": {}, "fidelity": {}, "mode": None}
    warnings: list[str] = []

    url = request.get("url")
    if not isinstance(url, str):
        raise AdapterFail("invalid_request", "capture request needs a url")
    try:
        parts = urlsplit(url)
    except ValueError as exc:
        raise AdapterFail("invalid_request", f"unparsable URL: {exc}") from exc
    if parts.scheme not in ("http", "https") or not parts.hostname:
        raise AdapterFail("invalid_request",
                          "only http(s) URLs with a host can be captured")
    host = parts.hostname
    workdir = request.get("workdir")
    if not isinstance(workdir, str) or not os.path.isabs(workdir):
        raise AdapterFail("invalid_request",
                          "capture request needs an absolute workdir")
    out_pdf = request.get("out_pdf")
    if not isinstance(out_pdf, str):
        raise AdapterFail("invalid_request",
                          "capture request needs an out_pdf path")
    dry_run = bool(request.get("dry_run", False))
    html_path = request.get("html_path")
    html_bytes: bytes | None = None
    if html_path is not None:
        if not isinstance(html_path, str):
            raise AdapterFail("invalid_request", "html_path must be a path")
        try:
            with open(html_path, "rb") as handle:
                html_bytes = handle.read()
        except OSError as exc:
            raise AdapterFail("invalid_request",
                              f"html file not found: {exc}") from exc
    overrides = request.get("overrides") or {}
    if not isinstance(overrides, dict):
        raise AdapterFail("invalid_request", "overrides must be an object")
    for key in ("title", "author", "published"):
        if overrides.get(key) is not None and not isinstance(
                overrides.get(key), str):
            raise AdapterFail("invalid_request",
                              f"override {key} must be a string or null")

    os.makedirs(workdir, mode=0o700, exist_ok=True)
    assets_dir = os.path.join(workdir, "assets")
    os.makedirs(assets_dir, exist_ok=True)
    # Chrome's profile and socket paths break under SASE's deep per-agent
    # TMPDIR, so keep everything under the short scratch directory.
    os.environ["TMPDIR"] = workdir

    try:
        with open(SNAPSHOT_JS_PATH, encoding="utf-8") as handle:
            snapshot_js = handle.read()
    except OSError as exc:
        raise AdapterFail("internal",
                          f"snapshot.js is missing: {exc}") from exc
    if "window.__bobClip" not in snapshot_js:
        raise AdapterFail("internal", "snapshot.js defines no __bobClip")

    # With a route hook (self-test fixtures) the hook fulfills the document
    # before any DNS happens, so skip the fail-fast check; the per-request
    # guard still aborts anything the hook does not serve.
    if html_bytes is None and route_hook is None and not host_is_global(host):
        raise AdapterFail("network",
                          f"{host} resolves to a private or local address",
                          PRIVATE_HINT)

    browser_info = discover_browser()
    if browser_info is None:
        raise AdapterFail("browser",
                          "no usable browser found", BROWSER_HINT)

    with sync_playwright() as pw:
        contexts: list = []
        browser = None
        xvfb_proc = None
        try:
            probe_api_context = pw.request.new_context()
            try:
                if html_bytes is None and _probe_pdf(probe_api_context, url):
                    if dry_run:
                        return (_direct_pdf_response(
                            browser_info=browser_info, mode="direct-pdf",
                            final_url=url, host=host, overrides=overrides,
                            out_pdf=out_pdf, pdf_bytes=None, dry_run=True),
                            debug)
                    size = _download_remote_pdf(probe_api_context, url, out_pdf)
                    return (_direct_pdf_response(
                        browser_info=browser_info, mode="direct-pdf",
                        final_url=url, host=host, overrides=overrides,
                        out_pdf=out_pdf, pdf_bytes=size, dry_run=False),
                        debug)
            finally:
                probe_api_context.dispose()

            mode = "headless"
            if html_bytes is not None:
                mode = "html-file"
            browser = _launch_with_fallback(
                pw, browser_info["launch"], headless=True, warnings=warnings)
            context = browser.new_context(**CONTEXT_OPTIONS)
            contexts.append(context)
            keep: list = []
            aborted: list[str] = []
            _install_guard(context, target_url=url, html_bytes=html_bytes,
                           hook=route_hook, aborted=aborted)
            page = context.new_page()
            keep.append(page)
            popup_guard = _watch_popups(context, keep)
            try:
                nav = page.goto(url, wait_until="domcontentloaded",
                                timeout=GOTO_TIMEOUT_MS)
            except PlaywrightTimeoutError as exc:
                raise AdapterFail(
                    "timeout", f"navigation timed out after 45 s: {url}",
                    THIN_HINT) from exc
            except PlaywrightError as exc:
                if aborted:
                    raise AdapterFail(
                        "network",
                        f"refused a private-network request for {url}",
                        PRIVATE_HINT) from exc
                raise AdapterFail("network", f"navigation failed: {exc}",
                                  PRIVATE_HINT) from exc
            if nav is not None and _is_pdf_response(nav):
                if dry_run:
                    return (_direct_pdf_response(
                        browser_info=browser_info, mode="direct-pdf",
                        final_url=nav.url or url, host=host,
                        overrides=overrides, out_pdf=out_pdf, pdf_bytes=None,
                        dry_run=True), debug)
                size = _download_remote_pdf(
                    context.request, nav.url or url, out_pdf)
                return (_direct_pdf_response(
                    browser_info=browser_info, mode="direct-pdf",
                    final_url=nav.url or url, host=host,
                    overrides=overrides, out_pdf=out_pdf, pdf_bytes=size,
                    dry_run=False), debug)
            if nav is not None and nav.status is not None and nav.status >= 400:
                if nav.status == 403:
                    # Cloudflare serves its bot challenge as a 403 page;
                    # let the probe and the headed fallback decide.
                    log("navigation returned HTTP 403; treating as a "
                        "possible challenge")
                else:
                    raise AdapterFail(
                        "network", f"server returned HTTP {nav.status}",
                        PRIVATE_HINT)
            cf_headers = {}
            if nav is not None:
                try:
                    headers = nav.headers or {}
                    cf_headers = {
                        key: headers.get(key)
                        for key in ("cf-mitigated", "server") if headers.get(key)
                    }
                except Exception:  # noqa: BLE001 - headers are best effort
                    pass
            debug["nav_headers"] = cf_headers
            if cf_headers.get("cf-mitigated") == "challenge":
                page.evaluate("window.__bobClipCfMitigated = true")
            final_url = page.url or url

            _check_deadline(deadline)
            settled = _settle_page(page, snapshot_js, poll_secs=POLL_MAX_SECS)
            warnings.extend(settled["warnings"])
            retried = False
            if (settled["end_state"] == "challenged"
                    and html_bytes is None and allow_headed):
                for old in contexts:
                    try:
                        old.close()
                    except Exception:  # noqa: BLE001 - best effort
                        pass
                contexts = []
                try:
                    browser.close()
                except Exception:  # noqa: BLE001 - best effort
                    pass
                browser, mode, xvfb_proc = _launch_headed(
                    pw, browser_info, warnings)
                debug["mode"] = mode
                context = browser.new_context(**CONTEXT_OPTIONS)
                contexts.append(context)
                keep = []
                aborted = []
                _install_guard(context, target_url=url, html_bytes=None,
                               hook=route_hook, aborted=aborted)
                page = context.new_page()
                keep.append(page)
                popup_guard = _watch_popups(context, keep)
                try:
                    page.goto(url, wait_until="domcontentloaded",
                              timeout=GOTO_TIMEOUT_MS)
                except PlaywrightTimeoutError as exc:
                    raise AdapterFail(
                        "timeout",
                        f"headed navigation timed out after 45 s: {url}",
                        CHALLENGE_STUCK_HINT) from exc
                except PlaywrightError as exc:
                    raise AdapterFail(
                        "network", f"headed navigation failed: {exc}",
                        CHALLENGE_STUCK_HINT) from exc
                final_url = page.url or url
                _check_deadline(deadline)
                settled = _settle_page(page, snapshot_js,
                                       poll_secs=HEADED_WAIT_SECS)
                warnings.extend(settled["warnings"])
                retried = True
            if settled["end_state"] == "challenged":
                if html_bytes is not None or not allow_headed:
                    raise AdapterFail("blocked",
                                      "the page still shows a bot challenge",
                                      CHALLENGE_STUCK_HINT)
                raise AdapterFail("blocked",
                                  "the bot challenge did not clear",
                                  CHALLENGE_STUCK_HINT)
            if settled["end_state"] == "login":
                raise AdapterFail("blocked",
                                  f"the page needs a login ({settled['reason']})",
                                  LOGIN_HINT)
            snapshot = settled["best"]
            if snapshot is None:
                raise AdapterFail("thin",
                                  "no readable snapshot settled in time",
                                  THIN_HINT)
            facts = snapshot.get("facts", {})
            counts = facts.get("counts", {})
            meta = facts.get("meta", {})
            jsonld = facts.get("jsonLd", {}) or {}

            _check_deadline(deadline)
            extracted = _extract_isolated(context, keep, snapshot["html"],
                                          final_url, popup_guard)
            content = extracted.get("content") or ""

            slug = url_slug(final_url)
            title, title_source = layer_title(
                facts.get("h1"), meta.get("ogTitle"),
                extracted.get("title"), slug)
            author, author_source = layer_author(
                jsonld.get("author"),
                meta.get("articleAuthor") or meta.get("author"),
                facts.get("byline"), extracted.get("author"),
                facts.get("embedAuthors") or [])
            published, published_source = layer_published(
                jsonld.get("datePublished"), meta.get("publishedTime"),
                facts.get("visibleDates") or [],
                extracted.get("published"),
                facts.get("embedDates") or [])
            site, site_source = layer_site(
                meta.get("siteName"), extracted.get("site"), host)
            sources: dict = {}
            if title_source:
                sources["title"] = title_source
            if author_source:
                sources["author"] = author_source
            if published_source:
                sources["published"] = published_source
            if site_source:
                sources["site"] = site_source
            if overrides.get("title"):
                title, sources["title"] = overrides["title"], "override"
            if overrides.get("author"):
                author, sources["author"] = overrides["author"], "override"
            if overrides.get("published"):
                published = parse_published(overrides["published"])
                if published is None:
                    raise AdapterFail(
                        "invalid_request",
                        "override published must be YYYY-MM-DD")
                sources["published"] = "override"

            cleaned, dropped_heading = clean_text_html(content, title)
            if dropped_heading:
                log("dropped a leading heading repeating the title")
            sanitized = sanitize_html(cleaned)

            images = {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0}
            pdf_bytes: int | None = None
            if not dry_run:
                _check_deadline(deadline)
                sanitized, images = _localize_images(
                    context.request, sanitized, assets_dir, final_url,
                    deadline, warnings)
            kept_counts = {
                "large_media": count_tag(sanitized, "img"),
                "code_blocks": count_tag(sanitized, "pre"),
                "words": html_word_count(sanitized),
            }
            page_counts = {
                "large_media": counts.get("largeMedia", 0),
                "code_blocks": counts.get("codeBlocks", 0),
                "words": counts.get("words", 0),
            }
            # One verdict on the final markup, so dropped images degrade
            # the reported status instead of vanishing from it.
            status, fidelity_warnings, thin = fidelity_verdict(
                page_counts, kept_counts)
            if thin is not None:
                raise thin
            warnings.extend(fidelity_warnings)

            if not dry_run:
                article = {
                    "title": title,
                    "author": author,
                    "published": published,
                    "site": site,
                    "description": extracted.get("description"),
                    "html": sanitized,
                    "source_url": final_url,
                    "fidelity": {"status": status, **page_counts},
                    "images": images,
                    "metadata_sources": sources,
                    "word_count": kept_counts["words"],
                }
                article_json = os.path.join(workdir, "article.json")
                with open(article_json, "w", encoding="utf-8") as handle:
                    json.dump(article, handle)
                if WEB_CLIP_DIR not in sys.path:
                    sys.path.insert(0, WEB_CLIP_DIR)
                import web_clip_render

                pdf_bytes = web_clip_render.render(
                    article_json, out_pdf,
                    _RendererLauncher(pw, browser_info))["pdf_bytes"]
                if pdf_bytes > PDF_MAX_BYTES:
                    raise AdapterFail(
                        "render", "the printed PDF is larger than 95 MiB",
                        "vault sync refuses files of 95 MiB or more")
                if pdf_bytes > PDF_WARN_BYTES:
                    warnings.append(
                        f"printed PDF is {pdf_bytes / 1048576:.1f} MiB "
                        "(vault sync refuses 95 MiB or more)")

            response = {
                "protocol": PROTOCOL_VERSION,
                "ok": True,
                "op": "capture",
                "kind": "article",
                "final_url": final_url,
                "title": title,
                "author": author,
                "published": published,
                "site": site,
                "description": extracted.get("description"),
                "metadata_sources": sources,
                "word_count": kept_counts["words"],
                "capture": {
                    "browser": browser_info["kind"],
                    "browser_version": browser_info["version"],
                    "mode": mode,
                    "retried_after_challenge": retried,
                },
                "fidelity": {
                    "status": status,
                    "page_large_media": page_counts["large_media"],
                    "kept_large_media": kept_counts["large_media"],
                    "page_code_blocks": page_counts["code_blocks"],
                    "kept_code_blocks": kept_counts["code_blocks"],
                    "page_words": page_counts["words"],
                    "kept_words": kept_counts["words"],
                },
                "images": images,
                "pdf_bytes": pdf_bytes,
                "warnings": warnings,
            }
            debug.update({"images": images,
                          "fidelity": response["fidelity"], "mode": mode,
                          "snapshot_html": snapshot["html"],
                          "sanitized_html": sanitized,
                          "facts": facts})
            return response, debug
        finally:
            for old in contexts:
                try:
                    old.close()
                except Exception:  # noqa: BLE001 - best effort
                    pass
            if browser is not None:
                try:
                    browser.close()
                except Exception:  # noqa: BLE001 - best effort
                    pass
            if xvfb_proc is not None:
                _stop_xvfb(xvfb_proc)


class _RendererLauncher:
    """Headless-only browser factory for the placeholder renderer."""

    def __init__(self, pw, browser_info: dict) -> None:
        self._pw = pw
        self._browser_info = browser_info

    def __call__(self, *, headless: bool = True):
        """Launch a separate headless browser for ``page.pdf``."""
        return self.launch(headless=headless)

    def launch(self, *, headless: bool = True):
        """Launch a separate headless browser for ``page.pdf``."""
        return _launch_with_fallback(
            self._pw, self._browser_info["launch"], headless=True,
            warnings=[])


def _launch_headed(pw, browser_info: dict,
                   warnings: list[str]) -> tuple[object, str, object | None]:
    """Relaunch headed after a headless bot challenge; return browser, mode."""
    capability = headed_capability()
    if capability is None:
        raise AdapterFail("blocked",
                          "this site blocks headless browsers and no headed "
                          "display is available",
                          BLOCKED_HINT)
    if capability == "xvfb":
        proc, display = _start_xvfb()
        os.environ["DISPLAY"] = display
        headed_env = dict(os.environ, DISPLAY=display)
        try:
            browser = _launch_with_fallback(
                pw, browser_info["launch"], headless=False,
                warnings=warnings, env=headed_env)
        except Exception as exc:
            _stop_xvfb(proc)
            raise _headed_launch_fail(exc) from exc
        return browser, "headed-xvfb", proc
    try:
        if capability == "display":
            browser = _launch_with_fallback(
                pw, browser_info["launch"], headless=False,
                warnings=warnings)
            return browser, "headed-display", None
        browser = pw.chromium.launch(
            headless=False,
            chromium_sandbox=True,
            args=CHROME_ARGS + ["--window-position=-32000,-32000"],
            **browser_info["launch"],
        )
    except Exception as exc:
        raise _headed_launch_fail(exc) from exc
    return browser, "headed-macos", None


def _headed_launch_fail(exc: Exception) -> AdapterFail:
    """Map a headed-launch crash to a ``browser`` failure, never ``internal``."""
    if isinstance(exc, AdapterFail):
        return exc
    return AdapterFail("browser", f"headed browser launch failed: {exc}",
                       BROWSER_HINT)


def op_capture(request: dict) -> dict:
    """Run one capture and return its protocol response."""
    response, _debug = run_capture(request)
    return response


OPS = {
    "ping": op_ping,
    "capture": op_capture,
}


def _parent_alive(pid: int) -> bool:
    """Whether the parent process still exists (for the watchdog)."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return True
    return True


def _maybe_start_parent_watchdog() -> None:
    """Exit when the Rust parent is gone (Ctrl-C orphan fix).

    `spawn_adapter` sets `BOB_WEB_CLIP_PARENT_PID`; a daemon thread checks
    every 0.5s and exits when the parent disappears. Unset or invalid
    values start no watchdog.
    """
    raw = os.environ.get("BOB_WEB_CLIP_PARENT_PID")
    if not raw:
        return
    try:
        pid = int(raw)
    except ValueError:
        return
    if pid <= 0:
        return

    def _watch() -> None:
        while True:
            time.sleep(0.5)
            if not _parent_alive(pid):
                os._exit(1)

    thread = threading.Thread(target=_watch, daemon=True)
    thread.start()


def validate_request(data: object) -> dict:
    """Check the request envelope and return it, or raise ``AdapterFail``."""
    if not isinstance(data, dict):
        raise AdapterFail("invalid_request",
                          "adapter request must be a JSON object")
    protocol = data.get("protocol")
    if protocol != PROTOCOL_VERSION:
        raise AdapterFail(
            "invalid_request",
            f"unsupported protocol {protocol!r}: this adapter speaks "
            f"protocol {PROTOCOL_VERSION}",
        )
    op = data.get("op")
    if not isinstance(op, str) or op not in OPS:
        raise AdapterFail(
            "invalid_request",
            f"unknown op {op!r}: expected one of {', '.join(sorted(OPS))}",
        )
    return data


def dispatch(data: dict) -> dict:
    """Run one validated request and return its response object."""
    return OPS[data["op"]](data)


def _fixture_hook_factory(directory: str, statuses: dict | None = None):
    """Serve ``directory/*.html`` at ``https://fixture.bob-clip.invalid/``.

    ``statuses`` maps a fixture name to its HTTP status (default 200); the
    403 entry proves a challenge page served as HTTP 403 still reaches the
    challenge fallback instead of failing as a network error.
    """

    def _hook(route) -> bool:
        try:
            name = urlsplit(route.request.url).path.lstrip("/")
        except ValueError:
            return False
        if urlsplit(route.request.url).hostname != FIXTURE_HOST:
            return False
        if not name or "/" in name or not name.endswith(".html"):
            return False
        path = os.path.join(directory, name)
        if not os.path.isfile(path):
            return False
        with open(path, "rb") as handle:
            body = handle.read()
        status = (statuses or {}).get(name, 200)
        route.fulfill(status=status,
                      headers={"content-type": "text/html; charset=utf-8"},
                      body=body)
        return True

    return _hook


def self_test() -> int:
    """Exercise the offline paths, plus fixtures when a browser exists."""
    failures: list[str] = []

    def check(label: str, condition: bool) -> None:
        if not condition:
            failures.append(label)

    # -- Date parsing -----------------------------------------------------
    check("date april-long",
          parse_published("April 27, 2026") == "2026-04-27")
    check("date april-short",
          parse_published("Apr 5, 2026") == "2026-04-05")
    check("date day-first",
          parse_published("27 April 2026") == "2026-04-27")
    check("date iso", parse_published("2026-04-27") == "2026-04-27")
    check("date iso-datetime",
          parse_published("2026-03-08T03:45:54Z") == "2026-03-08")
    check("date sept", parse_published("Sept. 1, 2026") == "2026-09-01")
    check("date garbage", parse_published("sometime last spring") is None)
    check("date empty", parse_published("") is None)
    check("date non-string", parse_published(None) is None)

    # -- Probe classification on recorded probe dicts ----------------------
    challenged = {"title": "Just a moment...",
                  "challengeDom": True, "loginPhrases": [],
                  "articleCandidate": False, "visibleWords": 12}
    check("probe challenge",
          classify_probe(challenged, False)[0] == "challenged")
    plain = {"title": "Real article", "challengeDom": False,
             "loginPhrases": [], "articleCandidate": True,
             "visibleWords": 9000}
    check("probe cf-header",
          classify_probe(plain, True)[0] == "challenged")
    check("probe empty-title",
          classify_probe({**plain, "title": "  "}, False)[0] == "loading")
    login = {"title": "Inside the launch", "challengeDom": False,
             "loginPhrases": ["subscribe to continue reading"],
             "articleCandidate": True, "visibleWords": 120}
    check("probe login", classify_probe(login, False)[0] == "login")
    check("probe login-needs-few-words",
          classify_probe({**login, "visibleWords": 900}, False)[0] == "ready")
    check("probe ready", classify_probe(plain, False)[0] == "ready")

    # -- Metadata layering, including the OpenAI embed-date trap -----------
    title, source = layer_title(
        "An open-source spec for orchestration: Symphony",
        "An open-source spec for orchestration: Symphony | Example",
        "An open-source spec for orchestration: Symphony",
        "article")
    check("title h1 wins", title.startswith("An open-source spec")
          and source == "h1")
    check("title suffix stripped",
          layer_title(None, "Some Post | Example", None, "x")[0]
          == "Some Post")
    check("title url-slug fallback",
          layer_title(None, None, None, "hello_world")[1] == "url-slug")
    author, asource = layer_author(
        None, None, "Alex Kotliarskyi, Victor Zhu, and Zach Brock",
        "Someone Else", ["@symphonydev"])
    check("author byline", author.startswith("Alex") and asource == "byline")
    check("author embed-handle discarded",
          layer_author(None, "@symphonydev", None, "Fallback Name",
                       ["@symphonydev"]) == ("Fallback Name", "defuddle"))
    check("author by-prefix stripped",
          layer_author(None, None, "By Jane Reporter", None, [])[0]
          == "Jane Reporter")
    published, psource = layer_published(
        None, None, ["April 27, 2026"], "2026-03-08T03:45:54Z",
        ["2026-03-08T03:45:54Z"])
    check("published visible beats embed tweet date",
          published == "2026-04-27" and psource == "visible-date")
    check("published defuddle lonely embed date refused",
          layer_published(None, None, [], "2026-03-08T03:45:54Z",
                          ["2026-03-08T03:45:54Z"]) == (None, None))
    check("published json-ld first",
          layer_published("2026-01-02", "2026-05-05", ["April 27, 2026"],
                          None, []) == ("2026-01-02", "json-ld"))
    check("site og", layer_site("Example", "Other", "example.com")
          == ("Example", "og:site_name"))
    check("site host fallback",
          layer_site(None, None, "example.com")
          == ("example.com", "host"))

    # -- Text cleanup -------------------------------------------------------
    cleaned, dropped = clean_text_html(
        "<h1>Symphony</h1><p>soft\xadhy\u00adphen and\u2060word joiner\u200bhere"
        " <a>link (opens in a new window)</a></p>", "Symphony!")
    check("cleanup drops repeating lead heading", dropped
          and "<h1>" not in cleaned)
    check("cleanup strips invisible chars",
          "\xad" not in cleaned and "\ufeff" not in cleaned
          and "\u2060" not in cleaned and "\u200b" not in cleaned)
    check("cleanup strips open suffix",
          "(opens in a new window)" not in cleaned and "link</a>" in cleaned)
    check("cleanup keeps foreign heading",
          clean_text_html("<h1>Other</h1><p>x</p>", "Symphony")[1] is False)

    # -- nh3 allowlist on the hostile fixture --------------------------------
    fixtures_dir = os.path.join(WEB_CLIP_DIR, "fixtures")
    with open(os.path.join(fixtures_dir, "hostile.html"),
              encoding="utf-8") as handle:
        hostile = handle.read()
    safe = sanitize_html(hostile)
    check("sanitize kills scripts", "<script" not in safe)
    check("sanitize kills handlers", "onerror" not in safe)
    check("sanitize kills javascript urls", "javascript:" not in safe)
    check("sanitize kills iframes", "<iframe" not in safe)
    check("sanitize kills forms", "<form" not in safe and "<input" not in safe)
    check("sanitize keeps footnote anchors",
          'id="fnref-1"' in safe and 'href="#fn-1"' in safe)
    check("sanitize keeps svg shell",
          "<svg" in safe and "<circle" in safe)
    check("sanitize keeps math",
          "<math>" in safe and "<mi>x</mi>" in safe)
    check("sanitize keeps code and details",
          "<pre>" in safe and "<details>" in safe)
    check("sanitize keeps prose",
          "genuine text" in safe and "footnote anchor" in safe)

    # -- Fidelity verdicts ---------------------------------------------------
    status, warnings, thin = fidelity_verdict(
        {"large_media": 2, "code_blocks": 1, "words": 10858},
        {"large_media": 2, "code_blocks": 1, "words": 8921})
    check("fidelity ok", status == "ok" and warnings == [] and thin is None)
    status, warnings, thin = fidelity_verdict(
        {"large_media": 2, "code_blocks": 1, "words": 10858},
        {"large_media": 1, "code_blocks": 0, "words": 3000})
    check("fidelity warn",
          status == "warn" and len(warnings) == 3 and thin is None)
    _status, _warnings, thin = fidelity_verdict(
        {"large_media": 0, "code_blocks": 0, "words": 900},
        {"large_media": 0, "code_blocks": 0, "words": 120})
    check("fidelity thin", thin is not None and thin.kind == "thin")
    check("fidelity thin needs long page",
          fidelity_verdict({"words": 300}, {"words": 120})[2] is None)

    # -- Envelope validation --------------------------------------------------
    for bad in ({"protocol": 999, "op": "ping"},
                {"protocol": 1, "op": "bogus"},
                {"protocol": 1, "op": ["ping"]},
                ["ping"]):
        try:
            validate_request(bad)
            check(f"envelope rejects {bad!r}", False)
        except AdapterFail as exc:
            check(f"envelope rejects {bad!r}",
                  exc.kind == "invalid_request")
    check("defuddle pinned",
          DEFUDDLE_VERSION == "0.19.4"
          and os.path.isfile(DEFUDDLE_JS_PATH))
    check("snapshot.js present",
          os.path.isfile(SNAPSHOT_JS_PATH))
    check("vendor readme present",
          os.path.isfile(os.path.join(WEB_CLIP_DIR, "vendor", "README.md")))

    # -- Browser-backed fixture checks (offline via routing) -----------------
    try:
        browser = discover_browser()
    except Exception as exc:  # noqa: BLE001 - discovery must not fail tests
        log(f"browser discovery raised: {exc}")
        browser = None
    if browser is None:
        print("self-test: skipped browser checks (no browser)")
    else:
        import tempfile

        hook = _fixture_hook_factory(fixtures_dir)

        def _capture(name: str, **kw) -> tuple[dict, dict, str]:
            workdir = tempfile.mkdtemp(prefix="bob-clip-test-")
            req = {"protocol": 1, "op": "capture",
                   "url": f"https://{FIXTURE_HOST}/{name}",
                   "html_path": None, "workdir": workdir,
                   "out_pdf": os.path.join(workdir, "render.pdf"),
                   "dry_run": False, "captured": "2026-10-01",
                   "overrides": {"title": None, "author": None,
                                 "published": None}}
            req.update(kw)
            response, debug = run_capture(req, route_hook=hook)
            return response, debug, workdir

        # Full article pipeline, including the placeholder render.
        response, debug, workdir = _capture("article_basic.html")
        check("article ok", response.get("ok") is True)
        check("article kind", response.get("kind") == "article")
        check("article title", response.get("title")
              == "An open-source spec for orchestration: Symphony")
        check("article author", response.get("author")
              == "Alex Kotliarskyi, Victor Zhu, and Zach Brock")
        check("article published", response.get("published") == "2026-04-27")
        check("article published source",
              response.get("metadata_sources", {}).get("published")
              == "visible-date")
        check("script data-island dates ignored",
              "2020-01-01" not in (debug.get("facts", {})
                                   .get("visibleDates", [])))
        check("article author source",
              response.get("metadata_sources", {}).get("author") == "byline")
        check("article headless mode",
              response.get("capture", {}).get("mode") == "headless")
        check("article words kept",
              response.get("word_count", 0) >= 300)
        check("article fidelity",
              response.get("fidelity", {}).get("status") in ("ok", "warn"))
        snapshot_html = debug.get("snapshot_html", "")
        check("picture resolves to the light variant",
              "Desktop-Light" in snapshot_html
              and "Desktop-Dark" not in snapshot_html)
        sanitized_out = debug.get("sanitized_html", "")
        check("soft hyphens stripped", "\u00ad" not in sanitized_out)
        check("word joiners stripped", "\u2060" not in sanitized_out)
        check("zero-width spaces stripped", "\u200b" not in sanitized_out)
        check("extraction dropped nav junk",
              "Cookie preferences" not in sanitized_out)
        pdf_bytes = response.get("pdf_bytes") or 0
        check("placeholder pdf written", pdf_bytes > 1000)
        with open(os.path.join(workdir, "render.pdf"), "rb") as handle:
            head = handle.read(5)
        check("placeholder pdf header", head == b"%PDF-")

        # Challenge and login fixtures fail closed with headed disabled.
        import tempfile as _tf

        def _capture_fails(name: str, kind: str) -> bool:
            workdir = _tf.mkdtemp(prefix="bob-clip-test-")
            req = {"protocol": 1, "op": "capture",
                   "url": f"https://{FIXTURE_HOST}/{name}",
                   "html_path": None, "workdir": workdir,
                   "out_pdf": os.path.join(workdir, "render.pdf"),
                   "dry_run": False, "captured": "2026-10-01",
                   "overrides": {"title": None, "author": None,
                                 "published": None}}
            try:
                run_capture(req, allow_headed=False, route_hook=hook)
            except AdapterFail as exc:
                return exc.kind == kind
            return False

        check("challenge blocked headless",
              _capture_fails("challenge_cloudflare.html", "blocked"))
        check("login wall blocked", _capture_fails("login_wall.html", "blocked"))

        # A challenge page served as HTTP 403 must reach the challenge
        # fallback (blocked), not fail fast as a network error.
        hook_403 = _fixture_hook_factory(
            fixtures_dir, {"challenge_403.html": 403})
        workdir_403 = tempfile.mkdtemp(prefix="bob-clip-test-")
        req_403 = {"protocol": 1, "op": "capture",
                   "url": f"https://{FIXTURE_HOST}/challenge_403.html",
                   "html_path": None, "workdir": workdir_403,
                   "out_pdf": os.path.join(workdir_403, "render.pdf"),
                   "dry_run": False, "captured": "2026-10-01",
                   "overrides": {"title": None, "author": None,
                                 "published": None}}
        try:
            run_capture(req_403, allow_headed=False, route_hook=hook_403)
            check("challenge 403 reaches fallback", False)
        except AdapterFail as exc:
            check("challenge 403 reaches fallback", exc.kind == "blocked")

        # Hostile fixture: hostile markup never survives.
        hostile_resp, hostile_debug, _hostile_workdir = _capture("hostile.html")
        hostile_out = hostile_debug.get("sanitized_html", "")
        check("hostile scripts gone (browser)", "<script" not in hostile_out)
        check("hostile handlers gone (browser)", "onerror" not in hostile_out)
        check("hostile iframes gone (browser)", "<iframe" not in hostile_out)
        check("hostile js urls gone (browser)", "javascript:" not in hostile_out)
        _ = hostile_resp

        # Headed launch smoke check where Xvfb exists: the private DISPLAY
        # must reach the browser even though the driver started earlier.
        if headed_capability() == "xvfb":
            from playwright.sync_api import sync_playwright as _sync_pw

            with _sync_pw() as _pw:
                _headed = None
                _proc = None
                try:
                    _headed, _mode, _proc = _launch_headed(_pw, browser, [])
                    _page = _headed.new_page()
                    _page.goto("about:blank")
                    _page.close()
                    check("headed xvfb launch", _mode == "headed-xvfb")
                except Exception as exc:  # noqa: BLE001 - recorded below
                    log(f"headed smoke check failed: {exc}")
                    check("headed xvfb launch", False)
                finally:
                    if _headed is not None:
                        try:
                            _headed.close()
                        except Exception:  # noqa: BLE001 - best effort
                            pass
                    if _proc is not None:
                        _stop_xvfb(_proc)
        else:
            print("self-test: skipped headed check (no Xvfb)")

    if failures:
        for failure in failures:
            print(f"self-test failure: {failure}", file=sys.stderr)
        return 1
    print("ok")
    return 0


def main(argv: list[str]) -> int:
    """Read one request from stdin, write one response to stdout."""
    if "--self-test" in argv[1:]:
        return self_test()
    _maybe_start_parent_watchdog()
    try:
        raw = sys.stdin.read()
    except Exception as exc:  # noqa: BLE001 - stdin is best effort
        print(json.dumps(error_response(
            "capture", "internal", f"could not read stdin: {exc}")))
        return 0
    try:
        data = json.loads(raw)
    except ValueError as exc:
        print(json.dumps(error_response(
            "capture", "internal",
            f"adapter request was not valid JSON: {exc}")))
        return 0
    try:
        response = dispatch(validate_request(data))
        print(json.dumps(response))
        return 0
    except AdapterFail as fail:
        op = data.get("op") if isinstance(data, dict) else "capture"
        if not isinstance(op, str) or op not in OPS:
            op = "capture"
        print(json.dumps(error_response(op, fail.kind, fail.message,
                                        fail.hint)))
        return 0
    except Exception as exc:  # noqa: BLE001 - unexpected: traceback + ok:false
        sys.stderr.write(traceback.format_exc())
        sys.stderr.write("\n")
        op = data.get("op") if isinstance(data, dict) else "capture"
        if not isinstance(op, str) or op not in OPS:
            op = "capture"
        print(json.dumps(error_response(op, "internal", f"{type(exc).__name__}: {exc}")))
        return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

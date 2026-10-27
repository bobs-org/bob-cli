"""Render batches of Mermaid sources to fitted, selectable vector PDFs."""

from __future__ import annotations

import base64
import math
import os
import re
import shutil
import subprocess
from typing import Any

WEB_CLIP_DIR = os.path.dirname(os.path.abspath(__file__))
# Byte-identical payloads moved to the `bob-obsidian-assets` crate. Installed
# copies live next to this renderer; direct source execution falls back to
# the crate's repository locations.
ASSETS_CRATE_DIR = os.path.normpath(
    os.path.join(WEB_CLIP_DIR, "..", "..", "crates", "bob-obsidian-assets"))
ASSETS_VENDOR_DIR = os.path.join(
    ASSETS_CRATE_DIR, "scripts", "web_clip", "vendor")
ASSETS_FONTS_DIR = os.path.join(
    ASSETS_CRATE_DIR, "scripts", "web_clip", "fonts")


def _first_existing(*paths: str) -> str:
    """Return the first existing path, or the installed candidate."""
    for path in paths:
        if os.path.isfile(path) or os.path.isdir(path):
            return path
    return paths[0] if paths else ""


MERMAID_JS_PATH = _first_existing(
    os.path.join(WEB_CLIP_DIR, "vendor", "mermaid.min.js"),
    os.path.join(ASSETS_VENDOR_DIR, "mermaid.min.js"),
)
DIAGRAM_FONT_PATH = _first_existing(
    os.path.join(WEB_CLIP_DIR, "fonts", "DejaVuSans.ttf"),
    os.path.join(ASSETS_FONTS_DIR, "DejaVuSans.ttf"),
)
MAX_DIAGRAM_ITEMS = 200
DIAGRAM_ID_RE = re.compile(r"^[0-9a-f]{40}$")

# Mermaid 12.2.0's defaults, extended to protect the renderer-owned theme.
MERMAID_SECURE_KEYS = [
    "secure",
    "securityLevel",
    "startOnLoad",
    "maxTextSize",
    "suppressErrorRendering",
    "maxEdges",
    "theme",
    "themeCSS",
    "themeVariables",
    "fontFamily",
]

# Every diagram-family section in Mermaid 12.2.0's MermaidConfig extends
# BaseDiagramConfig and accepts useMaxWidth. Keep this list in sync with the
# pinned bundle's dist/config.type.d.ts when bumping Mermaid.
DIAGRAM_CONFIG_FAMILIES = (
    "flowchart",
    "swimlane",
    "agentflow",
    "sequence",
    "gantt",
    "journey",
    "timeline",
    "class",
    "state",
    "er",
    "pie",
    "quadrantChart",
    "xyChart",
    "requirement",
    "architecture",
    "mindmap",
    "ishikawa",
    "kanban",
    "gitGraph",
    "c4",
    "sankey",
    "packet",
    "block",
    "eventmodeling",
    "treeView",
    "radar",
    "usecase",
    "venn",
    "wardley-beta",
    "cynefin",
    "railroad",
)


class DiagramRequestError(ValueError):
    """The request does not match the diagrams protocol."""


class DiagramRenderError(RuntimeError):
    """The browser could not set up the pinned diagram renderer."""


def validate_request(request: object) -> dict[str, Any]:
    """Validate a complete protocol-1 diagrams request."""
    if not isinstance(request, dict):
        raise DiagramRequestError("diagrams request must be an object")
    required = {"protocol", "op", "workdir", "items", "item_timeout_s"}
    if set(request) != required:
        unknown = sorted(set(request) - required)
        missing = sorted(required - set(request))
        detail = []
        if unknown:
            detail.append(f"unknown fields: {', '.join(unknown)}")
        if missing:
            detail.append(f"missing fields: {', '.join(missing)}")
        raise DiagramRequestError("invalid diagrams request shape (" + "; ".join(detail) + ")")
    if request["protocol"] != 1 or request["op"] != "diagrams":
        raise DiagramRequestError("diagrams request needs protocol 1 and op 'diagrams'")

    workdir = request["workdir"]
    if not isinstance(workdir, str) or not workdir or not os.path.isabs(workdir):
        raise DiagramRequestError("workdir must be an absolute path")

    items = request["items"]
    if not isinstance(items, list) or not items:
        raise DiagramRequestError("items must be a non-empty array")
    if len(items) > MAX_DIAGRAM_ITEMS:
        raise DiagramRequestError(
            f"items cannot contain more than {MAX_DIAGRAM_ITEMS} diagrams"
        )
    for index, item in enumerate(items):
        if not isinstance(item, dict) or set(item) != {"id", "source"}:
            raise DiagramRequestError(
                f"items[{index}] must contain exactly id and source"
            )
        if not isinstance(item["id"], str) or not DIAGRAM_ID_RE.fullmatch(item["id"]):
            raise DiagramRequestError(
                f"items[{index}].id must be a lowercase 40-character SHA-1 hex string"
            )
        if not isinstance(item["source"], str):
            raise DiagramRequestError(f"items[{index}].source must be a string")

    timeout = request["item_timeout_s"]
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)):
        raise DiagramRequestError("item_timeout_s must be a positive number")
    if not math.isfinite(timeout) or timeout <= 0:
        raise DiagramRequestError("item_timeout_s must be a positive number")
    return request


def _first_line_error(error: object) -> str:
    """Return a compact, stable error string for one diagram."""
    message = str(error).splitlines()[0].strip() if str(error) else "render failed"
    return message[:200].rstrip()


def _new_page(context, mermaid_js: str, font_b64: str):
    """Build a fresh offline page and fail closed if the static font is absent."""
    page = context.new_page()
    try:
        page.set_content(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>"
            "@font-face{font-family:'BobDiagram';"
            f"src:url(data:font/ttf;base64,{font_b64}) format('truetype');"
            "font-style:normal;font-weight:400;}"
            "html,body{margin:0;padding:0;background:white}"
            "</style></head><body></body></html>"
        )
        page.add_script_tag(content=mermaid_js)
    except Exception as exc:  # noqa: BLE001 - converted to an op-level failure
        try:
            page.close()
        except Exception:  # noqa: BLE001 - best effort
            pass
        raise DiagramRenderError(f"could not load Mermaid assets: {exc}") from exc

    config = {
        "startOnLoad": False,
        "securityLevel": "strict",
        "theme": "neutral",
        "fontFamily": "BobDiagram",
        "themeVariables": {
            "fontFamily": "BobDiagram",
            "noteBkgColor": "#f8f9fa",
            "noteTextColor": "#24292e",
            "noteBorderColor": "#9ca3af",
            "dropShadow": "none",
        },
        "themeCSS": "* { filter: none !important; }",
        "suppressErrorRendering": True,
        "secure": MERMAID_SECURE_KEYS,
        **{name: {"useMaxWidth": False} for name in DIAGRAM_CONFIG_FAMILIES},
    }
    try:
        font_loaded = page.evaluate(
            "async (config) => {"
            "await document.fonts.load('16px BobDiagram');"
            "if (!document.fonts.check('16px BobDiagram')) return false;"
            "mermaid.initialize(config);"
            "return true;"
            "}",
            config,
        )
    except Exception as exc:  # noqa: BLE001 - converted to an op-level failure
        try:
            page.close()
        except Exception:  # noqa: BLE001 - best effort
            pass
        raise DiagramRenderError(f"could not initialize Mermaid page: {exc}") from exc
    if not font_loaded:
        page.close()
        raise DiagramRenderError("the bundled DejaVu Sans font did not load")
    return page


def _render_one(
    page, item: dict[str, str], index: int, timeout_s: float, workdir: str
) -> dict:
    """Render one source, fit its PDF to the SVG, and validate the output."""
    page.evaluate("() => { document.body.replaceChildren(); }")
    dimensions = page.evaluate(
        "async ([id, source, timeoutMs]) => {"
        "document.body.replaceChildren();"
        "const render = mermaid.render(id, source);"
        "const timeout = new Promise((_, reject) => setTimeout(() => "
        "reject(new Error('diagram render timed out')), timeoutMs));"
        "const result = await Promise.race([render, timeout]);"
        "const host = document.createElement('div');"
        "host.style.display = 'inline-block';"
        "host.innerHTML = result.svg;"
        "document.body.appendChild(host);"
        "await document.fonts.ready;"
        "const rect = host.getBoundingClientRect();"
        "const width = Math.ceil(rect.width);"
        "const height = Math.ceil(rect.height);"
        "if (!(width > 0 && height > 0)) throw new Error('diagram has no visible size');"
        "return {width, height};"
        "}",
        [f"bob-d{index}", item["source"], int(timeout_s * 1000)],
    )
    output = os.path.abspath(os.path.join(workdir, f"{item['id']}.pdf"))
    page.pdf(
        path=output,
        width=f"{dimensions['width']}px",
        height=f"{dimensions['height'] + 1}px",
        print_background=True,
        page_ranges="1",
        margin={"top": "0px", "right": "0px", "bottom": "0px", "left": "0px"},
    )
    with open(output, "rb") as handle:
        header = handle.read(5)
    if header != b"%PDF-" or os.path.getsize(output) <= 5:
        raise RuntimeError("Chrome did not write a non-empty PDF")
    page.evaluate("() => document.body.replaceChildren()")
    return {
        "id": item["id"],
        "ok": True,
        "pdf": output,
        "width_pt": round(dimensions["width"] * 0.75, 2),
        "height_pt": round((dimensions["height"] + 1) * 0.75, 2),
    }


def render(request: dict[str, Any], browser, browser_info: dict) -> tuple[dict, list[str]]:
    """Render all request items in one offline browser context.

    The returned request list is test instrumentation; no request is allowed
    through the context route, and it is not part of the wire response.
    """
    validate_request(request)
    workdir = request["workdir"]
    os.makedirs(workdir, mode=0o700, exist_ok=True)
    try:
        os.chmod(workdir, 0o700)
        with open(MERMAID_JS_PATH, encoding="utf-8") as handle:
            mermaid_js = handle.read()
        with open(DIAGRAM_FONT_PATH, "rb") as handle:
            font_b64 = base64.b64encode(handle.read()).decode("ascii")
    except OSError as exc:
        raise DiagramRenderError(f"diagram renderer asset is unavailable: {exc}") from exc

    attempted_requests: list[str] = []
    context = browser.new_context(
        offline=True,
        service_workers="block",
        accept_downloads=False,
        color_scheme="light",
    )

    def abort(route) -> None:
        attempted_requests.append(route.request.url)
        route.abort()

    context.route("**/*", abort)
    page = None
    results: list[dict] = []
    try:
        page = _new_page(context, mermaid_js, font_b64)
        page._bob_diagram_workdir = workdir
        for index, item in enumerate(request["items"]):
            try:
                results.append(_render_one(
                    page, item, index, request["item_timeout_s"], workdir
                ))
            except Exception as exc:  # noqa: BLE001 - failures are isolated per item
                error = _first_line_error(exc)
                results.append({"id": item["id"], "ok": False, "error": error})
                try:
                    failed_pdf = os.path.join(workdir, f"{item['id']}.pdf")
                    if os.path.exists(failed_pdf):
                        os.unlink(failed_pdf)
                except OSError:
                    pass
                if "diagram render timed out" in str(exc).lower():
                    try:
                        page.close()
                    except Exception:  # noqa: BLE001 - best effort
                        pass
                    page = _new_page(context, mermaid_js, font_b64)
                    page._bob_diagram_workdir = workdir
                else:
                    try:
                        page.evaluate("() => document.body.replaceChildren()")
                    except Exception:  # noqa: BLE001 - rebuild a damaged page
                        try:
                            page.close()
                        except Exception:  # noqa: BLE001 - best effort
                            pass
                        page = _new_page(context, mermaid_js, font_b64)
    finally:
        if page is not None:
            try:
                page.close()
            except Exception:  # noqa: BLE001 - best effort
                pass
        context.close()

    return (
        {
            "protocol": 1,
            "ok": True,
            "op": "diagrams",
            "mermaid": "12.2.0",
            "browser": {
                "kind": browser_info["kind"],
                "path": browser_info["path"],
                "version": browser_info["version"],
            },
            "results": results,
            "warnings": [],
        },
        attempted_requests,
    )


def pdf_page_count(path: str) -> int | None:
    """Return a PDF's page count using pdfinfo, then a conservative byte check."""
    if (pdfinfo := shutil.which("pdfinfo")):
        try:
            proc = subprocess.run(
                [pdfinfo, path], capture_output=True, text=True, timeout=10
            )
            match = re.search(r"^Pages:\s+(\d+)\s*$", proc.stdout, re.MULTILINE)
            if proc.returncode == 0 and match:
                return int(match.group(1))
        except (OSError, subprocess.SubprocessError):
            pass
    try:
        with open(path, "rb") as handle:
            content = handle.read()
    except OSError:
        return None
    pages = re.findall(rb"/Type\s*/Page\b", content)
    return len(pages) if pages else None

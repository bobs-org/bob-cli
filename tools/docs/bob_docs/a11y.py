"""Playwright + axe accessibility audit for the built docs site (dev only)."""

from __future__ import annotations

import functools
import http.server
import json
import threading
from datetime import datetime, timezone
from pathlib import Path

from .errors import DocsError

WIDTHS = (360, 768, 1280)
SCHEMES = ("light", "dark")

# Default audit matrix: homepage, short contract, long contract, plus the
# generated CLI reference when a later phase lands it.
PAGES: tuple[tuple[str, str], ...] = (
    ("homepage", "/"),
    ("today-marks", "/today-marks/"),
    ("capture", "/capture/"),
    ("cli-reference", "/reference/cli/"),
)

CAPTURE_DEEP_ANCHOR = "/capture/"


def _require_playwright() -> None:
    try:
        import playwright  # noqa: F401
    except ImportError as error:
        raise DocsError(
            "missing a11y dependencies (playwright); install browsers with "
            "`uv run --project tools/docs --group a11y playwright install chromium` "
            "and rerun `just docs-a11y`"
        ) from error


def _read_builder(site: Path) -> str:
    manifest_path = site / "build.json"
    if not site.is_dir() or not manifest_path.is_file():
        raise DocsError(
            f"built site is missing at {site}; run `just docs-build` "
            "(or `just docs-build-material` for the Material fallback) first"
        )
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise DocsError(f"cannot read built-site manifest {manifest_path}: {error}") from error
    builder = manifest.get("builder", "zensical") if isinstance(manifest, dict) else "zensical"
    return str(builder)


def _serve(site: Path) -> tuple[http.server.ThreadingHTTPServer, str]:
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(site))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, f"http://127.0.0.1:{port}"


FIXTURE_MARKDOWN = """```bob-task
- [ ] #task Ready row #prj ^ready
- [*] #task Next row [created:: 2024-02-29] ^next
- [/] #task In progress row [scheduled:: 2026-10-21] ^prog
- [?] #task Blocked row [scheduled:: 2026-10-09] ^blocked
- [x] #task Done row [completion:: 2026-10-19] ^done
- [-] #task Cancelled row [cancelled:: 2026-10-18] ^cancel
```

```bob-task today
- [*] #task Today row ^today
```

Press <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>M</kbd> to move a task.

```text { .no-copy title="Output" }
bob plan - Tue 2026-10-20 - no daily note yet
```

> [!NOTE]
> A note callout.

> [!TIP]
> A tip callout.

> [!IMPORTANT]
> An important callout.

> [!WARNING]
> A warning callout.

> [!CAUTION]
> A caution callout.

```mermaid
flowchart LR
  A["Capture"] --> B["Plan"]
```
"""


def _fixture_html() -> str:
    import markdown

    from .fences import fence_format, fence_validator

    md = markdown.Markdown(
        extensions=["pymdownx.superfences", "pymdownx.highlight", "admonition", "github-callouts"],
        extension_configs={
            "pymdownx.superfences": {
                "custom_fences": [
                    {
                        "name": "bob-task",
                        "class": "bob-task",
                        "format": fence_format,
                        "validator": fence_validator,
                    }
                ]
            }
        },
    )
    body = md.convert(FIXTURE_MARKDOWN)
    return (
        "<!doctype html><html lang='en'><head><meta charset='utf-8'>"
        "<title>a11y fixture</title>"
        "<link rel='stylesheet' href='/stylesheets/bob.css'></head>"
        f"<body><main>{body}</main></body></html>"
    )


def run_a11y(root: Path, *, builder: str | None = None, site: str | Path | None = None) -> int:
    _require_playwright()
    from playwright.sync_api import sync_playwright

    repo = Path(root).resolve()
    site_dir = Path(site) if site else repo / "target/docs/site"
    recorded_builder = _read_builder(site_dir)
    if builder and builder != recorded_builder:
        print(f"a11y: auditing existing {recorded_builder} site (requested {builder})")
    builder_name = recorded_builder
    out_dir = repo / "target/docs/a11y" / builder_name
    out_dir.mkdir(parents=True, exist_ok=True)

    vendor_axe = repo / "tools/docs/vendor/axe.min.js"
    if not vendor_axe.is_file():
        raise DocsError("vendored axe bundle is missing at tools/docs/vendor/axe.min.js")
    axe_source = vendor_axe.read_text(encoding="utf-8")

    # Scratch fixture so checks never depend on later phases adding examples.
    fixture_path = site_dir / "__a11y_fixture.html"
    fixture_path.write_text(_fixture_html(), encoding="utf-8")
    remove_fixture = True

    server, base = _serve(site_dir)
    report: dict = {
        "builder": builder_name,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "pages": [],
        "violations": [],
    }
    failures: list[str] = []
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch()
            try:
                # Resolve the audit matrix against the built site.
                matrix: list[tuple[str, str]] = []
                for key, path in PAGES:
                    if key == "cli-reference" and not (site_dir / "reference/cli/index.html").exists():
                        print("a11y: CLI reference page does not yet exist (later generated phase)")
                        report["pages"].append({"page": key, "status": "missing-later-phase"})
                        continue
                    page_file = site_dir / path.lstrip("/") / "index.html"
                    if key == "homepage":
                        page_file = site_dir / "index.html"
                    if not page_file.is_file():
                        print(f"a11y: page {key} ({path}) is missing from the built site")
                        report["pages"].append({"page": key, "status": "missing"})
                        continue
                    matrix.append((key, path))
                # Always audit the scratch fixture.
                matrix.append(("fixture", "/__a11y_fixture.html"))

                for key, path in matrix:
                    url = base + path
                    for width in WIDTHS:
                        for scheme in SCHEMES:
                            context = browser.new_context(
                                viewport={"width": width, "height": 800},
                                color_scheme="dark" if scheme == "dark" else "light",
                                java_script_enabled=True,
                            )
                            page = context.new_page()
                            try:
                                page.goto(url, wait_until="networkidle", timeout=30000)
                                # Long capture: viewport shots at top and a deep section.
                                if key == "capture":
                                    shot_top = out_dir / f"{key}-{width}-{scheme}-top.png"
                                    page.screenshot(path=str(shot_top))
                                    page.evaluate("window.scrollTo(0, document.body.scrollHeight / 2)")
                                    page.wait_for_timeout(400)
                                    shot_deep = out_dir / f"{key}-{width}-{scheme}-deep.png"
                                    page.screenshot(path=str(shot_deep))
                                else:
                                    shot = out_dir / f"{key}-{width}-{scheme}.png"
                                    page.screenshot(path=str(shot))
                                # Inject the vendored axe bundle locally; never a CDN.
                                page.evaluate(axe_source)
                                result = page.evaluate(
                                    "axe.run({resultTypes: ['violations']}).then(r => JSON.stringify(r))"
                                )
                                data = json.loads(result)
                                for violation in data.get("violations", []):
                                    impact = violation.get("impact", "")
                                    if impact in ("serious", "critical"):
                                        nodes = [
                                            node.get("target") for node in violation.get("nodes", [])
                                        ]
                                        failures.append(
                                            f"{key} {width}px {scheme}: {violation.get('id')} "
                                            f"({impact}) selectors={nodes}"
                                        )
                                        report["violations"].append(
                                            {
                                                "page": key,
                                                "width": width,
                                                "scheme": scheme,
                                                "id": violation.get("id"),
                                                "impact": impact,
                                                "selectors": nodes,
                                                "help": violation.get("help"),
                                            }
                                        )
                                # External font requests: local fonts only.
                                requests = []
                                page.on("request", lambda r: requests.append(r.url))
                                page.reload(wait_until="networkidle", timeout=30000)
                                external_fonts = [
                                    u
                                    for u in requests
                                    if "fonts.googleapis.com" in u or "fonts.gstatic.com" in u
                                ]
                                if external_fonts:
                                    failures.append(f"{key}: external font requests {external_fonts}")
                                report["pages"].append(
                                    {"page": key, "width": width, "scheme": scheme, "status": "ok"}
                                )
                            finally:
                                context.close()

                # Reflow at 200% zoom on a 360px viewport (CSS zoom, not DPR).
                context = browser.new_context(
                    viewport={"width": 360, "height": 800}, java_script_enabled=True
                )
                page = context.new_page()
                try:
                    page.goto(base + "/", wait_until="networkidle", timeout=30000)
                    page.evaluate("document.body.style.zoom = '200%'")
                    page.wait_for_timeout(300)
                    overflow = page.evaluate(
                        "(() => { const de = document.documentElement; return "
                        "{scrollWidth: de.scrollWidth, clientWidth: de.clientWidth}; })()"
                    )
                    if overflow["scrollWidth"] > overflow["clientWidth"] + 2:
                        failures.append(
                            f"reflow: page scrolls horizontally at 200% zoom "
                            f"({overflow['scrollWidth']} > {overflow['clientWidth']})"
                        )
                    report["reflow_200"] = overflow
                finally:
                    context.close()

                # Keyboard-only: skip link, nav, palette, search, content.
                context = browser.new_context(viewport={"width": 1280, "height": 800})
                page = context.new_page()
                try:
                    page.goto(base + "/", wait_until="networkidle", timeout=30000)
                    page.keyboard.press("Tab")
                    focused = page.evaluate("document.activeElement ? document.activeElement.outerHTML.slice(0,200) : ''")
                    visible = page.evaluate(
                        "(() => { const el = document.activeElement; if (!el) return false; "
                        "const r = el.getBoundingClientRect(); const s = getComputedStyle(el); "
                        "return r.width > 0 && r.height > 0 && s.visibility !== 'hidden'; })()"
                    )
                    if not visible:
                        failures.append("keyboard: no visible focus after Tab from top")
                    report["keyboard_focus"] = {"focused_html": focused, "visible": visible}
                    # Palette toggle exists (light/dark).
                    palette = page.evaluate("!!document.querySelector('[data-md-component=palette], [data-md-toggle=palette]')")
                    report["keyboard_palette"] = bool(palette)
                finally:
                    context.close()

                # No-JS context: readable core content and working ordinary links.
                nojs = browser.new_context(java_script_enabled=False)
                nojs_page = nojs.new_page()
                try:
                    nojs_page.goto(base + "/", wait_until="domcontentloaded", timeout=30000)
                    text = nojs_page.evaluate("document.body.innerText || ''")
                    if "Capture a thought" not in text:
                        failures.append("no-js: homepage core content is not readable without JS")
                    hrefs = nojs_page.evaluate(
                        "[...document.querySelectorAll('main a[href]')].slice(0,5).map(a => a.getAttribute('href'))"
                    )
                    report["nojs"] = {"links_sample": hrefs}
                finally:
                    nojs.close()
            finally:
                browser.close()
    finally:
        server.shutdown()
        server.server_close()
        if remove_fixture:
            try:
                fixture_path.unlink()
            except OSError:
                pass

    report_path = out_dir / "report.json"
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"a11y: report {report_path.relative_to(repo)}")
    if failures:
        print("a11y: FAILED with serious/critical findings:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(f"a11y: passed ({builder_name}; {len(matrix)} pages x {len(WIDTHS)} widths x {len(SCHEMES)} schemes)")
    return 0

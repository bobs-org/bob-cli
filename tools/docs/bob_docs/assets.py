"""Deterministic Bob identity assets: mark, favicons, social card."""

from __future__ import annotations

from datetime import date
from pathlib import Path

from .errors import DocsError

PAPER = (251, 248, 242)  # #FBF8F2
INK = (23, 28, 26)  # #171C1A
TEAL = (10, 147, 145)  # #0A9391 identity glyph
TEAL_DARK = (15, 107, 99)  # links/active tab light
MUTED = (92, 95, 90)

MARK_SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18" width="{size}" height="{size}" role="img" aria-label="Bob mark">
  <g fill="none" stroke="{color}" stroke-width="1.5" stroke-linecap="round">
    <line x1="4.25" y1="3.25" x2="4.25" y2="10"/>
    <circle cx="9" cy="10" r="4.75"/>
  </g>
  <circle cx="9" cy="10" r="1.6" fill="{color}"/>
</svg>
"""


def _mark_svg(*, size: int = 18, color: str = "#0A9391") -> str:
    return MARK_SVG.format(size=size, color=color) + "\n"


def _draw_mark(draw, *, scale: float, ox: float, oy: float, color, width: float) -> None:
    # Vertical stroke from (4.25, 3.25) to (4.25, 10) in 18x18 space.
    x = ox + 4.25 * scale
    draw.line([(x, oy + 3.25 * scale), (x, oy + 10 * scale)], fill=color, width=max(1, int(round(width * scale))))
    # Stroked circle centered at (9, 10) radius 4.75.
    cx, cy, r = ox + 9 * scale, oy + 10 * scale, 4.75 * scale
    draw.ellipse(
        [(cx - r, cy - r), (cx + r, cy + r)],
        outline=color,
        width=max(1, int(round(width * scale))),
    )
    # Filled center circle radius 1.6.
    cr = 1.6 * scale
    draw.ellipse([(cx - cr, cy - cr), (cx + cr, cy + cr)], fill=color)


def _require_pillow() -> None:
    try:
        import PIL  # noqa: F401
    except ImportError as error:
        raise DocsError(
            "missing generation dependencies (Pillow); "
            "run with `uv run --project tools/docs --group fonts bob-docs social-card`"
        ) from error


def _render_favicon_png(path: Path) -> None:
    from PIL import Image, ImageDraw

    size = 32
    img = Image.new("RGBA", (size, size), PAPER + (255,))
    draw = ImageDraw.Draw(img)
    scale = size / 18.0
    _draw_mark(draw, scale=scale, ox=0.0, oy=0.0, color=TEAL + (255,), width=1.5)
    img.save(str(path), format="PNG", optimize=False, compress_level=9)


def _render_social_card(path: Path, repo: Path) -> None:
    from PIL import Image, ImageDraw, ImageFont

    width, height = 1200, 630
    img = Image.new("RGB", (width, height), PAPER)
    draw = ImageDraw.Draw(img)
    # Mark at left.
    mark_scale = 9.0  # 18 * 9 = 162px
    _draw_mark(draw, scale=mark_scale, ox=96.0, oy=234.0, color=TEAL, width=1.5)
    # Copy: short Bob copy only.
    title = "Bob"
    subtitle = "Capture a thought. Choose today's work."
    sub2 = "Keep an honest record."
    # Bundled fonts; fall back to default bitmap font if unavailable.
    try:
        title_font = ImageFont.truetype(
            str(repo / "assets/print_fonts/InterDisplay-SemiBold.otf"), 96
        )
        body_font = ImageFont.truetype(
            str(repo / "assets/print_fonts/Inter-Regular.otf"), 44
        )
    except Exception:
        title_font = ImageFont.load_default()
        body_font = ImageFont.load_default()
    draw.text((300, 220), title, fill=INK, font=title_font)
    draw.text((300, 340), subtitle, fill=INK, font=body_font)
    draw.text((300, 400), sub2, fill=MUTED, font=body_font)
    # Teal rule under the copy.
    draw.line([(300, 480), (1020, 480)], fill=TEAL_DARK, width=6)
    img.save(str(path), format="PNG", optimize=False, compress_level=9)


def generate_assets(root: Path, *, check: bool = False) -> int:
    _require_pillow()
    repo = Path(root).resolve()
    assets = repo / "docs/assets"
    expected = {
        assets / "bob-mark.svg": _mark_svg(size=18, color="#0A9391"),
        assets / "favicon.svg": _mark_svg(size=32, color="#0A9391"),
    }
    import tempfile

    with tempfile.TemporaryDirectory(prefix="bob-assets-") as tmp:
        work = Path(tmp)
        for path, desired in expected.items():
            (work / path.name).write_text(desired, encoding="utf-8")
        _render_favicon_png(work / "favicon.png")
        _render_social_card(work / "social-card.png", repo)
        if check:
            drift: list[str] = []
            for name in ("bob-mark.svg", "favicon.svg", "favicon.png", "social-card.png"):
                target = assets / name
                candidate = work / name
                if not target.is_file() or target.read_bytes() != candidate.read_bytes():
                    drift.append(f"docs/assets/{name}")
            if drift:
                print(f"assets: drift in {', '.join(sorted(drift))} (regenerate with just docs-assets)")
                return 1
            print("assets: check passed (regeneration is byte-stable)")
            return 0
        assets.mkdir(parents=True, exist_ok=True)
        for name in ("bob-mark.svg", "favicon.svg", "favicon.png", "social-card.png"):
            (assets / name).write_bytes((work / name).read_bytes())
    # Report sizes for completion evidence.
    for name in ("bob-mark.svg", "favicon.svg", "favicon.png", "social-card.png"):
        size = (assets / name).stat().st_size
        print(f"assets: docs/assets/{name} {size} bytes")
    print(f"assets: media date {date.today().isoformat()}")
    return 0

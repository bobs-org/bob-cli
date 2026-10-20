"""Subset bundled OFL fonts to deterministic WOFF2 for the docs site."""

from __future__ import annotations

import shutil
from pathlib import Path

from .errors import DocsError

# Fixed timestamp for deterministic font binaries (2024-01-01T00:00:00Z).
FIXED_TIMESTAMP = 1704067200

# Explicit Unicode selection: Latin/Latin-1, punctuation, arrows, box
# drawing, plus U+2022 and U+2317. A read-only scan of the bundled cmaps
# found no U+2317, so the mono derivative adds a docs-only hash outline.
SUBSET_UNICODES = (
    "U+0020-007E,"  # Basic Latin
    "U+00A0-00FF,"  # Latin-1 Supplement
    "U+2010-205E,"  # General punctuation (includes U+2022 bullet)
    "U+2190-21FF,"  # Arrows
    "U+2500-257F,"  # Box drawing
    "U+2022,"  # Bullet (explicit)
    "U+2317"  # View-data / hash placeholder (docs-only derivative)
)

FONTS_OUT = Path("docs/assets/fonts")

# (output name, source file, family label)
# Compact set: Inter body/UI with emphasis, Inter Display headings, and the
# Bob Mono derivative for code. The derivative replaces the upstream mono
# faces so the WOFF2 total stays under budget.
FACES: tuple[tuple[str, str, str], ...] = (
    ("Inter-Regular.woff2", "Inter-Regular.otf", "Inter"),
    ("Inter-Italic.woff2", "Inter-Italic.otf", "Inter"),
    ("InterDisplay-SemiBold.woff2", "InterDisplay-SemiBold.otf", "Inter Display"),
)

# Mono derivative carries the docs-only U+2317 hash outline.
DERIVATIVE_OUT = "BobMono-Regular.woff2"
DERIVATIVE_SOURCE = "JetBrainsMono-Regular.ttf"
DERIVATIVE_FAMILY = "Bob Mono"

BUDGET_BYTES = 450_000


def _require_fonts_group() -> None:
    try:
        import fontTools  # noqa: F401
        import brotli  # noqa: F401
    except ImportError as error:
        raise DocsError(
            "missing generation dependencies (fonttools, brotli); "
            "run with `uv run --project tools/docs --group fonts bob-docs fonts`"
        ) from error


def _unicodes_arg() -> list[str]:
    return [f"--unicodes={SUBSET_UNICODES}"]


def _subset_one(src: Path, dst: Path) -> None:
    from fontTools import subset

    options = subset.Options()
    options.flavor = "woff2"
    options.with_zopfli = False
    # Deterministic output: drop hints names that embed timestamps where
    # possible and keep layout features stable.
    options.hinting = True
    options.desubroutinize = False
    options.name_IDs = ["*"]
    options.glyph_names = False
    options.recalc_bounds = True
    options.recalc_timestamp = False
    options.prune_unicode = True
    options.prune_name = True
    font = subset.load_font(str(src), options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=subset.parse_unicodes(SUBSET_UNICODES))
    subsetter.subset(font)
    # Fixed modification timestamp for byte-stability.
    if "head" in font:
        font["head"].modified = FIXED_TIMESTAMP
    if "maxp" in font:
        pass
    font.save(str(dst))


def _ensure_derivative(src_dir: Path, work_dir: Path) -> Path:
    """Build the Bob Mono derivative with a docs-only U+2317 outline.

    Copies the U+0023 (#) outline to U+2317 so the subsetter has a real,
    nonempty glyph. The task-tag contract geometry is a slanted hash;
    JetBrains Mono's # is the closest available outline.
    """

    from fontTools.ttLib import TTFont
    import copy

    src = src_dir / DERIVATIVE_SOURCE
    work = work_dir / "BobMono-Regular.ttf"
    font = TTFont(str(src))
    cmap = font.getBestCmap()
    if 0x2317 in cmap:
        # Source already has the glyph; still rename the family.
        pass
    else:
        if 0x23 not in cmap:
            raise DocsError(f"{src}: cannot derive U+2317, U+0023 is missing")
        base_name = cmap[0x23]
        glyph_order = font.getGlyphOrder()
        new_name = "uni2317"
        if new_name in glyph_order:
            raise DocsError(f"{src}: unexpected existing glyph {new_name}")
        glyph_set = font.getGlyphSet()
        base_glyph = glyph_set[base_name]
        # Copy outlines via the glyf table for TTF sources.
        if "glyf" not in font:
            raise DocsError(f"{src}: no glyf table for derivative")
        glyf = font["glyf"]
        hmtx = font["hmtx"]
        base = glyf[base_name]
        new_glyph = copy.deepcopy(base)
        glyf.glyphs[new_name] = new_glyph
        glyph_order.append(new_name)
        font.setGlyphOrder(glyph_order)
        hmtx[new_name] = hmtx[base_name]
        # Map U+2317 to the new glyph in every cmap subtable that has U+0023.
        for table in font["cmap"].tables:
            if table.isUnicode() and 0x23 in table.cmap:
                table.cmap[0x2317] = new_name
        # loca/maxp recalculated on save.
    # Bob-specific family name, preserving all other name records.
    name_table = font["name"]
    for record in name_table.names:
        try:
            text = record.toUnicode()
        except UnicodeError:
            continue
        replaced = None
        if "JetBrains Mono" in text:
            replaced = text.replace("JetBrains Mono", DERIVATIVE_FAMILY)
        elif "JetBrainsMono" in text and record.nameID == 6:
            replaced = text.replace("JetBrainsMono", "BobMono")
        if replaced is not None:
            record.string = replaced.encode(record.getEncoding())
    # Fixed timestamp before subsetting.
    if "head" in font:
        font["head"].modified = FIXED_TIMESTAMP
    work.parent.mkdir(parents=True, exist_ok=True)
    font.save(str(work))
    return work


def _verify_cmap(path: Path) -> None:
    from fontTools.ttLib import TTFont

    font = TTFont(str(path))
    cmap = font.getBestCmap()
    for code in (0x2022, 0x2317):
        if code not in cmap:
            raise DocsError(f"{path.name}: missing U+{code:04X} in cmap")
    # Nonempty outline for the derivative hash.
    glyph_name = cmap[0x2317]
    glyph_set = font.getGlyphSet()
    glyph = glyph_set[glyph_name]
    try:
        # Draw to a bounds pen; None bounds means empty.
        from fontTools.pens.boundsPen import BoundsPen

        pen = BoundsPen(glyph_set)
        glyph.draw(pen)
        bounds = pen.bounds
    except Exception as error:
        raise DocsError(f"{path.name}: cannot verify U+2317 outline: {error}") from error
    if bounds is None:
        raise DocsError(f"{path.name}: U+2317 outline is empty")
    width = bounds[2] - bounds[0]
    height = bounds[3] - bounds[1]
    if width <= 0 or height <= 0:
        raise DocsError(f"{path.name}: U+2317 outline has zero area")


def generate_fonts(root: Path, *, check: bool = False) -> int:
    _require_fonts_group()
    repo = Path(root).resolve()
    src_dir = repo / "assets/print_fonts"
    out_dir = repo / FONTS_OUT
    for _, src_name, _ in FACES:
        if not (src_dir / src_name).is_file():
            raise DocsError(f"missing pinned source font: assets/print_fonts/{src_name}")
    import tempfile

    with tempfile.TemporaryDirectory(prefix="bob-fonts-") as tmp:
        work_dir = Path(tmp)
        # Subset the pinned faces in sorted order for determinism.
        for out_name, src_name, _ in sorted(FACES):
            _subset_one(src_dir / src_name, work_dir / out_name)
        # Mono derivative with docs-only U+2317.
        derived_src = _ensure_derivative(src_dir, work_dir)
        _subset_one(derived_src, work_dir / DERIVATIVE_OUT)
        # Verify coverage on the derivative before publishing.
        _verify_cmap(work_dir / DERIVATIVE_OUT)
        expected_ofl = (src_dir / "OFL.txt").read_bytes()
        if check:
            drift: list[str] = []
            for path in sorted(work_dir.glob("*.woff2")):
                target = out_dir / path.name
                if not target.is_file() or target.read_bytes() != path.read_bytes():
                    drift.append(path.name)
            ofl_target = out_dir / "OFL.txt"
            if not ofl_target.is_file() or ofl_target.read_bytes() != expected_ofl:
                drift.append("OFL.txt")
            if drift:
                print(f"fonts: drift in generated files: {', '.join(sorted(drift))}")
                print("fonts: regenerate with `just docs-fonts`")
                return 1
            print("fonts: check passed (regeneration is byte-stable)")
            return 0
        out_dir.mkdir(parents=True, exist_ok=True)
        # Publish in sorted order.
        for path in sorted(work_dir.glob("*.woff2")):
            target = out_dir / path.name
            target.write_bytes(path.read_bytes())
        # Ship the upstream OFL notices beside the generated fonts.
        shutil.copyfile(src_dir / "OFL.txt", out_dir / "OFL.txt")
    total = 0
    sizes: list[tuple[str, int]] = []
    for path in sorted(out_dir.glob("*.woff2")):
        size = path.stat().st_size
        total += size
        sizes.append((path.name, size))
    for name, size in sizes:
        print(f"fonts: {name} {size} bytes")
    print(f"fonts: total {total} bytes (budget {BUDGET_BYTES})")
    if total >= BUDGET_BYTES:
        print(f"fonts: error: WOFF2 payload exceeds {BUDGET_BYTES} bytes")
        return 1
    # Representative coverage report: punctuation, arrows, box drawing.
    print("fonts: coverage U+2022 present, U+2317 present in BobMono derivative;")
    print("fonts: arrows U+2190-21FF and box drawing U+2500-257F retained where present in sources")
    return 0

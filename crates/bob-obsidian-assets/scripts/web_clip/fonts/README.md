# Bundled reader fonts (web-clip print template)

Body faces are the static OTF/TTF print fonts from
[`assets/print_fonts/`](../../../assets/print_fonts/): Source Serif 4 Text
(Regular, Italic, Semibold, Semibold Italic), Inter (Regular, Italic,
Medium, SemiBold, SemiBold Italic), and JetBrains Mono (Regular, Bold).
The Bob binary embeds each file once (`src/print_fonts.rs`); the web-clip
installer copies the reader subset next to this adapter. At print time the
renderer converts CFF OpenType faces to glyf TrueType so Chromium embeds
CID TrueType instead of Type 3. Variable `*-wght-*.woff2` faces are not
shipped: Chromium embedded those as Type 3 and PDFKit then split
Highlights quote words.

Hashes, upstream pins, and the 6 MB print-font budget live in
[`assets/print_fonts/README.md`](../../../assets/print_fonts/README.md).
License: OFL-1.1 for the three families (`OFL.txt` here; the same notices
are in `assets/print_fonts/OFL.txt`).

## Mermaid diagram font

`DejaVuSans.ttf` is the static DejaVu Sans Book face used by Mermaid diagrams.
It is embedded as a data URL so Chrome produces selectable TrueType text in the
diagram PDFs. Its separate Bitstream Vera / DejaVu license is in
`DEJAVU_LICENSE.txt`.

| File | SHA-256 |
| ---- | ------- |
| `DejaVuSans.ttf` | `57f73e11f51999432bf7ab22ce55b6f945d5eca1bf824404cfa9ec2e3718c84e` |

# highlights-ref Fixtures

Fixtures in this directory are intentionally small and synthetic. Integration
tests generate PDF annotation fixtures at runtime with `lopdf`, while
`sample_sidecar.md` documents the simple Markdown sidecar shape,
`linked_sidecar.md` documents the linked-page Highlights export shape,
`quote_heading_sidecar.md` documents a linked-page quote that hard-wraps a
`# ` line with no blank line, `color_grouped_quote_sidecar.md` documents a
color-grouped quote whose wrap lines alternate between `>` and bare lines,
and `image_sidecar.md` documents the TextBundle
image selection shape supported by the parser.

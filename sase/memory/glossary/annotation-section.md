---
keyword: Annotation Section
aliases:
  - section tag
  - section card
---

A named group of Highlights annotations inside a [[reference-note]]'s managed
`## Highlights` region. A `# Name` line declares it: the first line of a sticky note
(its later lines become the section's description) or of a highlight, underline, or
image comment (that annotation joins the section). Trailing `@alias` or `#alias`
tokens add match keys. An annotation joins by carrying `@name` or `#name` as a whole
token at the start or end of a line. Matching ignores case and punctuation, and a
matched tag is consumed, so a tag that still shows did not group. Mid-sentence tokens,
`#task` lines, and `\` escapes never join. The space matters: `# Memory` declares,
`#memory` joins. Each displayed block has one home, and a comment declaration beats
tags. Each section renders as `### Name`, then a `[!summary|section]` card captioned
`@tag · N highlights · Pages`, its `^s-<slug>` block, and its members under `####`
page captions, ahead of Other highlights and Removed highlights. `bob ref show` and
the Mac Refs panel group the same way. Membership is re-derived from the sidecar on
every scan and nothing new is stored. Tag edits never change `^h-` IDs, and a PDF with
no declaration renders unchanged. Not a Markdown `##` heading or a `bob capture`
target section. See `docs/highlights-ref-sync.md`.

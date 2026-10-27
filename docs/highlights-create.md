# `bob ref create` — vault notes, Markdown, PDF, and URL targets

`bob ref create <TARGET>` turns a vault note, Markdown, a local PDF, a PDF URL,
an arXiv paper URL, or a web article URL into a Highlights-ready reference.
By default, create files the PDF in `lib/`, writes the `ref/` note and reading
task, commits those paths together, and pushes. If intake is kept or cannot
run safely, create leaves the PDF in `xlib/` with a queue reason for
`bob ref scan -i` or the host's local timer to drain later. Vault notes become
projected reading snapshots; other Markdown uses pandoc, a PDF is stamped as
is, and web article URLs run through the web-article engine.

```bash
bob ref create [OPTIONS] <TARGET>...
```

## Examples

```bash
# Markdown through pandoc (default ref type chat)
bob ref create report.md -P sase

# Local PDF, stamped as-is (default ref type papers)
bob ref create paper.pdf -t papers -P sase

# PDF URL with an explicit stem
bob ref create https://example.com/paper.pdf -N my_paper -P sase

# arXiv paper, preview only
bob ref create https://arxiv.org/abs/1706.03762 -d -P bob

# Web article through the web-article engine (default ref type blogs)
bob ref create https://example.com/article/hello -P sase

# Overrides
bob ref create paper.pdf -T "The Real Title" -N my_paper -t docs -P sase
bob ref create report.md --audio episode.mp3 --include-id -P sase

# Keep the PDF queued for a later bob ref scan
bob ref create report.md -k -P sase

# Web article with saved page and metadata overrides
bob ref create https://example.com/essay -H saved.html -P sase
bob ref create https://example.com/essay -A "Jane Doe" -p 2026-01-02 -d -P sase

# Merge a reading packet into one reference and PDF (2–20 web articles)
bob ref create https://example.com/2024/01/tokio-internals-part-1 \
  https://example.com/2024/02/tokio-internals-part-2 -L -P sase
bob ref create URL1 URL2 URL3 -N spotify_bg_agents -P sase
```

## Create-time intake

Create-time intake is enabled by default. Use `-k, --keep-intake` to leave a
new PDF in `xlib/` for a later `bob ref scan -i` or local timer. The same
behavior can be selected with
`highlights.ingest_on_create: false`; `BOB_HIGHLIGHTS_INGEST_ON_CREATE` accepts
`0`, `false`, `no`, or `off` to disable intake and `1`, `true`, `yes`, or `on`
to enable it. A nonblank environment value overrides the config value.

Filed `-f overwrite` requires intake to be enabled because overwrite cannot
queue a replacement. With intake disabled, or with a `-b` vault that differs
from `BOB_DIR` (the vault its pre- and post-sync use), it refuses with
`intake_unavailable` before fetching or rendering, including in dry runs.
Enable `highlights.ingest_on_create` or set
`BOB_HIGHLIGHTS_INGEST_ON_CREATE=1` on this host, and point `BOB_DIR` at the
vault you pass with `-b`.

When enabled, create takes `bob_sync.lock` and then `scan.lock`, pulls the
vault, rechecks the library destinations, moves the PDF and its companions,
writes the reference note and reading task, commits only those paths, and
pushes. It skips the pull for a PDF created directly in `lib/`. If the vault
is unreachable, a lock is busy, or a collision or planning refusal occurs,
create exits successfully and reports `queued: ... (<reason>) · next: bob ref
scan`; the PDF stays in `xlib/`. `--dry-run` prints the intended intake line
without taking locks or syncing.

## Vault note targets

A target may be an exact vault-relative route, a unique case-insensitive note
basename, a simple `[[route|alias]]` wikilink, or an existing in-vault Markdown
path. Existing filesystem paths keep precedence. Dot-directories, `_conflicts/`,
`xlib/`, `lib/`, `lit_review/`, and shared excluded note directories are not
snapshot notes. An excluded Markdown path keeps the regular Markdown route.

For a vault note, the default parent is the note itself if it is an eligible
root area or non-terminal project. Otherwise create follows its `parent:` links
for up to eight hops and chooses the nearest eligible area or project. If that
walk finds none, pass `-P/--parent`. Other target types still require `-P`.
Explicit parents use the existing `bob capture-targets` eligibility and
resolution rules (see [`highlights-ref-sync.md`](highlights-ref-sync.md)).

The default filename is `<note-stem>_<YYMMDD>.pdf`; `BOB_NOW` controls the date
in tests. `-N/--name` overrides the dated stem, while `-o/--output` chooses the
complete PDF path. The identity title comes from `-T`, frontmatter `title`, the
first H1 outside fences and comments, then a humanized filename stem. The
selected H1 is removed from the projected body only when it supplied the title.
Without `-T`, the snapshot date is appended to the identity title. `-T` is the
exact identity title.

By default, closed tasks are summarized as omitted and managed schedule/work
logs are condensed. `-C/--closed` restores closed tasks in an appendix and full
logs. Blocked `[?]` tasks with open dependencies, a `#prj_is_blocked` project,
or no future date stay in a Waiting on lane; tasks blocked only by a
`scheduled` date after the snapshot day become compact Scheduled rows grouped
This week, This month, and Later. `-M/--plain` uses the existing plain
Markdown renderer, while keeping the vault-note parent, naming, title, and
`source_note` provenance. `-C` and `-M` only accept one vault-note target
and conflict with each other.
Repeat `-S/--section HEADING` to keep only named projected sections; names match
case-insensitively, and the dry-run section summary reflects the selection.
Dry run lists earlier snapshots of the same note newest-first as
`earlier: <stem> (<status>, N highlights)`. Both dry run and a real create print
`hint: earlier snapshot <stem> is still unread (0 highlights)` for each earlier
snapshot that is still `ready` with 0 highlights. Create never changes, closes,
or abandons those notes.

Status glyphs are open ○, next ◉, in progress ◐, blocked ⊘, done ✓, and
canceled ⊗. Chips show priority, `sched`, `repeats`, `ref`, and `moved N×`
metadata, along with `Keep` and `quote` links where available.

Note snapshot and research-report Markdown PDFs use a clean text layer: ragged
right, no hyphenation, `Ligatures=NoCommon` for serif, sans, and mono fonts,
and no Type 3 body fonts. Snapshots render through pandoc and XeLaTeX because
the PDFKit measurements found that Chromium/Skia with variable fonts damages
some extracted words.

A second snapshot of the same note on the same day refuses with `already
queued` (use `-f` to replace it) or `already captured` (`-f` rereads it as
`<dated-root>__vN`). Pass `-N` for a deliberate second snapshot under a
different name that day; a new-day snapshot is an ordinary dated create that
needs no force. Older plain-Markdown renders of a note
stay where they are when note snapshots are created.

Embedded PNG and JPEG images are copied into the private render scratch as-is
when their long edge is at most 1600 pixels; larger images are downscaled with
a high-quality filter. GIF embeds use the first frame, and WebP embeds are
converted to PNG. SVG and unreadable images become an “Image not printable”
placeholder with a warning. Dry runs report the image count, downscaled paths,
and unprintable paths.

Dry runs show the selected title, parent, destination, projection summary, and
warnings without running pandoc, xelatex, or listen; they end with `writes:
none`. Set `BOB_HIGHLIGHTS_KEEP_WORKDIR=1` to retain projected Markdown, TeX,
logs, fonts, and staged assets for debugging. `-L/--listen` passes the rendered
`projected.md` as `{target}` and the snapshot PDF as `{pdf}`. In `-M` mode,
`{target}` is the original Markdown.

```bash
bob ref create harbor_cranes
bob ref create '[[harbor_cranes]]' -d
bob ref create zk/some_idea -N idea_261016
bob ref create harbor_cranes -C
bob ref create harbor_cranes -S Requirements -S Outline
bob ref create harbor_cranes -M
bob ref create harbor_cranes -L
```

## Other single targets

Markdown outside the vault, local PDFs, and URLs continue to require
`-P/--parent`: the area or project note that owns the reading task. A missing
`-P` fails before rendering, fetching, or writing with a hint naming
`bob capture-targets`. Markdown files render ragged-right, with hyphenation
off and `Ligatures=NoCommon` on the DejaVu serif, sans, and mono families, so
Highlights quotes stay free of hyphen splits and common ligatures.
This covers plain Markdown files and `-M` vault-note renders. Sections are
numbered from the file's top section level. A lone H1 prints as an unnumbered
title heading and stays out of the Contents list and the bookmarks, so its H2s
number `1`, `2`, …. A file with no H1 numbers from its shallowest heading.
Files with two or more H1s number those H1s. A PDF with no listed heading skips
the Contents list.
See [web article capture](highlights-clip.md) for reader-mode capture, headed
retries, the `--html` escape hatch, and environment variables. `bob ref clip`
is a hidden compatibility alias for `create` with the same targets and options.

## Several URLs

Two to twenty web-article URLs passed to one `create` invocation become one
reference and one PDF. The URLs stay in argument order. The collection has one
reading task when `create` files it, one annotation space, and one optional listen
episode. Use it for material you'll read as one unit; capture an ongoing series
part by part under the same `-P`. A collection is a snapshot: `create` does not
add parts to an existing collection.

```bash
bob ref create URL1 URL2 -P sase
bob ref create URL1 URL2 -N my_reading_packet -P sase
bob ref create URL1 URL2 -o ~/bob/xlib/blogs/my_reading_packet.pdf -P sase
bob ref create URL1 URL2 -T "Tokio Internals" -P sase
```

Options can appear before or after the URLs. One target still uses the existing
single-target route. Several targets must all be live HTTP(S) web articles:
local files, arXiv papers, PDFs, and `-H/--html` replay are not supported in a
collection. A collection accepts at most 20 URLs. Duplicate URLs and any
pre-existing PDF-backed or queued reference that overlaps one of the parts make
the whole command fail. Legacy notes without a Highlights PDF produce a warning
and do not block capture. Captures are staged privately; a failed part leaves
the vault unchanged and reports its `part N/M` position.

### Naming

The collection name is resolved before any network request, in this order:

1. `-N/--name`.
2. The filename stem from `-o/--output` (the option remains an exact PDF path).
3. Shared slug words, then a qualifying shared folder, as described below.
4. `snake_case` of `-T/--title`.
5. Otherwise the command exits 2 with the URLs and a hint to pass `-N` or `-T`.

With several URLs and no `-N`, bob names the reference after the words every
URL's slug starts with, or failing that the folder they all share, ignoring
dates, ids, part numbers, and filler. It needs two meaningful words, or one
when the pages differ only by a part or page number. Otherwise pass `-N` (or
`-T`). The derived name is validated and follows the usual `_2`, `_3` collision
walk. The selected source is shown in normal and dry-run output.

The rule is intentionally strict. URLs must have the same site (host compared
case-insensitively, with `www.` ignored). Each URL must have a usable slug: the
last path segment after skipping short, numeric, date, and known noise
segments. Slugs are split into lowercase words; bob takes the longest shared
word prefix and trims filler, series terms (`part`, `chapter`, `episode`,
`page`, and related forms), numbers, and hex ids from its ends. It accepts at
least two meaningful words when one is specific rather than generic. One
remaining word of at least four letters is accepted only when the URLs differ
by a part/page number. If the slug rule fails, a shared multi-word folder can
qualify by the same two-word rule; a one-word folder never names a collection.
This prevents common paths such as `core` or `configuration` from becoming
misleading names.

Examples: `tokio-internals-part-1` and `tokio-internals-part-2` derive
`tokio_internals`; `/raft-part-1` and `/raft-part-2` derive `raft`; and
`/core/configuration` plus `/core/usage` require an explicit name because
`core` is too generic. A Spotify set whose slugs share no meaningful start also
requires `-N` or `-T`. If every URL has a part/page number and their numeric
order goes backwards, bob warns but keeps the supplied argument order.

### Collection document and metadata

The PDF opens with a collection title, byline/metadata, and linked contents.
Every article starts on a fresh page with its own masthead and part label. The
PDF has one outline bookmark per article, continuous page numbering, and
part-prefixed fragment ids and assets so repeated names do not collide. Relative
links resolve against each article's URL. The `source_url` marker value is an
ordered list; after scanning, the single reference note retains every URL, and
`bob ref find` can locate the collection from any part.

`-T` overrides the collection title. Otherwise bob uses the longest shared
article-title prefix at a word boundary, or the humanized collection name when
there is no useful shared title. An author shared by every part is kept; when
they differ, bob formats the distinct authors together. The published date is
the earliest date available, and `-A` / `-p` override the collection values.
Each part retains its own source and article metadata in the PDF.

`--dry-run` still loads each web page in the browser but skips composition and
installation. `-L/--listen` narrates the staged collection PDF as one episode.
See [the listen contract](#listen) below.

## Prerequisites

Dependencies depend on the target; basic vault capture does not require the
full reference-import toolchain.

| Target | Required tools |
| --- | --- |
| Local PDF | No external renderer; Bob stamps the PDF directly |
| PDF URL or arXiv | `curl` for downloads |
| Web article | `curl` for initial URL classification (skipped with `--html`), `uv` for the pinned Python adapter, and a browser: Google Chrome at `/opt/google/chrome/chrome`, `/usr/bin/google-chrome`, `/usr/bin/google-chrome-stable`, or the macOS Google Chrome app, then Playwright's bundled Chromium. Set `BOB_CHROME` for any other executable, including a system Chromium package. Headed retries use Xvfb on Linux and an off-screen window on macOS |
| Markdown | `pandoc`, `xelatex`, the LaTeX packages below, and DejaVu Serif, DejaVu Sans, and DejaVu Sans Mono (`mainfont`, `sansfont`, and `monofont`) |
| Markdown with Mermaid | `uv` and a discovered browser are optional; without them Mermaid blocks print as source with a visible note |
| Any target with `--listen` | A configured `highlights.listen_command` (`sase listen render ...`) that writes the requested MP3. Installing Bob does not install a narration frontend: the SASE plugin (`sase plugin install listen`) provides `sase listen` |

Run `bob ref doctor` to inspect the current host. Its `pandoc`, `xelatex`, and
`latex_packages` rows diagnose Markdown rendering. A missing renderer or
package is a warning rather than a fatal doctor error because other target
types can still work. The package check runs `kpsewhich` from beside
`xelatex` when that copy exists, otherwise `kpsewhich` on `PATH`. The status
row names the missing files (`latex_packages: warn (missing …)`). The
warnings section prints one combined `tlmgr install …` line for every missing
package; run that command with the TeX Live installation that provides your
`xelatex`. A later render failure can also hint at a single missing package.

Bob's headers require `fvextra`, `lineno`, `upquote`, `needspace`, and `pgf`
(`tikz.sty`). The note snapshot print profile additionally requires
`titlesec`, `enumitem`, `fancyhdr`, `lastpage`, `refcount`, and `xurl`.
Doctor also checks the packages used by pandoc's LaTeX template, including
table and image support. Doctor does not check whether the DejaVu fonts are
installed. If XeLaTeX cannot find DejaVu Serif, DejaVu Sans, or DejaVu Sans
Mono, install them for the host's font system before retrying. Note snapshots
bundle their own fonts (`Source Serif 4`, `Inter`, `JetBrains Mono`, Noto
Emoji) and write them to the scratch `fonts/` directory, so no system-font
install is needed for that route.

### Mermaid diagrams

`bob ref create` renders explicit Mermaid code blocks as fitted vector figures
on both Markdown routes: ordinary Markdown imports and vault-note snapshots,
including `-M, --plain`. A block must be tagged as Mermaid: a backtick fence
labelled `mermaid`, a tilde fence labelled `mermaid`, or a backtick fence with
`{.mermaid}`; fences nested in lists and blockquotes
are supported. Bob does not guess from untagged `flowchart` or `graph` source,
so use a `text` fence when you want Mermaid code to remain visible as source.
PDF, PDF URL, arXiv, and web-article imports are unchanged.

Rendering uses the pinned, vendored Mermaid 12.2.0 bundle and DejaVu Sans
font through one offline browser launch per document. A block-level parse or
render failure keeps the source below an italic note and does not fail the
import. If `uv`, the browser, or the adapter is unavailable, every Mermaid
block remains as source with a note and Bob prints one warning. For example:

```text
diagrams: rendered 3 of 4 Mermaid diagrams
warning: Mermaid diagram 4 of 4 not rendered: Parse error on line 2: …
```

Figures scale down to fit the text width and at most 80% of the text
height. That leaves room on the same page for the heading, return links, and
opening caption line, so a tall diagram no longer leaves a blank page or runs
into the margin. Figures are still never rearranged or split, and short
figures print at natural size. When a figure would print below 50% of its
natural size, Bob warns that its labels may be small; that estimate uses the
same 80% height budget as the printed scale. The diagrams remain vector text;
no raster images are used.

The renderer does not backfill existing library PDFs. Recreate an individual
reference manually when you want its existing PDF replaced. Set
`BOB_HIGHLIGHTS_DIAGRAMS=off` to skip diagram rendering while debugging; the
dry-run still counts the candidate fences and says rendering is disabled.

To bump Mermaid, replace `scripts/web_clip/vendor/mermaid.min.js`, update its
version and SHA-256 in `web_clip_adapter.py` and `vendor/README.md`, then run
`just check-web-clip-adapter` and the gated live Mermaid render test. Keep the
bundle self-contained and verify the vector-font and no-raster checks before
updating the pin. Other diagram languages, SVG image embeds, a listen-skip
marker, persistent caching, landscape pages, and Mermaid accessibility-title
metadata are outside this version's scope.

### macOS BasicTeX: installing a missing package in user mode

BasicTeX ships a small package set, so a render can fail with
`! LaTeX Error: File '<name>.sty' not found` even when pandoc and XeLaTeX
are present (verified 2026-10-10: `needspace.sty` missing from BasicTeX /
TeX Live 2026). Use `kpsewhich`, `xelatex`, and `tlmgr` from the same
installation; on that host all three live under `/Library/TeX/texbin`,
while the system tree (`/usr/local/texlive/2026basic`) is root-owned.

For a missing package such as `needspace`, initialize the
default user tree once and install without administrator access, as the
same user that runs the file hook:

```sh
tex_home=$(/Library/TeX/texbin/kpsewhich -var-value=TEXMFHOME)
if ! test -f "$tex_home/tlpkg/texlive.tlpdb"; then
    /Library/TeX/texbin/tlmgr init-usertree
fi
/Library/TeX/texbin/tlmgr --usermode install needspace
/Library/TeX/texbin/kpsewhich needspace.sty
```

The default user tree (`~/Library/texmf`) is already on kpsewhich's
search path. User-tree packages are maintained separately from the system
installation, so specify `--usermode` on future package operations too.
Verify with the `latex_packages` doctor row (`ok` with all packages checked)
and then a real render: a doctor exit of zero alone does not establish render
readiness, because missing render dependencies are warnings by design,
and Markdown `--dry-run` returns before pandoc runs.

The `[WARNING] Deprecated: --highlight-style` line is unrelated noise
from pandoc, not the fatal error; the `! LaTeX Error` line names the
actual missing file. See the official
[TeX Live user-mode documentation](https://tug.org/texlive/doc/tlmgr.html#USER-MODE);
the [CTAN needspace entry](https://ctan.org/pkg/needspace) confirms its
TeX Live package name.

`--dry-run` plans the import and skips installing the final PDF and audio.
What it still does depends on the target:

| Target | During `--dry-run` |
| --- | --- |
| Local PDF | Reads the local file and prints the plan. No renderer runs |
| Markdown | Returns before pandoc and XeLaTeX run. When Mermaid fences are present, prints their approximate count; dry-run never starts the adapter or browser |
| PDF URL | Downloads the PDF with `curl`, then prints the plan |
| arXiv | Downloads the API metadata and the PDF, then prints the plan |
| Web article | Launches the browser and loads the page. Image localization and the PDF write are skipped |
| Web article with `--html` | Skips the `curl` classification fetch, launches the browser in html-file mode, and loads the page. Image localization and the PDF write are skipped |

URL targets still use the network during `--dry-run`. Use
`bob capture --dry-run '<URL>'` to preview automatic URL routing from local
state only.

## Targets

| Kind | Route | Default `-t` | Title (first hit wins) | Stem (first hit wins) |
| ---- | ----- | ------------ | ---------------------- | --------------------- |
| Markdown (`.md`) | Rendered with pandoc/XeLaTeX | `chat` | `-T`, frontmatter `title`, first H1, file stem | `-N`, file stem |
| Local PDF (`.pdf` or `%PDF-` magic) | Stamped as-is | `papers` | `-T`, plausible Info `/Title`, humanized stem | `-N`, file stem if it passes `--name` validation, else `snake_case` |
| PDF URL (PDF content-type or sniffed `%PDF-`) | Downloaded with `curl`, stamped as-is | `papers` | `-T`, plausible Info `/Title`, humanized stem | `-N`, URL slug, short-title stem, `<host>_<YYYYMMDD>` |
| arXiv (`arxiv.org` abs/html/pdf) | PDF fetched from `arxiv.org/pdf/<id>`, metadata from the API | `papers` | `-T`, arXiv API title, plausible Info `/Title`, `arXiv <id>` (no version) | `-N`, short-title stem, `arxiv_<id>` (no version) |
| Web article (HTML 2xx or 403/429/503) | Captured with the web-article engine | `blogs` | web-article rules (`-T` overrides the title) | web-article rules (`-N`) |
| Several web articles (2–20 URLs) | Captured in order and composed into one PDF | `blogs` | Shared title prefix or humanized name (`-T` overrides) | Collection naming waterfall above |

Short-title stem: when the text before the first `:` is 1–4 words, that
prefix is the short name (`EA-Graph: …` becomes `ea_graph`); otherwise the
first 6 words stand in, run through `snake_case` (capped at 80 chars).
A trailing `.md`, `.markdown`, or `.txt` is dropped first (case-insensitive),
and the same rule applies to the title-stem fallback.
`Attention Is All You Need` and `Attention Is All You Need.md` both become
`attention_is_all_you_need`. URL-derived stems discard trailing opaque hex IDs
and UUIDs; if the final meaningful URL path segment is itself an opaque ID,
there is no URL stem and the next configured fallback is used.

Plausible Info title: trimmed and whitespace-collapsed, at least 3 chars
with a letter, not equal to the source file stem or URL stem
(case-insensitive), not a bare filename (`.pdf`, `.doc`, `.docx`, `.tex`,
`.dvi`, `.ps`, `.pages`), not an exporter default (`Microsoft Word - …`,
`Microsoft PowerPoint - …`), and not `untitled`/`title`. The same
normalization applies to `/Author`.

## Local links and return pills

Every same-document link (`[words](#some-heading)`) gets a small raised
letter tag beside its words, and its target grows a row of return pills just
underneath. Each pill reads `↩ p. N` plus the matching tag: tapping it jumps
back to the passage the reader left, about a line and a half above the
original sentence, keeping the current zoom and horizontal position. The
heading text, TOC, and PDF bookmarks never change, and before tapping
anything the row also shows which pages cite that section.

Eligibility:

| Link location or target | Result |
| ----------------------- | ------ |
| Paragraphs, plain blocks, line blocks, definition lists, block quotes, Div content, list items (not nav lists), table body cells, figure content, footnotes | Tagged + paired (when the target is return-capable) |
| Headings, figure captions, table captions, table head rows (including intermediate body heads), table foot rows, hand-written TOC lists | Untagged: rewritten to the canonical id, still a working forward link |
| Target is a CodeBlock, Table, Figure, Image, Code, Link, Span in a skip context, or a duplicated id | Untagged: working forward link |
| Listen cards (already raw LaTeX) and metadata (title, abstract) | Untouched |
| Unresolvable target (`#`, missing, or ambiguous alias) | Dead: plain text (a Span of the link content) plus a report record; applies in every context |
| Non-`#` links | Untouched |

Return-capable targets are Headings, Divs, and Spans outside headings,
captions, and table head/foot rows. Each tagged link gets its own sequential
letter in reading order (`a…z` without `l`, `o`, `q`, then `aa, ab, …`), and
each target carries exactly one pill row in source reading order.

Targets resolve in this order: the exact pandoc id, then the percent-decoded
id, then a GitHub-style alias computed by pandoc's own `gfm` reader (only
unique aliases count). Real ids always beat aliases; nothing is fuzzed by
case or heading text. Headings that are linked often are safest with an
explicit `{#id}`, which also fixes ambiguous-alias warnings.

On success bob prints a `links:` line after `pages:` (`links: 27 paired
(11 targets)`, plus `· N via GitHub-style slugs`, `· N untagged`, `· N dead`
when nonzero; `links: none` when there are no `#` links; `links: off
(bob-return-links: false)` for the opt-out below). Each dead link and each
duplicated anchor also prints one `warning:` line on stderr: a missing target
renders as plain text, an ambiguous GitHub-style slug names the fix
(`give the heading an explicit {#id}`), and a duplicated anchor keeps working
forward links but gets no return pills.

Frontmatter `bob-return-links: false` turns the whole feature off for that
document. Internal links, URLs, file links, citations, tags, and pills share
one link ink (`#2F5E96`); the TOC stays black. Existing PDFs gain the feature
only when re-rendered. When a render pairs links, bob stamps
`return_links: true` so `bob ref sync` can keep exports clean. Dry runs
preview the planned marker without `return_links`; the flag is added only
after an actual render pairs a link.

## PDF route

Before any write the PDF must load with `lopdf`, have at least one page,
not be encrypted, and be smaller than 95 MiB (files of 50 MiB or more print
a warning). Encrypted PDFs fail with `save an unencrypted copy and pass
that file`.

When page 1 already carries a standalone `/Text` note, a parseable
Highlights marker means already captured (see below); any other note is
kept and bob's marker is prepended ahead of it, so it stays the first
standalone note.

Stamping loads the PDF, embeds the marker, sets Info Title/Author, saves
to a private scratch `stamped.pdf`, verifies the marker round-trips and
the page count is unchanged, and only then atomically installs the target.
A local source file is never modified.

URL routes stamp `source_url` (cleaned URL, or `https://arxiv.org/abs/<id>`
with version), `author`, `published` (arXiv first-version date), `captured`
(local `YYYY-MM-DD`), and always `id` (the stem). Local PDFs stamp `id`
only with `-i` (with `-N`, `-i` embeds the name).

A local PDF inside `lib/` or `xlib/` with a marker is already captured:
`already captured; bob ref sync <PDF> re-syncs it` plus the
`--listen` attach hint. Without a marker it must be moved out first. A PDF
outside the vault, or a downloaded PDF (PDF URL or arXiv), that already
carries a Highlights marker is refused before any write:
`PDF already carries a Highlights marker: <path>` with a
`bob ref sync` / `create <library PDF> --listen` hint.

## Code and link catalog

Markdown, `-M` vault-note renders, and note snapshots record each code block
from the Markdown AST before Mermaid diagrams are swapped for images. The
catalog keeps the original code text, language and other attributes, section,
and the PDF pages marked by invisible destinations; it does not change the
PDF's visible or selectable text. Mermaid source remains in the catalog when
its rendered block becomes an image.

Web articles record the same `code.json` shape from converted source:
`pandoc -f html -t gfm --wrap=none --preserve-tabs` turns the adapter's
printed article HTML into the saved `source.md`, and the record filters
run over that GFM with the `gfm` reader. Code pages come from Chrome's
print-time named destinations (one `bob-code-N` anchor pair per printed
code block, `page_source: anchor`) instead of TeX destinations; blocks
with no anchor stay pageless but copyable. Link pages come from the
printed PDF's URI annotations (`page_source: annotation`).

The same render records every URI link in document order: written links,
reference-style and titled links, `<…>` autolinks, `mailto:` autolinks, bare
URLs, and linked images (text is the alt text). Internal `#` links, relative
paths, standalone images, URLs inside code, and raw HTML `<a href>` are not
recorded. Each record keeps the exact target pandoc hands to hyperref, the
link text, title, an autolink flag, the lead-in context (last 160 characters
of the enclosing block, cut on a word boundary with a leading `…`), and the
section. An invisible `\BobLinkAnchor{bob-link-N}` sits just before each
anchorable link so copy equals click; links in headings, figure and table
captions, and table header rows are recorded with a null page and no anchor,
since an anchor there would leak into the TOC, bookmarks, or repeated heads.
Footnote links count at their reference position.

Pandoc reads these sources with `--preserve-tabs` plus
`markdown+autolink_bare_uris` for the plain `.md` and `-M` routes and
`commonmark_x-tex_math_dollars-emoji+autolink_bare_uris` for snapshots, so
bare URLs render as clickable `BobLinkInk` links, breakable at slashes,
instead of unbreakable plain text. A render filter expands tabs
to four-column stops in code blocks and inline code, preserving the current
layout while the catalog retains the original tab characters. A tab at the
start of a blockquote code line therefore expands to four columns, where the
older parser could treat it as two.

## Saved Markdown

Every new Markdown-born, single-URL web-article, or multi-URL
web-collection reference keeps the exact cataloged bytes under
`.bob/ref-sources/<ref_type>/<stem>--<hash12>/source.md` plus its
render-time catalog `code.json` (version 1). `hash12` is the first 12 hex
digits of SHA-256(`source.md`); `<stem>` is the final target stem and
`<ref_type>` the library type directory. `source.md` is write-once: a
digest mismatch refuses with a warning and never overwrites. `code.json`
is derived data that a later render of the same bytes may replace
atomically. Writes hold `bob_sync.lock`, and readers treat a missing
`code.json` as `no_catalog`.

| Route | `code.json` route | `source.md` bytes | `source_note` |
| --- | --- | --- | --- |
| Plain `.md` | `markdown` | Input file bytes | — |
| `-M` vault note | `plain_note` | Original note bytes | Note route + digest |
| Snapshot | `note_snapshot` | Projected Markdown bytes | Note route + digest |
| Web article | `web_article` | pandoc GFM of the printed article HTML | — |
| Web collection | `web_collection` | pandoc GFM of the composed collection HTML | — |

`code.json` keeps `"version": 1` with `blocks` and an additive `links` array
(`n`, `url`, `text`, `title`, `autolink`, `context`, `section`, `page`,
`page_source`). A missing `links` array means the catalog predates link
recording; an empty array means no links. Older bobs ignore the new field.

Hidden content such as HTML comments and frontmatter is preserved verbatim
and synced through vault git (`source.md` via `!*.md`, `code.json` via
`!/.bob/ref-sources/**/*.json`). `rename` never moves a store; a reread
makes a new store and an in-place overwrite makes a new store when the
bytes changed, keeping the old one as history. The note frontmatter
`source_markdown` points at `source.md` (for example
`.bob/ref-sources/chat/ace--5d3be944e47e/source.md`). `create` prints
`source_markdown:`, `code:`, and `links:` lines (`links: 14 (github.com,
...)` or `links: none`), and `bob ref show` carries the
pointer in JSON and in its `markdown` row.

When create successfully saves a source, it carries the verified pointer
through inline ingest and checks the planned and persisted note frontmatter
before reporting a completed ingest. A missing or different pointer refuses
or leaves ingest incomplete; create never reports that ingest as successful.
With `-k`, disabled ingest, or a lock/offline queue, the source store can be
saved before the note is born. A later `bob ref scan` derives the pointer from
the birth PDF digest and keeps it on the new note. The pre-sync can commit the
store before the note, so the store and note are not guaranteed to share one
Git commit.

## arXiv

URL recognition mirrors `sase-listen` (`abs`, `html`, `pdf`, new- and
old-style ids, version kept, query/fragment ignored, `.pdf` only on
`/pdf/`). The fetch always goes to `https://arxiv.org/pdf/<id>` and never
falls back to the abstract page. `arxiv:ID` and bare ids are out of scope.

Metadata comes from `https://export.arxiv.org/api/query?id_list=<id>` (20 s
cap): the first `<entry>` title (whitespace collapsed), every
`<author><name>`, and the `<published>` date part. An `api/errors` entry, a
network failure, or a parse failure prints a `warning:` and falls back to
PDF Info metadata. Authors display as `A`, `A and B`, `A, B, and C`, or
`A et al.`.

## Dedupe

Every URL dedupes before any fetch on its syntactic key. arXiv spellings
(abs/html/pdf, with or without version) share
`https://arxiv.org/abs/<id>`; everything else uses the web-article key. Ref notes
record `source_url` and legacy `url:`, and queued intake markers record
both, so existing paper refs take part instead of duplicating. A
duplicate refuses before any fetch, before a missing `-P` error, and
lists every copy:

```text
bob ref: error: already captured: Attention Is All You Need
  ref/papers/attention_is_all_you_need.md · read · 41 highlights · captured 2026-03-02
hint: -f            read it again as attention_is_all_you_need__v2
hint: -L            add --listen to narrate it and attach the episode to the existing capture
hint: -f overwrite  replace ref/papers/attention_is_all_you_need.md in place; authored notes move to ref_history/
```

Queued hits render `already queued: <title>` with `hint: -f  replace
the queued PDF`. Title routes add `hint: -N <name>  capture under a
different name`. A filed single-source family refuses with its copies
newest-first (multi-reading copies carrying their `reading vN` number) plus
`hint: -f read it again as <root>__vN`; bare `-f` on that family reads
it again as the named `<root>__vN` beside the earlier readings instead.
The reread defaults to the newest reading's parent and type (explicit
`-P`/`-t` win, and a reread without any resolvable parent still requires
`-P`), stamps `id` to the new stem even where `-i` is otherwise
optional, never inherits earlier audio, and prints
`reread: <old> → <new> (vN <status> · <M> highlights)` where `renamed:`
would print, in real runs and dry runs alike. `-N` must name a reading
of the same family (a free `-N` stays an ordinary capture) and selects the
reading a later overwrite recaptures in place; `-o` on a
captured reread exits 2. A library PDF as TARGET refuses even with `-f`
(copying highlights into the next reading is forbidden): reread from its
recorded `source_url` or the original file instead, while `-L` without
force still attaches to the newest reading. `-f overwrite` replaces an
eligible filed reference in place; authored prose moves to `ref_history/`.
Incompatible options and annotated references refuse before fetching
(annotated overwrite runs on the Mac; see Recapture), and
without a filed match overwrite exits 1. A URL recorded only by legacy notes without a
Highlights PDF warns (`already in the library as <path>, a note without
a Highlights PDF; capturing a fresh copy`) and captures a fresh copy
instead of refusing; with `--listen` it is a normal capture plus listen,
never attach mode. A dry run adds
`legacy: <path> (superseded by this capture)` to its report.

Import deduplication reads every URL from scalar or list-valued
`source_url`/`url` fields in ref notes and queued intake markers, matching
the URLs indexed by `bob ref find`.

For several URLs, each part is checked against recorded sources before capture.
An order-insensitive exact normalized URL-set match selects one family, even
when several readings share that set: without force it refuses naming the
next stem (`hint: -f read it again as <root>__vN`); bare `-f` reads it again
as the allocated `<root>__vN` beside the earlier readings, with the newest
reading's parent and type as defaults (explicit `-P`/`-t` win), `id` stamped
to the new stem, and a `reread:` line where `renamed:` would print. `-N` must
name a reading of the same family (a free `-N` stays an ordinary capture)
and selects the reading a later overwrite recaptures in place;
`-o` on a captured reread exits 2. Any partial overlap, including a single
URL of a larger collection, refuses under every force mode before adapters
or fetch, naming the overlapping members with
`hint: -f recaptures only the exact URL set of <stem>`. Unrelated duplicate
families sharing the set stay ambiguous: pass `-N <stem>` to choose one.
`-f overwrite` on an exact URL-set match replaces the selected reading in
place, in any argument order; `-N` selects a reading or disambiguates
unrelated families. Defaults come from that selected member. Authored prose
moves to `ref_history/`, companion audio stays unless `-a` or `-L` replaces
it, and `-L` narrates the replacement. `-o`, `-k`, a differing explicit
`-P`, or a differing explicit `-t` refuses before fetching. `--force` applies
to the queued collection only when it has exactly the same URL set, in any
order. A legacy PDF-less note is reported as a warning and
the collection may proceed. The collection marker stores the cleaned URLs in
argument order as a YAML list; create writes that list to one ref note (or a
later `bob ref scan -i` does so when intake is queued). A later create of an
individual part is refused as already captured, and `bob ref find <part URL>`
locates the collection. Queued rereads drain through `bob ref scan` with the
same lineage and task semantics as inline intake. Partial collection overlap
continues to refuse under every force mode before fetching or adapter work.

Default targets dedupe by walking `<stem>`, `<stem>_2`, `<stem>_3`, …
(up to `<stem>_999`): a candidate is occupied when its matching PDF, sidecar,
or note exists in any reference-type directory under `xlib/`, `lib/`, or
`ref/`, or when any note under `ref/` already records that id. A free
candidate is chosen, a candidate occupied by a different reference is
skipped, and a candidate occupied by the same reference refuses through
the same renderer (`already captured: <title>` for library or ref-note
occupants, `already queued: <title>` for intake occupants, each with its
copy rows and `hint:` remedies).
For Markdown and local PDFs the identity is the create-time title (the `-T`
override, else frontmatter/H1/stem or Info title/filename): a candidate is
the same reference when its intake or library PDF carries that title as its
page-1 marker `title` or raw Info `Title` (trimmed, whitespace-collapsed,
case-insensitive). A different source with the same filename but a different
title is suffixed; two different sources with the same stem and title still
refuse (pass `-N`). For URL routes the identity is the source URL dedupe
key (arXiv checks both the paper and URL keys): in practice only the
`--force` same-intake overwrite refuses, everything else suffixes. An
explicit `-o/--output` path keeps the exact path. Whenever the route records
an id, it uses that output PDF's basename as the id; if another note already
uses that id, Bob refuses and asks for a different output filename because the
exact path cannot be collision-suffixed. On default targets, the collision walk
also reserves ids case-insensitively across all of `ref/`.
When the walk skips at least one candidate, dry-run and success output print
`renamed: <base>.pdf is taken by <first occupant>; using <final>.pdf` just
before `pdf:`. Bare `--force` only overwrites the same reference's intake PDF,
never a different reference; `-f overwrite` instead replaces an eligible filed
reference in place (see Recapture). If all 999 candidates are taken, the command
fails with `no free default filename for <stem>; pass -N/--name or -o/--output`.

## Ingest boundary

`src/native/highlights_ref/ingest.rs::ingest_url` is the typed,
non-printing URL ingest shared by the capture background worker and
`bob gkeep pull`. It composes the same building blocks as create
(`resolve_url_syntactic`, `fetch_and_route`, `sources` dedupe,
`pdf_target` planning/stamp/install, `ClipAdapterClient`). It takes a
vault root, a cleaned URL, and the resolved parent route, and returns
`Created`, `AlreadyInLibrary`, or `AlreadyQueued`, or an `IngestError`
with a snake-case `kind` and a `retryable()` flag. Fixed defaults:
route-default ref type (`blogs` for articles, `papers` for PDFs and
arXiv), status `ready`, no audio, no force, no title or name override.
It holds
the machine-wide
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/ref/ingest.lock` (fs2
exclusive, blocking) for the whole call — the lock serializes the
capture worker and Keep pull — and reports
`waiting for another clip…` once through its progress callback while it
waits. `bob ref create` keeps its own printing routes and never calls
`ingest_url`. Ingest uses the same default-target suffixing (`<stem>_2`,
`<stem>_3`, …) with its URL identity and `force = false`. Ingest writes
nothing to stdout and prints nothing to stderr itself.
Error kinds: `network`, `timeout`, `http_status` (retryable only for 408,
429, and 5xx), `browser`, and `dependency` are retryable; `blocked`,
`thin`, `render`, `unsupported_content`, `collision`, `invalid_url`, and
`internal` are permanent. `IngestError::fallback_note(url)` renders the
shared `⚠️ Clip failed (<kind>): <message> · retry: bob ref create <url>`
bullet used by every fallback path.

## Listen

`-L, --listen` (conflicts with `-a` and `-n`) narrates the target with the
configured `highlights.listen_command` and binds the episode as the PDF's
companion audio. For a multi-URL collection, bob passes the staged composed
PDF as `{target}` and `{pdf}`, with the resolved collection title as `{title}`;
the episode binds to the one installed collection PDF. If the same URL set is
already captured or queued, in any order, `--listen` attaches an episode to the
newest reading of that capture. Partial overlap still refuses. With `-f`,
`--listen` narrates the new reading instead of attaching (earlier audio is
never inherited); when the newest reading already has an episode, a hint of
the form `hint: earlier reading vN keeps its episode at <audio path>; the
new reading is narrated fresh` names its reading number and audio path. A
plain reread (no `-L` or `-a`) of a narrated family instead prints
`hint: vN is narrated (<audio path>); add -L to narrate this reading`.

```bash
bob ref create https://arxiv.org/abs/1706.03762 -L
```

The `URL %` shorthand in `bob capture` and `bob gkeep pull` (with `URL @`
kept as a compatible alias) clips like
`bob ref create <URL> -P <parent> -L` (same placeholders, same install
guarantees, same create-time intake). It never adds audio to existing
captures; background ingest runs the command quietly (never stdout) with
progress on stderr.

All or nothing, on every route:

1. The listen command is resolved and validated first: an unconfigured or
   invalid command fails before any fetch or render.
2. The target is resolved, deduped, and planned (see above).
3. The episode destination `<target>.mp3` is planned: an existing mirrored
   library audio refuses.
4. The PDF is produced in private scratch (rendered, downloaded, or
   adapter-captured).
5. A dry run prints the plan with `listen: would-run …` and stops.
6. The `sase listen render` command runs with stdin, stdout, and stderr inherited, so
   the listen plugin's live checklist and its "Published to apollo" summary
   appear unchanged. bob prints `listen: run sase listen render ...` first (shell quoting is display-only).
7. Collisions and the audio plan are re-checked (the vault may have
   changed during a long listen), the audio is installed, then the PDF.
   If the PDF install fails, only an audio file this run created is
   deleted. If this episode or an earlier one is still in the sase-listen
   publish outbox, bob ends with a `warning: podcast feed: …` line and a
   hint.

Placeholder values: `{target}` is the cleaned URL for URL routes (the listen renderer
does its own arXiv rewrite), the absolute source path for a local PDF, and
the staged `<scratch>/<stem>.pdf` render for Markdown (pandoc sets its Info
Title, which the listen renderer reads). `{pdf}` is the unstamped scratch PDF (or
the existing capture in attach mode), `{audio}` is `<scratch>/<stem>.mp3`,
and `{title}` is the resolved title. bob passes every value as a single argument to `sase` itself;
never quote the placeholders. A direct `sase-listen render ...` setting migrates automatically to `sase listen render ...`; wrappers, shell programs, chains, pipelines, redirections, and expansions are rejected. Markdown skips audio discovery under
`--listen`, and the listen card's Play URI targets `<stem>.mp3`.

Success looks like this (arXiv example):

```text
listen: run sase listen render https://arxiv.org/abs/1706.03762 -e full -o /tmp/bob-create-…/attention_is_all_you_need.mp3
  … the listen plugin's live checklist and summary, unchanged …
ok created Highlights-ready PDF
source: https://arxiv.org/abs/1706.03762 (arXiv 1706.03762)
pdf: /home/bryan/bob/xlib/papers/attention_is_all_you_need.pdf
audio: /home/bryan/bob/xlib/papers/attention_is_all_you_need.mp3 (from --listen)
title: Attention Is All You Need
author: Ashish Vaswani et al.
published: 2017-06-12
captured: 2026-10-06
status: ready
parent: sase
id: attention_is_all_you_need
pages: 15 · size: 2.2 MB
ingested: lib/papers/attention_is_all_you_need.pdf → ref/papers/attention_is_all_you_need.md · task → sase.md ^ref-attention-is-all-you-need
sync: committed <sha7> · pushed
```

A failed listen command writes nothing (`nothing was written to the vault;
rerun the same command once the listen error above is fixed`); rerunning is
cheap because the listen renderer resumes from its caches. An interrupted listen
exits 130. Exit 0 with no MP3 at `{audio}` is an error. A failure after the
episode exists keeps the scratch directory, prints `kept: <audio path>`,
and hints at rebinding it (`bob ref create <TARGET> --audio <path>`
in normal mode, `copy it to <xlib dest> for scan to pair` in attach mode).

## Attach

When `--listen` is given and the target is already captured, the new
episode attaches to the newest reading of the existing capture instead of
refusing:

1. A ref-note dedupe hit: the PDF is the note's `source_pdf` (it must
   resolve inside `lib/` and exist, else `ref note has no resolvable
   source_pdf`).
2. An intake dedupe hit (not a `--force` recapture): the queued intake PDF.
3. A local PDF target inside `lib/` or `xlib/` that carries a marker.

The command refuses when the capture already has audio (any allowed
companion beside `lib/<rel>.pdf` or `xlib/<rel>.<ext>`, or an `audio`
field on the ref note: `already has companion audio: <path>`). Otherwise
it runs the listen command with `{target}` as the cleaned URL (or the PDF
path), `{pdf}` as the existing PDF, `{title}` as the note or marker
title, and `{audio}` as `<scratch>/<stem>.mp3`, then installs the episode
at `xlib/<rel>.mp3`. Intake then files that episode:

- Library capture (`lib/<rel>.pdf`): move the mp3 into `lib/`, re-sync
  the existing note so it gains `audio:` frontmatter and the embed, and
  commit the mp3 plus the note (`bob ref create: attach audio …`).
- Queued capture (`xlib/<rel>.pdf`): file the PDF with the mp3 as a
  companion, the same as a normal create.

`-k` (or a disabled ingest switch) keeps today's attach output, including
`next: bob ref scan`; drain the queue with `bob ref scan -i` or its local
timer. A failed intake prints that same next-step line so the queued files
can still be paired.

```text
ok attached listen episode to existing capture
source: https://arxiv.org/abs/2608.04278 (arXiv 2608.04278)
pdf: /home/bryan/bob/lib/papers/ea_graph.pdf (unchanged)
ref: /home/bryan/bob/ref/papers/ea_graph.md
audio: /home/bryan/bob/xlib/papers/ea_graph.mp3 (from --listen)
title: EA-Graph: Artifact-Anchored Verification Memory for Coding Agents under Upstream Drift
ingested: lib/papers/ea_graph.pdf → ref/papers/ea_graph.md
sync: committed abc1234 · pushed
```

## Recapture

`-f [MODE]` / `--force[=MODE]` changes what a repeated capture does. Bare
`-f` means `--force=reread`; an explicit mode needs `=`: `--force=overwrite`
or `-f=overwrite` (a glued `-foverwrite` is not a mode; it parses as
`-f -o verwrite`). A `reread` or `overwrite` word directly after a bare `-f`
is adopted as the mode (`-f overwrite URL` captures URL with overwrite); an
explicit `=` value never adopts a following word. A bare `TARGET` that equals
a mode, or is within edit distance 2 of one, refuses with exit 2: write
`./overwrite` for a file and `[[overwrite]]` for a vault note.

Bare `-f` on an already-captured source reads it again as `<stem>__vN`
beside the earlier readings: the newest reading's parent and type become the
defaults (explicit `-P`/`-t` win, and a reread without any resolvable parent
still requires `-P`), `id` is stamped to the new stem, earlier audio is never
inherited, and a `reread:` line prints where `renamed:` would, in real runs
and dry runs alike:

```text
reread: attention_is_all_you_need → attention_is_all_you_need__v2 (v1 read · 41 highlights)
```

When the newest reading is still `ready` with zero highlights, both the dry
run and the real reread also print
`hint: <stem> is still ready with 0 highlights; -f overwrite would replace it instead`.

Without force the refusal names the next stem
(`hint: -f read it again as <stem>__vN`). Bare `-f` on a fresh source
captures it as usual. `-N` must name a reading of the same family (a free
`-N` stays an ordinary capture) and selects the reading a later overwrite
recaptures in place; `-o` on a captured reread exits 2
(`a reread is named <root>__vN; drop -o`). Every supported single-source
route (Markdown, original local PDF, PDF URL, arXiv, web article with or
without `-H`, same-day vault-note snapshots) creates a complete new
bundle and task on explicit reread, with durable `earlier:` lineage, stable
aliases, and safe naming; old notes, PDFs, sidecars, assets, audio, and
reading task lines keep their bytes. A plain reread (no `-L` or `-a`) of a
narrated family prints `hint: vN is narrated (<audio path>); add -L to
narrate this reading`, naming the newest narrated reading; with `-f -L`
the hint instead names the earlier reading's kept episode. A library PDF
as `TARGET` refuses even
with `-f` (copying highlights into the next reading is forbidden): reread
from its recorded `source_url` or the original file instead, while `-L`
without force still attaches to the newest reading and `-f -L` narrates
the new reading. `-f overwrite` on a filed Markdown file, original local
PDF, same-day vault-note snapshot, or single-URL web capture (article,
saved HTML, PDF URL, arXiv) replaces that reading in place: the note
path, stem, id, and reading task stay, companion audio is preserved
unless `-a` or `-L` (which narrates the replacement) replaces it, and
`--no-audio` leaves an existing episode in place. Markdown-rendered and
web-article overwrites stage, install, and repoint the write-once source
store inside the same transaction; a PDF URL, arXiv, or local PDF
overwrite clears `source_markdown` on the live note. A web overwrite
whose source view or conversion is unavailable keeps that clearing
fallback and warns.

| Overwrite input | Live `source_markdown` | Store |
| --- | --- | --- |
| Markdown, new bytes | Repoints at the new `source.md` | New directory; old directory stays as history |
| Markdown, same bytes | Keeps the existing pointer | Reuses the directory; refreshes `code.json` if it differs |
| Web article/collection, new bytes | Repoints at the new `source.md` | New directory; old directory stays as history |
| Web article/collection, same bytes | Keeps the existing pointer | Reuses the directory; refreshes `code.json` if it differs |
| PDF URL, arXiv, local PDF | Cleared | Existing directory stays as history |
| Annotated Markdown or web | Live note as above | Frozen note keeps the old pointer |
| Rollback before commit | Restored | New directory removed |

If this host still ignores `code.json`, overwrite commits `source.md`,
keeps the catalog on disk, and warns. See
[Reference sources](vault-git-sync.md#reference-sources). `bob ref doctor`
prints a `ref sources` row for dangling pointers, digest mismatches,
missing catalogs, orphans, and an ignored catalog. Clap's default `-s ready`
is not an explicit reopen; pass `-s` to change the kept task. An explicit
open status cannot reopen a task archived in `done/`; reopen it where it lives
first. An orphan can be historical or still pending; preview adoption with
`bob ref attach-source -d`. Bob adopts only an unreferenced saved source that
matches the reference's PDF; pass `-s` to search for a Markdown fallback.
Doctor never attaches or deletes an orphan. Any status overwrite that cannot
stamp is rejected before rendering. Exact-set collection overwrite follows
the same identity, audio, and task-preservation rules; partial overlaps refuse
under every mode. Without a filed match
overwrite exits 1. Annotated references — text highlights, image-only
sidecars, `.assets`, or inbound `#^h-` links — overwrite only on macOS
with `highlights.coordinate_pdf_writes: true`, enabled after the guided
Highlights acceptance run. Other hosts refuse before fetch or adapter
work with `this reference has N highlights, which Highlights holds on
the Mac; run the overwrite there` and
`hint: -f starts a new reading from any host`. With the key off on the
Mac, the refusal names the key and says to enable it only after that
run. A dry run on any host previews the archive path, highlight and
inbound-link counts, and `annotated PDF retained: <path>`, then
`writes: none`. No force is needed for a URL recorded only by legacy
notes without a Highlights PDF (it warns and captures a fresh copy) or
for a new-day vault-note snapshot (an ordinary dated create).

A filed overwrite reports the same plan on dry runs and real runs, ending
dry runs with `writes: none`:

```text
ok replaced Highlights-ready PDF
overwrite: papers/report (ready · 0 highlights)
title: Report
earlier: ref_history/papers/report__2026-10-16.md
task: kept sase.md ^ref-report [ ] ready
audio: kept lib/papers/report.mp3 (narrates the replaced capture; add -L to re-narrate)
sync: committed abc1234
```

An annotated overwrite keeps that shape and expands `earlier:` with the
real highlight count, inbound `#^h-` rewrite count, and retained PDF:

```text
earlier: ref_history/papers/report__2026-10-16.md (3 highlights · 1 highlight link · annotated PDF retained: ref_history/papers/report__2026-10-16__annotated.pdf)
```

The link count covers rewritten links in other vault notes; links inside the
live note are rewritten in its frozen copy and are not reported as inbound
edits. Ambiguous bare targets and dangling highlight anchors stay unchanged
and each produces a `warning:` line naming its file and target.

The old PDF is renamed (not copied) to
`<archive>__annotated.pdf` and assets to `<archive>.assets/` under one
bounded `.forMerging` accessor. The frozen note keeps `^h-`/`^s-` anchors,
records `annotated_pdf` and `annotated_pdf_sha256`, and does not become a
library row, dedupe hit, or reading-task source. Only resolvable inbound
`#^h-` links move onto the frozen note; note links, heading links, `#^s-`
links, and code examples stay on the live note. The reading-task identity,
checkbox, residence, dependencies, relations, freshness, and logs stay
except explicit `-s` and title-alias edits. `[h::]` processed IDs survive
so highlighting the same passage again does not mint a duplicate follow-up.

Coordination holds (`coordination_timeout`, `coordination_failed`, and
`marker_changed`) write nothing and hint to close Highlights and retry. If
Highlights saves new annotations during the grant, overwrite aborts before
intent or apply, keeps those bytes, and asks for `bob ref scan -w` then
retry. A later
scan that sees the archived PDF bytes back at the live path holds with
`replaced_capture` (`Highlights wrote the replaced capture back`).

`overwrite:` names the replaced reading with its final status and highlight
count. `task:` prints the final mark and status (`· pass -s ready to reopen`
is appended for a kept closed, non-archived task). A replacement episode
prints `audio: replaced <path>`; a kept episode keeps the note embed and the
Play link of the replaced capture. With no authored prose and no annotations
there is no archive: `earlier: nothing to archive (0 highlights, no notes); git keeps
the old capture`. The commit prints `sync: committed <sha7>` like an
ordinary create; when the post-sync cannot publish, bob keeps the commit and
prints `overwritten; push pending (run bob vault-sync)`. A verification
failure exits 1 with code `verify_failed` and rolls the bundle back. Other
refusal codes: `legacy_reference`, `intake_reference`, `not_in_sync`,
`dirty_paths`, `open_note_tasks`, `intake_unavailable`,
`annotated_reference`, the incompatible-flag refusals (`-o`, `-k`, a
differing `-P` or `-t`, an unstampable `-s`, an archived `-s` reopen, a free
`-N`), and `nothing to overwrite` for a fresh source.

An exact-URL-set collection rereads the same way in any URL order. Several
readings sharing one set count as one family; unrelated duplicate families
sharing the set stay ambiguous until `-N <stem>` chooses one. Any partial
overlap, including one URL of a larger collection, refuses under every force
mode before adapters or fetch. Queued-PDF, exact-output (`-o`), and
companion-audio replacement keep working under either mode, and automatic
producers (capture workers, `ingest_url`, Keep) never mint a new reading.

```bash
bob ref create https://example.com/essay -f -P sase
bob ref create https://example.com/a https://example.com/b -f -P sase
bob ref create https://example.com/a https://example.com/b -f -L -P sase
```

## Configuration

```yaml
highlights:
  # `bob ref create --listen` narrates the target
  # with `sase listen render ...`, which must write MP3 audio to {audio}; bob binds it as
  # the PDF's companion audio. bob passes {target} {pdf} {audio} {title} as single
  # arguments to `sase` itself — do not quote them. The plugin's feed.auto_publish publishes the
  # episode. Installing Bob does not install a narration frontend.
  listen_command: sase listen render {target} -e full -o {audio}
```

A direct `sase-listen render {target} -e full -o {audio}` setting migrates automatically
to `sase listen render ...` in memory (the file is never rewritten). Wrappers,
shell programs, chains, pipelines, redirections, and expansions are rejected with
the canonical remediation. The managed setup above is the SASE plugin
(`sase plugin install listen`).

`BOB_HIGHLIGHTS_LISTEN_COMMAND` overrides the configured command. The setting must be
a direct `sase listen render ...` invocation (a direct `sase-listen render ...`
migrates); it must contain `{audio}` plus `{target}` or `{pdf}`; unknown placeholders,
quoted placeholders, wrappers, chains, pipelines, redirections, and shell expansions
are errors. `bob ref doctor` reports the `listen_command` row (`none`, `ok`, or `warn`)
and the `listen_feed` queue row, and checks the fixed `sase` launcher. An unconfigured
`--listen` fails with the config snippet above.

Troubleshooting:

- A missing launcher (`sase: command not found`) or a failing plugin:
  `sase` itself is missing from PATH, or the listen plugin (or its credentials)
  failed. Check with `command -v sase`, `sase listen --help`, and
  `bob ref doctor --no-hooks`. Doctor's `listen_command` row checks the fixed `sase`
  launcher and template validity (an `ok` row is not proof the plugin works).
  There is no fallback to `sase-listen`: a direct `sase-listen render ...` setting
  migrates to `sase listen render ...`, and wrappers/shell programs are rejected
  with the canonical remediation. The failed command left the
  vault untouched and can be rerun after SASE and the setting are corrected.
  Temporary override:

  ```sh
  BOB_HIGHLIGHTS_LISTEN_COMMAND='sase listen render {target} -e full -o {audio}' \
    bob ref create https://interconnected.org/home/2026/01/15/reminders -i -L -P sase
  ```

  Permanent repair: set `highlights.listen_command` in the managed Bob
  config to the canonical plugin template above and apply it.
- A listen-renderer bot-wall 403 on an article that bob captured headed: bob
  captured the PDF fine, but the renderer fetches the URL itself and fails.
  Replay the page another way, or narrate the staged PDF.
- A credential prompt from `pass` (via sase-listen's `api_key_command`):
  it inherits bob's stdin, so answer it in the terminal and the run
  continues.
- A queued publish (remote publish failure): the episode still binds and the
  vault is fine, but it will not reach AntennaPod until the feed host is fixed.
  Check `sase listen doctor` on the feed host, then run
  `sase listen publish --pending`; pull to refresh the feed in AntennaPod.
  For example, a feed host whose `token_command` needs an unlocked gpg-agent
  fails every non-interactive publish.
- A rerun after failure reuses the renderer's source, writer, and chunk
  caches, then re-masters and re-publishes; a kept scratch episode rebinds
  with `--audio`.

## Scratch staging

Renders, downloads, stamped copies, and filter files live in a `0700`
scratch directory (`TMPDIR` when short, else `/tmp`) until the final atomic
installs, because `create`, `scan`, and the per-host intake drain treat every
file in `xlib/` as input.
It is removed on drop, kept with `BOB_HIGHLIGHTS_KEEP_WORKDIR=1` (or the
legacy `BOB_WEB_CLIP_KEEP_WORKDIR=1`). The note snapshot renderer keeps the
same ownership: with the keep flag set, the scratch directory retains
`projected.md`, `note-profile.tex`, `note.lua`, `fonts/`, `glyphs.lua`, the
pandoc-generated `note.tex`, `note.log`, and `note.pdf` for debugging.
When Mermaid blocks are present, `diagrams/` holds the collect manifest,
render results, and per-diagram PDFs until XeLaTeX finishes; the same keep flag
retains those files for inspection.

## Environment variables

| Variable | Meaning |
| -------- | ------- |
| `BOB_HIGHLIGHTS_CURL` | Replaces `curl` (test seam, like `BOB_PANDOC_COMMAND`) |
| `BOB_HIGHLIGHTS_RESOLVE` | Replaces DNS for the fetch address check: comma-separated `host=ip` pairs with `*` as a wildcard (test seam) |
| `BOB_HIGHLIGHTS_KEEP_WORKDIR` | `1` keeps the scratch directory for debugging |
| `BOB_HIGHLIGHTS_DIAGRAMS` | `off`, `0`, `false`, or `no` skips Mermaid rendering; skipped blocks print as source without a fallback note |
| `BOB_PANDOC_COMMAND` | Overrides pandoc for the Markdown route |
| `BOB_XELATEX_COMMAND` | Overrides xelatex for the note snapshot print profile (test seam, like `BOB_PANDOC_COMMAND`) |
| `BOB_HIGHLIGHTS_AUDIO_LIBRARY` | sase-listen library for Markdown audio discovery |
| `BOB_HIGHLIGHTS_LISTEN_COMMAND` | Overrides `highlights.listen_command` for `--listen` |

Fetching runs `curl -sS --proto =http,https --connect-timeout 15
--max-time 300 --max-filesize 95M`, follows up to 10 redirects manually
through URL validation (a private-host redirect is refused), and reports
`curl` exit codes (6/7/28/35/60/63) with hints. Before every hop the host
is resolved and refused when any address is non-global (`<host> resolves
to a private address (<addr>)`); curl is then pinned to the checked
address with `--resolve host:port:addr`. The article adapter's own browser
navigation is not pinned: only bob's curl fetches go through the
resolved-address check. A missing `curl` hints at
`install curl or set BOB_HIGHLIGHTS_CURL`. On a TTY one
`fetching <host>…` line goes to stderr. `bob ref doctor` reports the
`curl` row.

## Verified on athena and the Mac

The extended note snapshot corpus gate ran on athena on 2026-10-17 from this
branch (working-tree parent `860f0be` plus working-tree resolver, visibility,
corpus-gate, and docs changes), with pandoc 3.1.11.1 and XeTeX
3.141592653-2.6-0.999996 (TeX Live 2025/dev/Debian):

- 103 area/project notes rendered successfully; 5,081 Markdown notes dry-ran
  successfully.
- 0 render failures, 0 dry-run failures, 0 missing-glyph warnings, 0 overfull
  warnings, 0 Type 3 fonts, and 0 residue findings (status-chip rows, raw
  block-ID labels, blank lines before task text, false local-heading warnings,
  false unresolved-file warnings). The vault git status was unchanged.
- Dry runs reported 5,543 unresolved-reference warnings and 4,348 parent
  fallbacks through `body` (plus 1 same-day name fallback). The gate treats an
  unresolved target as a failure when a matching vault file exists, including
  unique basename matches for directory-qualified targets; the remaining
  warnings are genuine missing notes, attachments, headings, or block
  fragments, plus cyclic or oversized embed skips. The summary contains no
  vault note contents.

The Mac verification ran on 2026-10-17 at `df4e120`, before these fixes. The
Mac used pandoc 3.11 and BasicTeX TeX Live 2026, with user-mode `titlesec`,
`enumitem`, and `lastpage`. The gate rendered 101 notes and dry-ran 5,062; it
reported 0 failures, 0 missing glyphs, 0 Type 3 fonts, and 0 overfull
warnings, and left the vault unchanged. The PDFKit probe paired 101/101 PDFs;
all 240 unknown tokens were explained (207 “In this note” chrome, 29
return-link tag joins, and 4 wikilink-suffix joins). The research-report probe
found 0 hyphen splits and 0 ligatures. These fixes add no TeX packages or
fonts.

The 2026-10-17 Highlights round trip returned a highlight spanning a task row
and its child bullet with every word byte-identical to the source; the child
row's list-bullet dash was included in the quote. The ref note carries
`source_note: "[[sase_blog]]"`. A live `#task`-comment follow-up and image
highlight were not exercised; CLI annotation-task tests cover that path.

Live verification ran 2026-10-06 into a scratch vault (`-b /tmp/lv-vault`):

- Every non-listen target kind created, and `scan` wrote 5 ref notes
  (markdown, local PDF, arXiv 1706.03762, PDF-URL dummy.pdf, Wikipedia
  article); a pdf-spelling dedupe rerun refused with the `--listen` hint.
- A real unpublished full-edition render
  (`BOB_HIGHLIGHTS_LISTEN_COMMAND='sase listen render {target} -e full -o
  {audio} --no-publish'`) streams the listen plugin's Rich
  live checklist unchanged, and a failed listen writes nothing to the
  vault. Cost estimate for the arXiv paper was ≈$0.15 (10 chunks).
- The full-edition TTS backend stalled on 2026-10-06: the arXiv render sat
  at Synthesize 6/10 for 45+ minutes with zero new cached chunks, and a
  fresh 239-word local-PDF render synthesized 0/6 in 9 minutes. The
  `--listen` → `scan` → player path was therefore proven with a stub
  listen command writing ID3-marker bytes to `{audio}`: `create -L` exited
  0 with `audio: … (from --listen)`, `scan` moved the pair to
  `lib/papers/`, and the ref note carries
  `audio: "[[lib/papers/<stem>.mp3]]"` plus a `![[…mp3]]` player under the
  `^ref` task. Attach mode (`create <in-vault-PDF> -L`) printed `ok
  attached listen episode to existing capture`, left the library PDF bytes
  unchanged, and a rescan paired the episode; re-attaching where audio
  already exists refuses with `already has companion audio`.
- `cargo test highlights`: 152 passed, 1 failed
  (`listen_filter_renders_card_and_encoded_play_link` expects `\&` inside
  `\href`, but athena's pandoc 3.1.11.1 emits a bare `&`; recorded as a
  follow-up on bead bob-cli-4s.6).

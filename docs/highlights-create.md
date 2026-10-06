# `bob highlights create` — Markdown, PDF, and URL targets

`bob highlights create <TARGET>` turns Markdown, a local PDF, a PDF URL,
an arXiv paper URL, or a web article URL into a Highlights-ready PDF in
the intake (`xlib/<ref-type>/<stem>.pdf` by default), which
`bob highlights scan` later moves into `lib/` and turns into a `ref/`
note. A PDF is stamped as-is, never re-rendered. Web article URLs run
through the clip engine with `create`'s options and get the same PDF,
marker, and report as `bob highlights clip <URL>`.

```bash
bob highlights create [OPTIONS] <TARGET>
```

## Examples

```bash
# Markdown through pandoc (default ref type chat)
bob highlights create report.md

# Local PDF, stamped as-is (default ref type papers)
bob highlights create paper.pdf -t papers

# PDF URL with an explicit stem
bob highlights create https://example.com/paper.pdf -N my_paper

# arXiv paper, preview only
bob highlights create https://arxiv.org/abs/1706.03762 -d

# Web article through the clip engine (default ref type blogs)
bob highlights create https://example.com/article/hello

# Overrides
bob highlights create paper.pdf -T "The Real Title" -N my_paper -t docs
bob highlights create report.md --audio episode.mp3 --include-id
```

## Targets

| Kind | Route | Default `-t` | Title (first hit wins) | Stem (first hit wins) |
| ---- | ----- | ------------ | ---------------------- | --------------------- |
| Markdown (`.md`) | Rendered with pandoc/XeLaTeX | `chat` | `-T`, frontmatter `title`, first H1, file stem | `-N`, file stem |
| Local PDF (`.pdf` or `%PDF-` magic) | Stamped as-is | `papers` | `-T`, plausible Info `/Title`, humanized stem | `-N`, file stem if it passes `--name` validation, else `snake_case` |
| PDF URL (PDF content-type or sniffed `%PDF-`) | Downloaded with `curl`, stamped as-is | `papers` | `-T`, plausible Info `/Title`, humanized stem | `-N`, URL slug, short-title stem, `<host>_<YYYYMMDD>` |
| arXiv (`arxiv.org` abs/html/pdf) | PDF fetched from `arxiv.org/pdf/<id>`, metadata from the API | `papers` | `-T`, arXiv API title, plausible Info `/Title`, `arXiv <id>` | `-N`, short-title stem, `arxiv_<id>` |
| Web article (HTML 2xx or 403/429/503) | Captured with the clip engine (same PDF, marker, and report as `bob highlights clip`) | `blogs` | clip rules (`-T` maps to the clip title override) | clip rules (`-N`) |

Short-title stem: when the text before the first `:` is 1–4 words, that
prefix is the short name (`EA-Graph: …` becomes `ea_graph`); otherwise the
first 6 words stand in, run through `snake_case` (capped at 80 chars).
`Attention Is All You Need` becomes `attention_is_all_you_need`.

Plausible Info title: trimmed and whitespace-collapsed, at least 3 chars
with a letter, not equal to the stem, not a bare filename (`.pdf`, `.doc`,
`.docx`, `.tex`, `.dvi`, `.ps`, `.pages`), not an exporter default
(`Microsoft Word - …`, `Microsoft PowerPoint - …`), and not
`untitled`/`title`. The same normalization applies to `/Author`.

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
only with `-i` (or `-N`, which implies it).

A local PDF inside `lib/` or `xlib/` with a marker is already captured:
`already captured; bob highlights sync <PDF> re-syncs it` plus the
`--listen` attach hint. Without a marker it must be moved out first.

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
`https://arxiv.org/abs/<id>`; everything else uses the clip key. Ref notes
record `source_url` and legacy `url:`, and queued intake markers record
both, so existing paper refs take part instead of duplicating. A ref-note
hit or a queued-intake hit refuses (use `--force` only to overwrite the same
intake target). The refusal hint points at `--listen` attach mode.

Markdown and local PDFs outside the vault whose planned library destination
already exists keep refusing; identity is not proven by stem alone.

## Scratch staging

Renders, downloads, stamped copies, and filter files live in a `0700`
scratch directory (`TMPDIR` when short, else `/tmp`) until the final atomic
installs, because `scan` and `bob_xlib_pull` read every file in `xlib/`.
It is removed on drop, kept with `BOB_HIGHLIGHTS_KEEP_WORKDIR=1` (or the
legacy `BOB_WEB_CLIP_KEEP_WORKDIR=1`).

## Environment variables

| Variable | Meaning |
| -------- | ------- |
| `BOB_HIGHLIGHTS_CURL` | Replaces `curl` (test seam, like `BOB_PANDOC_COMMAND`) |
| `BOB_HIGHLIGHTS_KEEP_WORKDIR` | `1` keeps the scratch directory for debugging |
| `BOB_PANDOC_COMMAND` | Overrides pandoc for the Markdown route |
| `BOB_HIGHLIGHTS_AUDIO_LIBRARY` | sase-listen library for Markdown audio discovery |

Fetching runs `curl -sS --proto =http,https --connect-timeout 15
--max-time 30 --max-filesize 95M`, follows up to 10 redirects manually
through URL validation (a private-host redirect is refused), and reports
`curl` exit codes (6/7/28/35/60/63) with hints. A missing `curl` hints at
`install curl or set BOB_HIGHLIGHTS_CURL`. On a TTY one
`fetching <host>…` line goes to stderr. `bob highlights doctor` reports the
`curl` row.

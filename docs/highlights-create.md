# `bob ref create` — Markdown, PDF, and URL targets

`bob ref create <TARGET>` turns Markdown, a local PDF, a PDF URL,
an arXiv paper URL, or a web article URL into a Highlights-ready PDF in
the intake (`xlib/<ref-type>/<stem>.pdf` by default), which
`bob ref scan` later moves into `lib/` and turns into a `ref/`
note. A PDF is stamped as-is, never re-rendered. Web article URLs run
through the web-article engine with `create`'s options.

```bash
bob ref create [OPTIONS] <TARGET>
```

## Examples

```bash
# Markdown through pandoc (default ref type chat)
bob ref create report.md

# Local PDF, stamped as-is (default ref type papers)
bob ref create paper.pdf -t papers

# PDF URL with an explicit stem
bob ref create https://example.com/paper.pdf -N my_paper

# arXiv paper, preview only
bob ref create https://arxiv.org/abs/1706.03762 -d

# Web article through the web-article engine (default ref type blogs)
bob ref create https://example.com/article/hello

# Overrides
bob ref create paper.pdf -T "The Real Title" -N my_paper -t docs
bob ref create report.md --audio episode.mp3 --include-id

# Web article with saved page and metadata overrides
bob ref create https://example.com/essay -H saved.html
bob ref create https://example.com/essay -A "Jane Doe" -p 2026-01-02 -d
```
See [`highlights-clip.md`](highlights-clip.md) for web-article route details (reader-mode capture, headed retry, `--html` escape hatch, environment).

## Targets

| Kind | Route | Default `-t` | Title (first hit wins) | Stem (first hit wins) |
| ---- | ----- | ------------ | ---------------------- | --------------------- |
| Markdown (`.md`) | Rendered with pandoc/XeLaTeX | `chat` | `-T`, frontmatter `title`, first H1, file stem | `-N`, file stem |
| Local PDF (`.pdf` or `%PDF-` magic) | Stamped as-is | `papers` | `-T`, plausible Info `/Title`, humanized stem | `-N`, file stem if it passes `--name` validation, else `snake_case` |
| PDF URL (PDF content-type or sniffed `%PDF-`) | Downloaded with `curl`, stamped as-is | `papers` | `-T`, plausible Info `/Title`, humanized stem | `-N`, URL slug, short-title stem, `<host>_<YYYYMMDD>` |
| arXiv (`arxiv.org` abs/html/pdf) | PDF fetched from `arxiv.org/pdf/<id>`, metadata from the API | `papers` | `-T`, arXiv API title, plausible Info `/Title`, `arXiv <id>` (no version) | `-N`, short-title stem, `arxiv_<id>` (no version) |
| Web article (HTML 2xx or 403/429/503) | Captured with the web-article engine | `blogs` | web-article rules (`-T` overrides the title) | web-article rules (`-N`) |

Short-title stem: when the text before the first `:` is 1–4 words, that
prefix is the short name (`EA-Graph: …` becomes `ea_graph`); otherwise the
first 6 words stand in, run through `snake_case` (capped at 80 chars).
`Attention Is All You Need` becomes `attention_is_all_you_need`.

Plausible Info title: trimmed and whitespace-collapsed, at least 3 chars
with a letter, not equal to the source file stem or URL stem
(case-insensitive), not a bare filename (`.pdf`, `.doc`, `.docx`, `.tex`,
`.dvi`, `.ps`, `.pages`), not an exporter default (`Microsoft Word - …`,
`Microsoft PowerPoint - …`), and not `untitled`/`title`. The same
normalization applies to `/Author`.

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
PDF-backed ref-note hit or a queued-intake hit refuses (use `--force`
only to overwrite the same intake target). The refusal hint points at
`--listen` attach mode. A URL recorded only by legacy notes without a
Highlights PDF warns (`already in the library as <path>, a note without
a Highlights PDF; capturing a fresh copy`) and captures a fresh copy
instead of refusing; with `--listen` it is a normal capture plus listen,
never attach mode. A dry run adds
`legacy: <path> (superseded by this capture)` to its report.

Markdown and local PDFs outside the vault whose planned library destination
already exists keep refusing; identity is not proven by stem alone. The
refusal adds `hint: to add audio to that capture, run bob ref
create <library PDF> --listen`, naming the existing PDF path. The same hint
applies when the planned intake target already exists.

## Ingest boundary

`src/native/highlights_ref/ingest.rs::ingest_url` is the typed,
non-printing URL ingest shared by the capture background worker and
`bob gkeep pull`. It composes the same building blocks as create
(`resolve_url_syntactic`, `fetch_and_route`, `sources` dedupe,
`pdf_target` planning/stamp/install, `ClipAdapterClient`). It takes a
vault root plus a cleaned URL and returns `Created`, `AlreadyInLibrary`,
or `AlreadyQueued`, or an `IngestError` with a snake-case `kind` and a
`retryable()` flag. Fixed defaults: route-default ref type (`blogs` for
articles, `papers` for PDFs and arXiv), status `ready`, parent
`obsidian_ref`, no audio, no force, no title or name override. It holds
the machine-wide
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/ref/ingest.lock` (fs2
exclusive, blocking) for the whole call — the lock serializes the
capture worker and Keep pull — and reports
`waiting for another clip…` once through its progress callback while it
waits. `bob ref create` keeps its own printing routes and never calls
`ingest_url`. Ingest writes nothing to stdout and prints nothing to
stderr itself.
Error kinds: `network`, `timeout`, `http_status` (retryable only for 408,
429, and 5xx), `browser`, and `dependency` are retryable; `blocked`,
`thin`, `render`, `unsupported_content`, `collision`, `invalid_url`, and
`internal` are permanent. `IngestError::fallback_note(url)` renders the
shared `⚠️ Clip failed (<kind>): <message> · retry: bob ref create <url>`
bullet used by every fallback path.

## Listen

`-L, --listen` (conflicts with `-a` and `-n`) narrates the target with the
configured `highlights.listen_command` and binds the episode as the PDF's
companion audio. The web-article route accepts the same flag with the same semantics.

```bash
bob ref create https://arxiv.org/abs/1706.03762 -L
```

All or nothing, on every route:

1. The listen command is resolved and validated first: an unconfigured or
   invalid command fails before any fetch or render.
2. The target is resolved, deduped, and planned (see above).
3. The episode destination `<target>.mp3` is planned: an existing mirrored
   library audio refuses.
4. The PDF is produced in private scratch (rendered, downloaded, or
   adapter-captured).
5. A dry run prints the plan with `listen: would-run …` and stops.
6. The listen command runs with stdin, stdout, and stderr inherited, so
   sase-listen's live checklist and its "Published to apollo" summary
   appear unchanged. bob prints `listen: run <expanded command>` first.
7. Collisions and the audio plan are re-checked (the vault may have
   changed during a long listen), the audio is installed, then the PDF.
   If the PDF install fails, only an audio file this run created is
   deleted.

Placeholder values: `{target}` is the cleaned URL for URL routes (sase-listen
does its own arXiv rewrite), the absolute source path for a local PDF, and
the staged `<scratch>/<stem>.pdf` render for Markdown (pandoc sets its Info
Title, which sase-listen reads). `{pdf}` is the unstamped scratch PDF (or
the existing capture in attach mode), `{audio}` is `<scratch>/<stem>.mp3`,
and `{title}` is the resolved title. bob shell-quotes every value itself;
never quote the placeholders. Markdown skips audio discovery under
`--listen`, and the listen card's Play URI targets `<stem>.mp3`.

Success looks like this (arXiv example):

```text
listen: run sase-listen render https://arxiv.org/abs/1706.03762 -e full -o /tmp/bob-create-…/attention_is_all_you_need.mp3
  … sase-listen's live checklist and summary, unchanged …
ok created Highlights-ready PDF
source: https://arxiv.org/abs/1706.03762 (arXiv 1706.03762)
pdf: /home/bryan/bob/xlib/papers/attention_is_all_you_need.pdf
audio: /home/bryan/bob/xlib/papers/attention_is_all_you_need.mp3 (from --listen)
title: Attention Is All You Need
author: Ashish Vaswani et al.
published: 2017-06-12
captured: 2026-10-06
status: ready
parent: obsidian_ref
id: attention_is_all_you_need
pages: 15 · size: 2.2 MB
next: bob ref scan
```

A failed listen command writes nothing (`nothing was written to the vault;
rerun the same command once the listen error above is fixed`); rerunning is
cheap because sase-listen resumes from its caches. An interrupted listen
exits 130. Exit 0 with no MP3 at `{audio}` is an error. A failure after the
episode exists keeps the scratch directory, prints `kept: <audio path>`,
and hints at rebinding it (`bob ref create <TARGET> --audio <path>`
in normal mode, `copy it to <xlib dest> for scan to pair` in attach mode).

## Attach

When `--listen` is given and the target is already captured, the new
episode attaches to the existing capture instead of refusing:

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
at `xlib/<rel>.mp3`. The PDF and the ref note stay untouched; `scan`
late-pairs the audio (see `highlights-ref-sync.md`).

```text
ok attached listen episode to existing capture
source: https://arxiv.org/abs/2608.04278 (arXiv 2608.04278)
pdf: /home/bryan/bob/lib/papers/ea_graph.pdf (unchanged)
ref: /home/bryan/bob/ref/papers/ea_graph.md
audio: /home/bryan/bob/xlib/papers/ea_graph.mp3 (from --listen)
title: EA-Graph: Artifact-Anchored Verification Memory for Coding Agents under Upstream Drift
next: bob ref scan
```

## Configuration

```yaml
highlights:
  # `bob ref create --listen` narrates the target
  # with this command, which must write MP3 audio to {audio}; bob binds it as
  # the PDF's companion audio. bob shell-quotes {target} {pdf} {audio} {title}
  # itself — do not quote them. sase-listen's feed.auto_publish publishes the
  # episode.
  listen_command: sase-listen render {target} -e full -o {audio}
```

`BOB_HIGHLIGHTS_LISTEN_COMMAND` overrides the configured command. The
template must contain `{audio}` plus `{target}` or `{pdf}`; unknown
placeholders and quoted placeholders are errors. `bob ref doctor`
reports the `listen_command` row (`none`, `ok`, or `warn`). An unconfigured
`--listen` fails with the config snippet above.

Troubleshooting:

- A sase-listen bot-wall 403 on an article that bob captured headed: bob
  captured the PDF fine, but sase-listen fetches the URL itself and fails.
  Replay the page for sase-listen another way, or narrate the staged PDF.
- A credential prompt from `pass` (via sase-listen's `api_key_command`):
  it inherits bob's stdin, so answer it in the terminal and the run
  continues.
- A queued publish (remote publish failure): sase-listen warns and still
  exits 0, so the episode binds normally; publish drains later.
- A rerun after failure reuses sase-listen's source, writer, and chunk
  caches, then re-masters and re-publishes; a kept scratch episode rebinds
  with `--audio`.

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
| `BOB_HIGHLIGHTS_RESOLVE` | Replaces DNS for the fetch address check: comma-separated `host=ip` pairs with `*` as a wildcard (test seam) |
| `BOB_HIGHLIGHTS_KEEP_WORKDIR` | `1` keeps the scratch directory for debugging |
| `BOB_PANDOC_COMMAND` | Overrides pandoc for the Markdown route |
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

## Verified on athena

Live verification ran 2026-10-06 into a scratch vault (`-b /tmp/lv-vault`):

- Every non-listen target kind created, and `scan` wrote 5 ref notes
  (markdown, local PDF, arXiv 1706.03762, PDF-URL dummy.pdf, Wikipedia
  article); a pdf-spelling dedupe rerun refused with the `--listen` hint.
- A real unpublished full-edition render
  (`BOB_HIGHLIGHTS_LISTEN_COMMAND='sase-listen render {target} -e full -o
  {audio} --no-publish'` under `script -qec`) streams sase-listen's Rich
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

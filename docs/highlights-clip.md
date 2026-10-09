# Web article capture (`bob ref create <URL>`)

`bob ref create <URL>` turns a web article into a beautiful, readable,
provenance-stamped PDF in the Highlights intake (`xlib/blogs/` by default),
which the existing `bob ref scan` turns into a `ref/` note. It is the web-article route of `bob ref create` (see [`highlights-create.md`](highlights-create.md)); `bob ref clip` is a permanent hidden alias.

```bash
bob ref create [OPTIONS] <URL>
```

## Examples

```bash
# Capture into xlib/blogs/ (scan writes ref/blogs/<stem>.md later)
bob ref create https://example.com/posts/some-article/ -P sase

# Preview everything without writing
bob ref create https://example.com/posts/some-article/ -d -P sase

# Override extracted metadata
bob ref create https://example.com/posts/some-article/ \
  -T "The Real Title" -A "Jane Doe" -p 2026-04-27 -P sase

# Replay a page saved from a real browser (see below)
bob ref create https://example.com/posts/some-article/ -H saved.html -P sase

# Narrate the article and bind the episode as companion audio
bob ref create https://example.com/posts/some-article/ -L -P sase
```

`-P/--parent` is required on every invocation: the area or project note
that owns the reading task.

`-L, --listen` narrates the article with `highlights.listen_command`
(`BOB_HIGHLIGHTS_LISTEN_COMMAND` overrides; see
[`highlights-create.md`](highlights-create.md) for the contract) and binds
the episode beside the PDF as `<stem>.mp3`. The command's output streams
unchanged, and nothing is written when it fails. When the URL is already
captured, `--listen` attaches the episode (`xlib/<rel>.mp3`) for `scan` to
late-pair onto the existing PDF and ref note — unless the URL is recorded
only by legacy notes without a Highlights PDF, which capture a fresh copy
plus the episode instead.

## The pipeline

1. The URL is validated (public `http(s)` only) and cleaned: the fragment
   and tracking parameters (`utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid`,
   `ref_src`) are removed. The cleaned URL is recorded as the marker
   `source_url`.
2. The URL is deduped against scalar ref-note `source_url` (and legacy
   `url:`) values and queued intake PDFs. A PDF-backed ref-note hit refuses
   even with `--force`; that flag can overwrite the same intake target when
   the planned path matches. A URL recorded only by notes without a
   Highlights PDF warns (`already in the library as <path>, a note without a
   Highlights PDF; capturing a fresh copy`) and captures a fresh copy.
   Import deduplication keeps only string scalars. A YAML list, including a
   one-element flow list (`url: [https://example.com/a]`) or a block list,
   is indexed by `ref find` and missed by this import check.
3. The pinned adapter fetches the page, extracts the article in reader
   mode, and renders a Bob-owned print template to PDF.
4. The PDF is stamped with a page-1 marker (`status`, `parent`, `title`,
   `id`, `source_url`, plus `author`, `published`, and `captured` when
   known) and installed into `xlib/<ref-type>/<stem>.pdf`.
5. `bob ref scan` later moves it into `lib/` and writes the note.
   Clip never writes `ref/` notes itself.

"Reliable" means "never silently wrong": bot challenges it cannot clear,
login walls, thin extractions, missing browsers, and private-network URLs
all fail closed with a `hint:` line and exit code 1, writing nothing.

## Hosts and browsers

The adapter starts headless and retries headed automatically when it meets
a bot challenge. That retry needs a display nobody sees: a private Xvfb
display on Linux (as on athena) or an off-screen window on macOS. Hosts
with no usable browser fail closed with an install hint; Playwright's
bundled Chromium counts when one is present (apollo captured headed over
an SSH-forwarded display). `bob ref doctor` reports the `web clip
uv`, `web clip adapter`, `web clip browser`, and `web clip headed
fallback` rows.

## The `--html` escape hatch

When a site will not yield to automation, save the page from a real
browser (Save Page As, or SingleFile) and replay it:

```bash
bob ref create https://example.com/walled/ -H ~/Downloads/walled.html
cat ~/Downloads/walled.html | bob ref create https://example.com/walled/ -H -
```

Subresources still load live on a best-effort basis; the challenge retry
is skipped.

## Environment variables

| Variable                    | Meaning                                                              |
| --------------------------- | -------------------------------------------------------------------- |
| `BOB_WEB_CLIP_ADAPTER`      | Replaces the adapter invocation (test seam)                          |
| `BOB_CHROME`                | Browser executable the adapter launches instead of discovery. Discovery itself tries Google Chrome at the usual Linux and macOS paths, then Playwright's bundled Chromium |
| `BOB_WEB_CLIP_TIMEOUT_SECS` | Overall adapter timeout in seconds (default 300)                     |
| `BOB_WEB_CLIP_KEEP_WORKDIR` | `1` keeps the scratch directory for debugging and prints its path    |

## Marker fields and dedupe

Every marker carries `status` (default `ready`), `parent` (required:
the resolved `-P` route), `title`, `id` (always the file stem), and
`source_url`.
It also carries `author`, `published` (`YYYY-MM-DD`), and `captured`
(`YYYY-MM-DD`, local date) when known; a field is omitted rather than
written wrong.

The dedupe key lowercases the scheme and host, drops a leading `www.` and
the default port, strips the trailing slash, and sorts the remaining
query parameters, so the same article reached through two URL spellings
dedupes. Default targets walk `<stem>`, `<stem>_2`, …: a name taken by a
different reference is suffixed (with a `renamed:` line before `pdf:`),
and only the same reference refuses. `--force` overwrites the same intake
target only; library PDFs are never overwritten, and an already-captured
ref note is never remade.

A ref-note hit refuses only when the note is PDF-backed (it carries a
`source_pdf`). A URL recorded only by legacy notes without a Highlights
PDF warns (`already in the library as <path>, a note without a
Highlights PDF; capturing a fresh copy`) and captures anyway; `bob ref
find` and `bob ref list` treat the older notes as superseded once `bob
ref scan` writes the new one. A dry run adds
`legacy: <path> (superseded by this capture)` to its report.

## Failure kinds

| Kind                  | Meaning and fix                                                      |
| --------------------- | -------------------------------------------------------------------- |
| `blocked`             | Challenge or login wall; rerun headed, or pass `--html FILE`         |
| `thin`                | Extraction too small; retry with `--html`, or not an article         |
| `browser`             | No usable browser. The hint is `install Google Chrome or set BOB_CHROME=/path/to/chrome`. A system Chromium package needs `BOB_CHROME` |
| `network`             | DNS, TLS, HTTP, or private-network refusal                           |
| `timeout`             | Capture took too long; raise `BOB_WEB_CLIP_TIMEOUT_SECS`             |
| `unsupported_content` | Non-HTML, non-PDF content                                            |
| `render`              | Printing failed                                                      |
| `invalid_request`     | Malformed adapter request (a bug; report it)                         |

## Adapter protocol v1 (summary)

The Rust side writes one compact JSON `capture` request to the adapter's
stdin and reads one JSON response from stdout; `ping` checks versions,
browser, and headed fallback for `doctor`. The adapter exits 0 whenever it
wrote a response. Non-JSON stdout or a nonzero exit is a protocol error,
reported with the stderr tail. The full request/response shapes live in
`src/native/highlights_ref/clip_adapter.rs`.

## Verified (2026-10-01)

Live gate for `https://openai.com/index/open-source-codex-orchestration-symphony/`,
built from this tree (`cargo build --release`) and copied to athena's
`/tmp/bob-clip-verify/bob` (never installed over athena's `~/.cargo/bin/bob`):

- `highlights doctor` on athena: uv, adapter (playwright 1.62.0, defuddle
  0.19.4), browser (chrome 154.0.8037.92), and headed fallback (xvfb) all OK.
- Dry run: target `~/bob/xlib/blogs/open_source_codex_orchestration_symphony.pdf`,
  title, author, published 2026-04-27 (visible-date), capture headed (Xvfb)
  after a headless challenge, fidelity ok.
- Scratch-vault capture (`-b /tmp/bob-clip-verify/vault`): 38 pages, images
  2/2, code blocks 1/1, 8912 words, 483.5 KB. Rendered pages 1-6 and 24 at
  80 dpi and viewed: clean masthead, both SVG diagrams are the light
  variants and clearly visible, the spec code block wraps, footers read
  `n / N` (first page without title), no site navigation or junk.
  `mutool draw -F txt` finds no U+00AD, U+2060, U+200B-D, U+FEFF, or
  ligature code points (U+FB00-FB06).
- Scratch scan (`--no-hooks`, scratch vault only): `ref/blogs/<stem>.md`
  carries `type: "[[ref]]"`, `ref_type: blogs`, `title`, `id`, `source_url`,
  `author`, `published`, `captured`, and the `^ref` task line; a second scan
  is a no-op (1 unchanged, writes: none).
- Real capture on athena into `~/bob/xlib/blogs/`; a re-run refuses the
  occupied target (exit 1, `--force` hint). The Mac's `bob_xlib_pull`
  drained the queue and created the ref note; vault git sync carried it to
  athena (and apollo) with all provenance fields intact.
- Static-site check (`https://lilianweng.github.io/posts/2023-06-23-agent/`,
  scratch vault): headless, no headed fallback, 28 pages, images 13/13,
  fidelity ok; figures and code blocks render correctly.
- apollo note: this host has Playwright's bundled Chromium 151, so with the
  session's SSH-forwarded `DISPLAY` it captured the OpenAI URL headed
  instead of failing closed (38 pages, 2/2 images). The stray real-vault
  PDF this produced was deleted; `~/bob/xlib/` is empty. Truly
  browser-less hosts still fail closed via the `browser`/`blocked` errors
  covered by the CLI and adapter fixture tests.

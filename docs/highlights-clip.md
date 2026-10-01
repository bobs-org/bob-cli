# `bob highlights clip` — web URL to Highlights reference PDF

`bob highlights clip <URL>` turns a web article into a beautiful, readable,
provenance-stamped PDF in the Highlights intake (`xlib/blogs/` by default),
which the existing `bob highlights scan` turns into a `ref/` note. It is a
sibling of `bob highlights create` and shares its target planning, collision
guards, marker composition, and atomic install.

```bash
bob highlights clip [OPTIONS] <URL>
```

## Examples

```bash
# Capture into xlib/blogs/ (scan writes ref/blogs/<stem>.md later)
bob highlights clip https://example.com/posts/some-article/

# Preview everything without writing
bob highlights clip https://example.com/posts/some-article/ -d

# Override extracted metadata
bob highlights clip https://example.com/posts/some-article/ \
  -T "The Real Title" -A "Jane Doe" -p 2026-04-27

# Replay a page saved from a real browser (see below)
bob highlights clip https://example.com/posts/some-article/ -H saved.html
```

## The pipeline

1. The URL is validated (public `http(s)` only) and cleaned: the fragment
   and tracking parameters (`utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid`,
   `ref_src`) are removed. The cleaned URL is recorded as the marker
   `source_url`.
2. The URL is deduped against every ref-note `source_url` and every queued
   intake PDF. An already-captured URL is refused, even with `--force`.
3. The pinned adapter fetches the page, extracts the article in reader
   mode, and renders a Bob-owned print template to PDF.
4. The PDF is stamped with a page-1 marker (`status`, `parent`, `title`,
   `id`, `source_url`, plus `author`, `published`, and `captured` when
   known) and installed into `xlib/<ref-type>/<stem>.pdf`.
5. `bob highlights scan` later moves it into `lib/` and writes the note.
   Clip never writes `ref/` notes itself.

"Reliable" means "never silently wrong": bot challenges it cannot clear,
login walls, thin extractions, missing browsers, and private-network URLs
all fail closed with a `hint:` line and exit code 1, writing nothing.

## Hosts and browsers

The adapter starts headless and retries headed automatically when it meets
a bot challenge. That retry needs a display nobody sees: a private Xvfb
display on Linux (as on athena) or an off-screen window on macOS. Hosts
with no browser (apollo has none) fail closed with an install hint.
`bob highlights doctor` reports the `web clip uv`, `web clip adapter`,
`web clip browser`, and `web clip headed fallback` rows.

## The `--html` escape hatch

When a site will not yield to automation, save the page from a real
browser (Save Page As, or SingleFile) and replay it:

```bash
bob highlights clip https://example.com/walled/ -H ~/Downloads/walled.html
cat ~/Downloads/walled.html | bob highlights clip https://example.com/walled/ -H -
```

Subresources still load live on a best-effort basis; the challenge retry
is skipped.

## Environment variables

| Variable                    | Meaning                                                              |
| --------------------------- | -------------------------------------------------------------------- |
| `BOB_WEB_CLIP_ADAPTER`      | Replaces the adapter invocation (test seam)                          |
| `BOB_CHROME`                | Chrome/Chromium executable the adapter launches instead of discovery |
| `BOB_WEB_CLIP_TIMEOUT_SECS` | Overall adapter timeout in seconds (default 300)                     |
| `BOB_WEB_CLIP_KEEP_WORKDIR` | `1` keeps the scratch directory for debugging and prints its path    |

## Marker fields and dedupe

Every marker carries `status` (default `ready`), `parent` (default
`obsidian_ref`), `title`, `id` (always the file stem), and `source_url`.
It also carries `author`, `published` (`YYYY-MM-DD`), and `captured`
(`YYYY-MM-DD`, local date) when known; a field is omitted rather than
written wrong.

The dedupe key lowercases the scheme and host, drops a leading `www.` and
the default port, strips the trailing slash, and sorts the remaining
query parameters, so the same article reached through two URL spellings
dedupes. `--force` overwrites the same intake target only; library PDFs
are never overwritten, and an already-captured ref note is never remade.

## Failure kinds

| Kind                  | Meaning and fix                                                      |
| --------------------- | -------------------------------------------------------------------- |
| `blocked`             | Challenge or login wall; rerun headed, or pass `--html FILE`         |
| `thin`                | Extraction too small; retry with `--html`, or not an article         |
| `browser`             | No usable browser; install Chrome or set `BOB_CHROME`                |
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

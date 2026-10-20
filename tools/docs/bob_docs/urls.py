"""Built-site URL contracts: resolution, urls.txt, mention scans, curated links.

The ``urls`` command owns every fragment-aware check over the published site.
It builds an index from the actual built HTML (page paths plus ``id``/``name``
anchors), resolves directory URLs, index pages, percent-encoded paths and
fragments, and redirect stubs with bounded loop detection, then verifies three
independent URL sources against that index:

* ``target/docs/site/urls.txt`` written from the build, compared with the live
  org-root ``/urls.txt`` so previously published pages keep resolving;
* every literal ``docs/<path>.md(#fragment)`` mention in ``src/``, ``tests/``,
  the root ``README.md``, and ``docs/``;
* the curated absolute site URLs in ``docs/_meta/url_contract.txt``.

Lychee (see :mod:`bob_docs.links`) never checks fragments on ``file://``
directory URLs, so this resolver is the authoritative fragment gate.
"""

from __future__ import annotations

import fnmatch
import re
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from html import unescape
from html.parser import HTMLParser
from pathlib import Path

from .context import DocsContext
from .errors import DocsError

MAX_REDIRECT_HOPS = 16
LIVE_TIMEOUT_SECONDS = 10
DOWNLOAD_TIMEOUT_SECONDS = 120

URLS_TXT_NAME = "urls.txt"
ERROR_PAGE = "404.html"

MENTIONS_IGNORE_FILE = "url_mentions_ignore.txt"
URL_CONTRACT_FILE = "url_contract.txt"

_MENTION_RE = re.compile(
    r"(?<![A-Za-z0-9_-])docs/"
    r"(?P<page>[A-Za-z0-9_][A-Za-z0-9_./-]*\.md)"
    r"(?P<fragment>#[^\s)'\"<>`]+)?"
)
_SITE_URL_RE = re.compile(
    r"https?://[A-Za-z0-9.-]+(?::\d+)?(?:/[^\s)'\"<>`]*)?"
)
_TRAILING_PUNCTUATION = ".,;:!?"


def site_origin(site_url: str) -> str:
    """Return the ``scheme://host[:port]`` origin for a configured site URL."""

    parsed = urllib.parse.urlsplit(site_url.strip())
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise DocsError(f"mkdocs.yml: site_url must be an absolute http(s) URL, got {site_url!r}")
    port = f":{parsed.port}" if parsed.port else ""
    return f"{parsed.scheme}://{parsed.hostname.lower()}{port}"


def strip_trailing_punctuation(value: str) -> str:
    """Drop prose punctuation accidentally captured at the end of a URL."""

    return value.rstrip(_TRAILING_PUNCTUATION)


class _PageParser(HTMLParser):
    """Collect anchors, the canonical link, and the meta-refresh target."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=False)
        self.anchors: set[str] = set()
        self.canonical: str | None = None
        self.refresh_target: str | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = {name.lower(): (value or "") for name, value in attrs}
        if "id" in values and values["id"]:
            self.anchors.add(unescape(values["id"]))
        if tag == "a" and values.get("name"):
            self.anchors.add(unescape(values["name"]))
        if tag == "link" and values.get("rel", "").lower() == "canonical" and values.get("href"):
            if self.canonical is None:
                self.canonical = values["href"].strip()
        if tag == "meta" and values.get("http-equiv", "").lower() == "refresh":
            target = _refresh_target(values.get("content", ""))
            if target and self.refresh_target is None:
                self.refresh_target = target


def _refresh_target(content: str) -> str | None:
    for part in content.split(";"):
        part = part.strip()
        if part.lower().startswith("url="):
            target = part[4:].strip().strip("'\"")
            if target:
                return target
    return None


@dataclass(frozen=True)
class SitePage:
    url_path: str
    file: Path
    anchors: frozenset[str]
    redirect: str | None


@dataclass
class SiteIndex:
    """Pages and anchors of one built site, keyed by URL path."""

    site_dir: Path
    origin: str
    pages: dict[str, SitePage] = field(default_factory=dict)

    @classmethod
    def from_dir(cls, site_dir: Path, origin: str) -> SiteIndex:
        index = cls(site_dir=site_dir.resolve(), origin=origin)
        for path in sorted(site_dir.rglob("*.html")):
            relative = path.relative_to(site_dir).as_posix()
            if relative == ERROR_PAGE or "/404.html" in f"/{relative}":
                continue
            url_path = _url_path_for(relative)
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError as error:
                raise DocsError(f"cannot read built page {path}: {error}") from error
            parser = _PageParser()
            parser.feed(text)
            index.pages[url_path] = SitePage(
                url_path=url_path,
                file=path.resolve(),
                anchors=frozenset(parser.anchors),
                redirect=parser.refresh_target,
            )
        return index


def _url_path_for(relative: str) -> str:
    if relative == "index.html":
        return "/"
    if relative.endswith("/index.html"):
        return f"/{relative[: -len('index.html')]}"
    return f"/{relative}"


@dataclass(frozen=True)
class ResolvedURL:
    page: str
    fragment: str | None


class SiteResolver:
    """Resolve published URLs against a built-site index."""

    def __init__(self, index: SiteIndex) -> None:
        self._index = index

    @property
    def origin(self) -> str:
        return self._index.origin

    def page_paths(self) -> list[str]:
        return sorted(self._index.pages)

    def resolve(self, url: str) -> ResolvedURL:
        """Resolve one site-relative, absolute-path, or absolute-origin URL."""

        path, fragment = self._split(url)
        normalised = self._normalise_path(path, url)
        page = self._follow(normalised, url, [normalised])
        target_fragment = page[1] if page[1] else fragment
        if target_fragment:
            anchors = self._index.pages[page[0]].anchors
            if target_fragment not in anchors:
                raise DocsError(
                    f"{url}: anchor #{target_fragment} not found on {page[0]} "
                    f"({len(anchors)} anchors indexed)"
                )
        return ResolvedURL(page=page[0], fragment=target_fragment)

    def _split(self, url: str) -> tuple[str, str | None]:
        text = url.strip()
        scheme_match = re.match(r"^([A-Za-z][A-Za-z0-9+.-]*):", text)
        if scheme_match and scheme_match.group(1).lower() not in ("http", "https"):
            raise DocsError(
                f"{url}: only http(s) site URLs resolve, not {scheme_match.group(1)!r}"
            )
        if "://" in text or text.lower().startswith("http"):
            parsed = urllib.parse.urlsplit(text)
            if parsed.scheme not in ("http", "https"):
                raise DocsError(f"{url}: only http(s) site URLs resolve, not {parsed.scheme!r}")
            candidate = f"{parsed.scheme}://{(parsed.hostname or '').lower()}"
            if parsed.port:
                candidate += f":{parsed.port}"
            if candidate != self._index.origin:
                raise DocsError(
                    f"{url}: external URL is not part of the {self._index.origin} URL contract"
                )
            if parsed.query:
                raise DocsError(f"{url}: query strings are not part of the published URL contract")
            path, fragment = parsed.path, (parsed.fragment or None)
        else:
            if "?" in text:
                raise DocsError(f"{url}: query strings are not part of the published URL contract")
            path, hash_mark, fragment_text = text.partition("#")
            if hash_mark and "#" in fragment_text:
                raise DocsError(f"{url}: only one fragment marker is allowed")
            fragment = fragment_text or None
        if "\\" in path:
            raise DocsError(f"{url}: backslashes never appear in published site paths")
        try:
            path = urllib.parse.unquote(path, encoding="utf-8", errors="strict")
        except (UnicodeDecodeError, ValueError) as error:
            raise DocsError(f"{url}: path is not valid percent-encoded UTF-8: {error}") from error
        if fragment is not None:
            try:
                fragment = urllib.parse.unquote(fragment, encoding="utf-8", errors="strict")
            except (UnicodeDecodeError, ValueError) as error:
                raise DocsError(
                    f"{url}: fragment is not valid percent-encoded UTF-8: {error}"
                ) from error
            if not fragment:
                fragment = None
        return path, fragment

    def _normalise_path(self, path: str, url: str) -> str:
        if not path.startswith("/"):
            path = f"/{path}"
        parts: list[str] = []
        for part in path.split("/"):
            if part in ("", "."):
                continue
            if part == "..":
                if not parts:
                    raise DocsError(f"{url}: path escapes the site root")
                parts.pop()
                continue
            if "\x00" in part:
                raise DocsError(f"{url}: path contains a NUL byte")
            parts.append(part)
        return "/" + "/".join(parts)

    def _page_file(self, path: str) -> Path | None:
        site_dir = self._index.site_dir
        candidates = [path, f"{path}/index.html", f"{path}.html"] if not path.endswith("/") else [path, f"{path}index.html"]
        seen: set[str] = set()
        for candidate in candidates:
            if candidate in seen:
                continue
            seen.add(candidate)
            if candidate == "/":
                file_path = site_dir / "index.html"
            else:
                file_path = site_dir / candidate.lstrip("/")
            if file_path.is_symlink():
                try:
                    resolved = file_path.resolve(strict=True)
                except OSError as error:
                    raise DocsError(f"{candidate}: destination symlink is broken: {error}") from error
                if not resolved.is_relative_to(site_dir):
                    raise DocsError(f"{candidate}: destination symlink escapes the built site")
                file_path = resolved
            if file_path.is_file():
                return file_path
        return None

    def _follow(self, path: str, url: str, visited: list[str]) -> tuple[str, str | None]:
        file_path = self._page_file(path)
        if file_path is None:
            raise DocsError(f"{url}: no built page for {path}")
        page_path = self._page_path_for_file(file_path)
        page = self._index.pages.get(page_path)
        if page is None or not page.redirect:
            return page_path, None
        target = self._redirect_target(page.redirect, page_path, url)
        target_path, _, target_fragment = target.partition("#")
        fragment = urllib.parse.unquote(target_fragment) if target_fragment else None
        if target_path in visited:
            raise DocsError(
                f"{url}: redirect loop detected: {' -> '.join([*visited, target_path])}"
            )
        if len(visited) > MAX_REDIRECT_HOPS:
            raise DocsError(f"{url}: redirect chain exceeds {MAX_REDIRECT_HOPS} hops")
        final_path, final_fragment = self._follow(target_path, url, [*visited, target_path])
        return final_path, fragment if fragment else final_fragment

    def _page_path_for_file(self, file_path: Path) -> str:
        relative = file_path.relative_to(self._index.site_dir).as_posix()
        return _url_path_for(relative)

    def _redirect_target(self, target: str, page_path: str, url: str) -> str:
        if "://" in target or target.lower().startswith("http"):
            parsed = urllib.parse.urlsplit(target)
            candidate = f"{parsed.scheme}://{(parsed.hostname or '').lower()}"
            if parsed.port:
                candidate += f":{parsed.port}"
            if candidate != self._index.origin:
                raise DocsError(f"{url}: redirect from {page_path} leaves the site: {target}")
            if parsed.query:
                raise DocsError(f"{url}: query strings are not part of the published URL contract")
            return parsed.path + (f"#{parsed.fragment}" if parsed.fragment else "")
        if target.startswith("/"):
            if "?" in target:
                raise DocsError(f"{url}: query strings are not part of the published URL contract")
            return self._normalise_path(target.split("#", 1)[0], url) + (
                f"#{target.split('#', 1)[1]}" if "#" in target else ""
            )
        base = page_path if page_path.endswith("/") else page_path.rsplit("/", 1)[0] + "/"
        if "?" in target:
            raise DocsError(f"{url}: query strings are not part of the published URL contract")
        head, hash_mark, tail = target.partition("#")
        joined = self._normalise_path(base + head, url)
        return joined + (f"#{tail}" if hash_mark else "")


def published_page_urls(index: SiteIndex) -> list[str]:
    """Sorted absolute page URLs for ``urls.txt``, including redirect pages."""

    return sorted(f"{index.origin}{path}" for path in index.pages)


def write_urls_txt(site_dir: Path, index: SiteIndex) -> Path:
    """Write sorted ``urls.txt`` into the built site and return its path."""

    path = site_dir / URLS_TXT_NAME
    path.write_text("\n".join(published_page_urls(index)) + "\n", encoding="utf-8")
    return path


def fetch_published_urls(origin: str, *, timeout: int = LIVE_TIMEOUT_SECONDS) -> tuple[str, list[str] | None]:
    """Fetch the live ``/urls.txt``.

    Returns ``("ok", urls)``, ``("offline", None)`` for a genuinely
    unreachable site, or ``("first-publish", None)`` for an HTTP 404.
    A reachable but unparsable response raises :class:`DocsError`.
    """

    request = urllib.request.Request(
        f"{origin}/{URLS_TXT_NAME}", headers={"User-Agent": "bob-docs/urls"}
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status = getattr(response, "status", 200)
            if status == 404:
                return "first-publish", None
            if status < 200 or status >= 300:
                raise DocsError(
                    f"{origin}/{URLS_TXT_NAME}: unexpected HTTP status {status}"
                )
            try:
                body = response.read().decode("utf-8")
            except (UnicodeDecodeError, ValueError) as error:
                raise DocsError(
                    f"{origin}/{URLS_TXT_NAME}: response is not UTF-8 text: {error}"
                ) from error
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return "first-publish", None
        raise DocsError(
            f"{origin}/{URLS_TXT_NAME}: unexpected HTTP status {error.code}"
        ) from error
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        return "offline", (str(error) or type(error).__name__)
    urls: list[str] = []
    for line_number, line in enumerate(body.splitlines(), start=1):
        line = line.strip()
        if not line:
            continue
        parsed = urllib.parse.urlsplit(line)
        candidate = f"{parsed.scheme}://{(parsed.hostname or '').lower()}"
        if parsed.port:
            candidate += f":{parsed.port}"
        if (
            parsed.scheme not in ("http", "https")
            or candidate != origin
            or parsed.query
        ):
            raise DocsError(
                f"{origin}/{URLS_TXT_NAME}:{line_number}: malformed published URL {line!r}; "
                "expected an absolute site URL without a query string"
            )
        urls.append(line)
    return "ok", urls


@dataclass(frozen=True)
class UrlMention:
    source: Path
    line: int
    text: str
    kind: str


def scan_source_mentions(root: Path) -> list[UrlMention]:
    """Collect literal ``docs/<path>.md(#fragment)`` mentions with provenance."""

    mentions: list[UrlMention] = []
    files: list[Path] = []
    for pattern in ("src/**/*.rs", "tests/**/*.rs", "README.md", "docs/**/*.md"):
        files.extend(sorted(root.glob(pattern)))
    for path in files:
        if not path.is_file() or path.is_symlink():
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            raise DocsError(f"cannot read {path}: {error}") from error
        for line_number, line in enumerate(text.splitlines(), start=1):
            for match in _MENTION_RE.finditer(line):
                mention = strip_trailing_punctuation(match.group(0))
                mentions.append(
                    UrlMention(source=path, line=line_number, text=mention, kind="docs-mention")
                )
    return mentions


def scan_site_urls(paths: list[Path], origin: str) -> list[UrlMention]:
    """Collect absolute site URLs from assembled docs and source READMEs."""

    mentions: list[UrlMention] = []
    for path in sorted(paths):
        if not path.is_file() or path.is_symlink():
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            raise DocsError(f"cannot read {path}: {error}") from error
        for line_number, line in enumerate(text.splitlines(), start=1):
            for match in _SITE_URL_RE.finditer(line):
                candidate = strip_trailing_punctuation(match.group(0))
                if candidate == origin or candidate.startswith(f"{origin}/"):
                    mentions.append(
                        UrlMention(source=path, line=line_number, text=candidate, kind="site-url")
                    )
    return mentions


def mention_site_path(mention: str) -> str:
    """Map a ``docs/<path>.md(#fragment)`` mention onto a site URL path."""

    page, hash_mark, fragment = mention.partition("#")
    if not page.startswith("docs/") or not page.endswith(".md"):
        raise DocsError(f"{mention}: not a docs/<path>.md mention")
    stem = page[len("docs/") : -len(".md")]
    if not stem or stem.startswith("/") or "/../" in f"/{stem}/" or stem in (".", ".."):
        raise DocsError(f"{mention}: path escapes docs/")
    site_path = f"/{stem}/" if stem != "index" else "/"
    if hash_mark:
        return site_path + f"#{fragment}"
    return site_path


@dataclass(frozen=True)
class IgnoreEntry:
    match: str
    scope: str
    reason: str
    line: int


def load_mentions_ignore(root: Path) -> list[IgnoreEntry]:
    """Load narrowly scoped ``MATCH | SCOPE | REASON`` ignore entries."""

    path = root / "docs/_meta" / MENTIONS_IGNORE_FILE
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"cannot read {path}: {error}") from error
    entries: list[IgnoreEntry] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parts = [part.strip() for part in stripped.split("|")]
        if len(parts) != 3 or not all(parts):
            raise DocsError(
                f"{path}:{line_number}: expected MATCH | SCOPE | REASON, got {line.strip()!r}"
            )
        match, scope, reason = parts
        if scope == "*":
            raise DocsError(
                f"{path}:{line_number}: scope '*' is too broad; name the file(s) the "
                "non-public reference lives in"
            )
        entries.append(IgnoreEntry(match=match, scope=scope, reason=reason, line=line_number))
    return entries


def apply_ignores(
    mentions: list[UrlMention], entries: list[IgnoreEntry], *, root: Path
) -> tuple[list[UrlMention], dict[int, int]]:
    """Split mentions into kept ones; every entry must match at least one mention."""

    uses: dict[int, int] = {entry.line: 0 for entry in entries}
    kept: list[UrlMention] = []
    for mention in mentions:
        try:
            relative = mention.source.resolve().relative_to(root.resolve()).as_posix()
        except ValueError:
            relative = mention.source.as_posix()
        ignored = False
        for entry in entries:
            if entry.match in mention.text and fnmatch.fnmatchcase(relative, entry.scope):
                uses[entry.line] += 1
                ignored = True
        if not ignored:
            kept.append(mention)
    stale = sorted(line for line, count in uses.items() if count == 0)
    if stale:
        raise DocsError(
            f"docs/_meta/{MENTIONS_IGNORE_FILE}: stale entries match no scanned mention "
            f"(lines {', '.join(map(str, stale))}); remove them"
        )
    return kept, uses


def load_url_contract(root: Path, origin: str) -> list[str]:
    """Load curated absolute site URLs, skipping comments and blank lines."""

    path = root / "docs/_meta" / URL_CONTRACT_FILE
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"cannot read {path}: {error}") from error
    urls: list[str] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parsed = urllib.parse.urlsplit(stripped)
        candidate = f"{parsed.scheme}://{(parsed.hostname or '').lower()}"
        if parsed.port:
            candidate += f":{parsed.port}"
        if parsed.scheme not in ("http", "https") or candidate != origin:
            raise DocsError(
                f"{path}:{line_number}: expected an absolute {origin} URL, got {stripped!r}"
            )
        urls.append(stripped)
    return urls


def refresh_url_contract(root: Path, origin: str, observed: list[str]) -> tuple[int, int, list[str]]:
    """Validate the curated contract, append resolving observed candidates.

    Never removes existing obligations. Returns ``(before, after, added)``.
    """

    path = root / "docs/_meta" / URL_CONTRACT_FILE
    existing = load_url_contract(root, origin)
    known = set(existing)
    added = sorted({url for url in observed if url not in known})
    if added:
        text = path.read_text(encoding="utf-8")
        if text and not text.endswith("\n"):
            text += "\n"
        path.write_text(text + "".join(f"{url}\n" for url in added), encoding="utf-8")
    return len(existing), len(existing) + len(added), added


def _display_path(root: Path, path: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def _staged_markdown_files(staged_docs: Path) -> list[Path]:
    return sorted(
        path
        for path in staged_docs.rglob("*.md")
        if path.is_file() and not path.is_symlink()
    )


def _source_readmes(
    root: Path,
    *,
    production: bool,
    sources: Path | str | None,
    source_dirs: dict[str, Path] | None = None,
) -> list[Path]:
    from .mounts import discover_repo, load_mounts, load_production_sources

    repo_dirs: dict[str, Path] = {}
    if source_dirs is not None:
        repo_dirs = {name: path for name, path in source_dirs.items() if name != "bob-cli"}
    elif production:
        assert sources is not None
        for source in load_production_sources(
            Path(sources), root=root, repos=sorted({mount.repo for mount in load_mounts(root)})
        ):
            if source.name != "bob-cli":
                repo_dirs[source.name] = source.path
    else:
        for mount in load_mounts(root):
            if mount.repo not in repo_dirs:
                repo_dir, _ = discover_repo(mount, root=root)
                repo_dirs[mount.repo] = repo_dir
    readmes = [root / "README.md"]
    for name in sorted(repo_dirs):
        candidate = repo_dirs[name] / "README.md"
        if candidate.is_file() and not candidate.is_symlink():
            readmes.append(candidate)
    return readmes


def run_urls(
    root: Path,
    *,
    refresh: bool = False,
    production: bool = False,
    sources: Path | str | None = None,
    context: DocsContext | None = None,
) -> int:
    """Check every published-URL obligation against the built site."""

    from .assemble import assemble

    repo_root = Path(root).resolve()
    if context is None:
        context, _ = assemble(repo_root, production=production, sources=sources)
    site_url = context.config.get("site_url", "")
    if not isinstance(site_url, str):
        raise DocsError("mkdocs.yml: site_url must be a string")
    origin = site_origin(site_url)
    site_dir = context.site_dir
    if not (site_dir / "index.html").is_file():
        raise DocsError(f"no built site at {site_dir}; run `just docs-build` first")
    index = SiteIndex.from_dir(site_dir, origin)
    resolver = SiteResolver(index)

    failures: list[str] = []
    urls_path = write_urls_txt(site_dir, index)
    print(f"wrote {urls_path} ({len(index.pages)} published pages)")

    status, payload = fetch_published_urls(origin)
    if status == "offline":
        print(f"warning: live {origin}/{URLS_TXT_NAME} unreachable ({payload}); skipping comparison")
    elif status == "first-publish":
        print(f"warning: live {origin}/{URLS_TXT_NAME} is not published yet; skipping comparison")
    else:
        assert payload is not None
        for published in payload:
            try:
                resolver.resolve(published)
            except DocsError as error:
                failures.append(f"published URL no longer resolves: {published} -> {error}")

    mentions = scan_source_mentions(repo_root)
    site_url_mentions = scan_site_urls(
        _staged_markdown_files(context.output_dir / "docs")
        + _source_readmes(
            repo_root,
            production=production,
            sources=sources,
            source_dirs=context.source_dirs if context.source_dirs else None,
        ),
        origin,
    )
    ignored = load_mentions_ignore(repo_root)
    kept, _ = apply_ignores([*mentions, *site_url_mentions], ignored, root=repo_root)
    for mention in kept:
        try:
            if mention.kind == "docs-mention":
                resolver.resolve(mention_site_path(mention.text))
            else:
                resolver.resolve(mention.text)
        except DocsError as error:
            failures.append(
                f"{_display_path(repo_root, mention.source)}:{mention.line}: "
                f"{mention.text} -> {error}"
            )

    if refresh:
        observed = sorted({mention.text for mention in site_url_mentions})
        before, after, added = refresh_url_contract(
            repo_root, origin, [url for url in observed if _check_url(resolver, url)]
        )
        print(f"{URL_CONTRACT_FILE}: {before} -> {after} (+{len(added)})")
        for url in added:
            print(f"  + {url}")
    for contracted in load_url_contract(repo_root, origin):
        try:
            resolver.resolve(contracted)
        except DocsError as error:
            failures.append(f"docs/_meta/{URL_CONTRACT_FILE}: {contracted} -> {error}")

    if failures:
        print(f"urls: {len(failures)} broken URL obligation(s):")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(
        f"urls: OK ({len(index.pages)} pages, {len(kept)} mentions checked, "
        f"{len(load_url_contract(repo_root, origin))} curated links)"
    )
    return 0


def _check_url(resolver: SiteResolver, url: str) -> bool:
    try:
        resolver.resolve(url)
    except DocsError:
        return False
    return True

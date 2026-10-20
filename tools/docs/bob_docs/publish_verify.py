"""Verify the live documentation site after a publication deploy.

The check confirms three obligations with bounded retries:

1. Every page in the freshly built ``urls.txt`` answers HTTP 200 on the
   live origin.
2. Every curated absolute URL with a fragment (``docs/_meta/url_contract.txt``)
   resolves to a live page that contains the anchor.
3. The live ``/build.json`` provenance matches the deployed publication:
   exact source SHAs with clean working trees.

A genuinely unreachable site or a missing first publish is a verification
failure here, never a skip: this gate runs only after a deploy claimed to
succeed.
"""

from __future__ import annotations

import re
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .errors import DocsError
from .urls import fetch_published_urls, load_url_contract, site_origin

DEFAULT_RETRIES = 5
DEFAULT_TIMEOUT_SECONDS = 15
USER_AGENT = "bob-docs/publish-verify"

_FRAGMENT_PATTERNS = ("id=\"{fragment}\"", "name=\"{fragment}\"")


@dataclass(frozen=True)
class FetchedPage:
    status: int
    body: str
    final_url: str


def fetch_text(url: str, *, timeout: int) -> FetchedPage:
    """GET a live URL; transport failures raise :class:`DocsError`."""

    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status = getattr(response, "status", 200)
            geturl = getattr(response, "geturl", None)
            final_url = geturl() if callable(geturl) else (url or "")
            try:
                body = response.read().decode("utf-8")
            except (UnicodeDecodeError, ValueError) as error:
                raise DocsError(f"{url}: live response is not UTF-8 text: {error}") from error
            return FetchedPage(status, body, final_url or url)
    except urllib.error.HTTPError as error:
        return FetchedPage(error.code, "", url)
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        raise DocsError(f"{url}: live request failed: {error}") from error


def get_with_retries(
    url: str,
    *,
    retries: int = DEFAULT_RETRIES,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
    sleep: Callable[[float], None] = time.sleep,
    fetch: Callable[..., FetchedPage] = fetch_text,
) -> FetchedPage:
    """GET with bounded retries on transport failures and HTTP 5xx.

    HTTP 4xx responses are definitive and never retried. Raises
    :class:`DocsError` when every attempt fails.
    """

    if retries < 0:
        raise DocsError("--retries must be zero or more")
    attempts = retries + 1
    last_error: DocsError | None = None
    for attempt in range(1, attempts + 1):
        try:
            page = fetch(url, timeout=timeout)
        except DocsError as error:
            last_error = error
            page = None
        else:
            last_error = None
            if page.status == 200 or not 500 <= page.status < 600:
                return page
            if attempt == attempts:
                return page
        if attempt < attempts:
            sleep(min(2.0 * attempt, 10.0))
    assert last_error is not None
    raise DocsError(f"{url}: live request failed after {attempts} attempt(s): {last_error}")


def _live_fragment_present(html: str, fragment: str) -> bool:
    """Return whether live HTML contains the curated anchor."""

    candidates = {fragment, urllib.parse.unquote(fragment)}
    for candidate in candidates:
        if not candidate:
            continue
        escaped = re.escape(candidate)
        if re.search(rf'(?:id|name)="{escaped}"', html):
            return True
    return False


def _read_expected_urls(site_urls_file: Path) -> list[str]:
    try:
        lines = site_urls_file.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise DocsError(f"cannot read expected site URLs {site_urls_file}: {error}") from error
    urls = [line.strip() for line in lines if line.strip()]
    if not urls:
        raise DocsError(f"expected site URLs {site_urls_file} is empty")
    return urls


def _read_expected_publication(expected_file: Path) -> dict[str, Any]:
    import json

    try:
        expected = json.loads(expected_file.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise DocsError(
            f"expected publication {expected_file} does not exist"
        ) from None
    except (OSError, UnicodeError, ValueError) as error:
        raise DocsError(
            f"expected publication {expected_file} is unreadable or invalid JSON: {error}"
        ) from error
    if not isinstance(expected, dict):
        raise DocsError(f"expected publication {expected_file} must be a JSON object")
    sources = expected.get("sources")
    if not isinstance(sources, dict) or not sources:
        raise DocsError(f"expected publication {expected_file}: sources must be a nonempty mapping")
    fingerprint = expected.get("fingerprint")
    if not isinstance(fingerprint, str) or not fingerprint:
        raise DocsError(f"expected publication {expected_file}: fingerprint must be nonempty")
    return expected


def verify_live(
    *,
    origin: str,
    expected_urls: list[str],
    curated_fragments: list[str],
    expected_sources: dict[str, Any],
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
    retries: int = DEFAULT_RETRIES,
    sleep: Callable[[float], None] = time.sleep,
    fetch: Callable[..., FetchedPage] = fetch_text,
) -> list[str]:
    """Check the live URL set, curated fragments, and build provenance."""

    failures: list[str] = []
    pages: dict[str, str] = {}

    def live_page(url: str) -> str | None:
        if url in pages:
            return pages[url]
        try:
            page = get_with_retries(url, retries=retries, timeout=timeout, sleep=sleep, fetch=fetch)
        except DocsError as error:
            failures.append(str(error))
            return None
        if page.status != 200:
            failures.append(f"{url}: live page answered HTTP {page.status}, expected 200")
            return None
        try:
            final_origin = site_origin(page.final_url)
        except DocsError as error:
            failures.append(f"{url}: live page redirected off the origin: {error}")
            return None
        if final_origin != origin:
            failures.append(
                f"{url}: live page redirected to origin {final_origin}, expected {origin}"
            )
            return None
        pages[url] = page.body
        return page.body

    for url in expected_urls:
        live_page(url)

    for contracted in curated_fragments:
        parsed = urllib.parse.urlsplit(contracted)
        fragment = parsed.fragment
        if not fragment:
            continue
        page_url = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "", ""))
        body = live_page(page_url)
        if body is not None and not _live_fragment_present(body, fragment):
            failures.append(f"{contracted}: live page has no #{fragment} anchor")

    try:
        manifest_page = get_with_retries(
            f"{origin}/build.json", retries=retries, timeout=timeout, sleep=sleep, fetch=fetch
        )
    except DocsError as error:
        failures.append(str(error))
    else:
        if manifest_page.status != 200:
            failures.append(
                f"{origin}/build.json: answered HTTP {manifest_page.status}, expected 200"
            )
        else:
            import json

            try:
                manifest = json.loads(manifest_page.body)
            except ValueError as error:
                failures.append(f"{origin}/build.json: live manifest is not JSON: {error}")
            else:
                failures.extend(_compare_provenance(origin, manifest, expected_sources))
    return failures


def _compare_provenance(
    origin: str, manifest: Any, expected_sources: dict[str, Any]
) -> list[str]:
    failures: list[str] = []
    if not isinstance(manifest, dict) or not isinstance(manifest.get("sources"), dict):
        return [f"{origin}/build.json: live manifest has no sources mapping"]
    live_sources = manifest["sources"]
    for repo in sorted(expected_sources):
        expected = expected_sources[repo]
        expected_sha = expected.get("sha") if isinstance(expected, dict) else None
        live = live_sources.get(repo)
        if not isinstance(live, dict):
            failures.append(f"{origin}/build.json: live manifest is missing source {repo!r}")
            continue
        if live.get("sha") != expected_sha:
            failures.append(
                f"{origin}/build.json: source {repo!r} is {live.get('sha')!r}, "
                f"expected {expected_sha!r}"
            )
        if live.get("dirty"):
            failures.append(f"{origin}/build.json: source {repo!r} is dirty in the live build")
    return failures


def run_publish_verify(
    root: Path,
    *,
    origin: str,
    site_urls_file: Path,
    expected_file: Path,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
    retries: int = DEFAULT_RETRIES,
) -> int:
    """Verify the deployed site; return 0 only when every check passes."""

    repo_root = Path(root).resolve()
    origin = site_origin(origin)
    expected_urls = _read_expected_urls(site_urls_file)
    for url in expected_urls:
        try:
            if site_origin(url) != origin:
                raise DocsError(f"expected site URL {url!r} is outside {origin}")
        except DocsError as error:
            raise DocsError(f"expected site URLs {site_urls_file}: {error}") from error
    expected = _read_expected_publication(expected_file)
    status, payload = fetch_published_urls(origin, timeout=timeout)
    if status != "ok" or not isinstance(payload, list):
        print(f"verify: live {origin}/urls.txt is {status} ({payload}); failing the deploy")
        return 1
    curated = load_url_contract(repo_root, origin)
    failures = verify_live(
        origin=origin,
        expected_urls=expected_urls,
        curated_fragments=curated,
        expected_sources=expected["sources"],
        timeout=timeout,
        retries=retries,
    )
    if failures:
        print(f"verify: {len(failures)} live check(s) failed:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(
        f"verify: OK ({len(expected_urls)} live pages, "
        f"{len([u for u in curated if '#' in u])} curated fragments, provenance matches)"
    )
    return 0

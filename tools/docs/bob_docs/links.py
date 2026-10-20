"""Pinned-lychee acquisition and the built-site link checks.

Lychee resolves from ``$LYCHEE``, then ``PATH``, then a pinned upstream
release downloaded into ``~/.cache/bob-docs/bin/`` (``$XDG_CACHE_HOME`` aware).
Every downloaded asset is SHA-256 verified before extraction or execution, and
unsupported platforms fail with the supported list. A missing checker is an
error, never a green skip.

The offline run checks the built site with the site directory as root and the
absolute org-root URLs remapped onto local output. Fragment checking uses
``--include-fragments --index-files index.html`` so directory URLs such as
``/page/#anchor`` resolve against each directory's ``index.html``. The
``urls`` resolver remains the mention, curated, and live-URL gate. A separate
offline edit-link gate (``run_edit_links``) proves every rendered
``rel="edit"`` href targets a real source file.

The online run keeps the same root, remap, and fragment checking and additionally
fetches external HTTP(S) destinations with bounded retries and timeouts. It adds
two narrowly scoped ``--exclude`` entries: the vendored OFL boilerplate Adobe
URL and the rendered GitHub edit links (verified offline instead);
any other broken external link fails rather than turning green.
"""

from __future__ import annotations

import hashlib
import html as html_module
import os
import platform
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from .context import DocsContext, context_from_root
from .errors import DocsError
from .urls import DOWNLOAD_TIMEOUT_SECONDS, site_origin

LYCHEE_ENV_VAR = "LYCHEE"
LYCHEE_BIN_DIRNAME = "bin"
LYCHEE_METADATA_FILE = "lychee.yml"
LYCHEE_CHECKSUM_SUFFIX = ".sha256"
ONLINE_TIMEOUT_SECONDS = 20
ONLINE_MAX_RETRIES = 3
ONLINE_RETRY_WAIT_SECONDS = 1
# llmstxt emits a Markdown copy at every directory URL. Its copy format keeps
# authored fragment syntax, which lychee parses differently from the checked
# HTML page and reports as false missing anchors. The published HTML remains
# fully checked, along with llms.txt and its page links.
LLMS_PAGE_MARKDOWN_EXCLUDE = "(^|/)index[.]md$"
# The vendored Source Serif OFL text (docs/assets/fonts/OFL.txt) carries
# Adobe's copyright line as http://www.adobe.com/. Adobe returns 403 to
# automated clients, so the weekly online check excludes exactly that URL.
# Other OFL.txt links, such as scripts.sil.org/OFL, stay checked.
ADOBE_OFL_URL_EXCLUDE = "^http://www[.]adobe[.]com/?$"
# Anonymous GitHub edit URLs always answer 302 -> /login, even for a branch
# or path that does not exist, so fetching them online proves only that the
# repository exists. Weekly runs 39475174066 and 39499697687 each failed on
# four different 429s for these edit URLs with zero other errors. The online
# check therefore excludes rendered edit links; run_edit_links below verifies
# each one offline against its source checkout instead.
EDIT_LINK_EXCLUDE = "^https://github[.]com/bobs-org/(bob-cli|bob-plugins|bob-mac-capture)/edit/"


@dataclass(frozen=True)
class LycheePlatform:
    key: str
    target: str
    asset: str
    sha256: str


@dataclass(frozen=True)
class LycheeRelease:
    version: str
    tag: str
    base_url: str
    binary: str
    platforms: dict[str, LycheePlatform]


def load_lychee_metadata(root: Path) -> LycheeRelease:
    """Load the pinned lychee version, tag, assets, and checksums."""

    from .yaml_tools import load_yaml

    path = root / "docs/_meta" / LYCHEE_METADATA_FILE
    raw = load_yaml(path)
    if not isinstance(raw, dict):
        raise DocsError(f"{path}: expected a mapping")
    for key in ("version", "tag", "base_url", "binary", "platforms"):
        if not isinstance(raw.get(key), str if key != "platforms" else dict) or not raw.get(key):
            raise DocsError(f"{path}: {key} must be a nonempty {('mapping' if key == 'platforms' else 'string')}")
    platforms: dict[str, LycheePlatform] = {}
    for key, entry in raw["platforms"].items():
        if not isinstance(entry, dict):
            raise DocsError(f"{path}: platforms.{key} must be a mapping")
        for field in ("target", "asset", "sha256"):
            value = entry.get(field)
            if not isinstance(value, str) or not value.strip():
                raise DocsError(
                    f"{path}: platforms.{key}.{field} must be a nonempty string "
                    "(quote checksums so YAML does not parse them as numbers)"
                )
        digest = entry["sha256"].strip().lower()
        if len(digest) != 64 or any(char not in "0123456789abcdef" for char in digest):
            raise DocsError(f"{path}: platforms.{key}.sha256 must be 64 lowercase hex characters")
        platforms[key] = LycheePlatform(
            key=key, target=entry["target"], asset=entry["asset"], sha256=digest
        )
    if not platforms:
        raise DocsError(f"{path}: platforms must list at least one platform")
    return LycheeRelease(
        version=raw["version"].strip(),
        tag=raw["tag"].strip(),
        base_url=raw["base_url"].strip().rstrip("/"),
        binary=raw["binary"].strip(),
        platforms=platforms,
    )


def current_platform_key(
    system: str | None = None, machine: str | None = None
) -> str:
    """Return the metadata key for this OS/CPU, or raise on unsupported platforms."""

    os_name = (system or platform.system()).lower()
    cpu = (machine or platform.machine()).lower()
    normalised_cpu = {"x86_64": "x86_64", "amd64": "x86_64"}.get(cpu, cpu)
    if os_name == "linux" and normalised_cpu in ("x86_64", "aarch64"):
        return f"linux-{normalised_cpu}"
    if os_name == "darwin" and normalised_cpu in ("arm64", "aarch64"):
        return "darwin-arm64"
    raise DocsError(
        f"unsupported platform {os_name}/{cpu}: pinned lychee builds exist for "
        "linux-x86_64, linux-aarch64, and darwin-arm64"
    )


def cache_bin_dir(cache_base: Path | None = None) -> Path:
    """Return ``~/.cache/bob-docs/bin/`` (``$XDG_CACHE_HOME`` aware)."""

    from .mounts import cache_root

    if cache_base is not None:
        return Path(cache_base) / LYCHEE_BIN_DIRNAME
    return cache_root() / LYCHEE_BIN_DIRNAME


def _is_usable_executable(path: Path) -> bool:
    return path.is_file() and not path.is_symlink() and os.access(path, os.X_OK)


def _version_matches(binary: Path, version: str) -> bool:
    try:
        result = subprocess.run(
            [str(binary), "--version"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0 and result.stdout.strip().split()[:2] == ["lychee", version]


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def resolve_lychee(
    root: Path,
    *,
    env: Mapping[str, str] | None = None,
    path_lookup: Any = None,
    cache_base: Path | None = None,
    system: str | None = None,
    machine: str | None = None,
) -> Path:
    """Return a usable pinned lychee binary, downloading it when needed."""

    environment = os.environ if env is None else env
    metadata = load_lychee_metadata(root)
    override = environment.get(LYCHEE_ENV_VAR)
    if override:
        candidate = Path(override)
        if not candidate.is_file():
            raise DocsError(
                f"${LYCHEE_ENV_VAR}={override} is not a file; point it at a lychee executable"
            )
        if not os.access(candidate, os.X_OK):
            raise DocsError(f"${LYCHEE_ENV_VAR}={override} is not executable")
        return candidate.resolve()
    lookup = shutil.which if path_lookup is None else path_lookup
    found = lookup("lychee")
    if found:
        return Path(found).resolve()
    bin_dir = cache_bin_dir(cache_base)
    cached = bin_dir / metadata.binary
    if cached.exists():
        sidecar = cached.with_name(cached.name + LYCHEE_CHECKSUM_SUFFIX)
        expected = sidecar.read_text(encoding="utf-8").strip().split()[0] if sidecar.is_file() else ""
        if (
            _is_usable_executable(cached)
            and expected
            and _sha256_file(cached) == expected.lower()
            and _version_matches(cached, metadata.version)
        ):
            return cached.resolve()
        print(f"cached lychee at {cached} is stale or corrupt; re-acquiring release {metadata.tag}")
        try:
            cached.unlink()
        except OSError:
            pass
        try:
            sidecar.unlink()
        except OSError:
            pass
    return _download_lychee(metadata, bin_dir, system=system, machine=machine)


def _download_lychee(
    metadata: LycheeRelease,
    bin_dir: Path,
    *,
    system: str | None = None,
    machine: str | None = None,
) -> Path:
    key = current_platform_key(system, machine)
    entry = metadata.platforms.get(key)
    if entry is None:
        raise DocsError(
            f"pinned lychee {metadata.tag} has no build for {key}; "
            f"supported: {', '.join(sorted(metadata.platforms))}"
        )
    url = f"{metadata.base_url}/{metadata.tag}/{entry.asset}"
    print(f"downloading lychee {metadata.tag} for {key}")
    bin_dir.mkdir(parents=True, exist_ok=True)
    tmp_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=bin_dir, prefix=".lychee-download-", suffix=".tar.gz", delete=False
        ) as handle:
            tmp_path = Path(handle.name)
        request = urllib.request.Request(url, headers={"User-Agent": "bob-docs/links"})
        digest = hashlib.sha256()
        try:
            with urllib.request.urlopen(request, timeout=DOWNLOAD_TIMEOUT_SECONDS) as response:
                while True:
                    chunk = response.read(1024 * 1024)
                    if not chunk:
                        break
                    digest.update(chunk)
                    assert tmp_path is not None
                    with tmp_path.open("ab") as out:
                        out.write(chunk)
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            raise DocsError(
                f"cannot download lychee {metadata.tag} for {key} from {url}: {error}; "
                "a missing checker is not a green skip"
            ) from error
        if digest.hexdigest() != entry.sha256:
            raise DocsError(
                f"checksum mismatch for lychee {metadata.tag} {entry.asset}: expected "
                f"{entry.sha256}, got {digest.hexdigest()}; refusing a corrupt binary"
            )
        binary = _extract_lychee(tmp_path, bin_dir, metadata.binary)
    finally:
        if tmp_path is not None:
            try:
                tmp_path.unlink()
            except OSError:
                pass
    sidecar = binary.with_name(binary.name + LYCHEE_CHECKSUM_SUFFIX)
    sidecar.write_text(f"{_sha256_file(binary)}  {binary.name}\n", encoding="utf-8")
    if not _version_matches(binary, metadata.version):
        raise DocsError(
            f"downloaded lychee at {binary} does not report version {metadata.version}"
        )
    return binary.resolve()


def _extract_lychee(archive: Path, bin_dir: Path, binary_name: str) -> Path:
    try:
        with tarfile.open(archive, "r:gz") as tar:
            members = tar.getmembers()
            candidate = next(
                (
                    member
                    for member in members
                    if member.isfile() and Path(member.name).name == binary_name
                ),
                None,
            )
            if candidate is None:
                raise DocsError(
                    f"lychee archive {archive.name} contains no {binary_name} executable"
                )
            if candidate.name.startswith("/") or ".." in Path(candidate.name).parts:
                raise DocsError(f"lychee archive {archive.name} has an unsafe member {candidate.name!r}")
            tar.extract(candidate, path=bin_dir, filter="data")
            extracted = bin_dir / candidate.name
    except tarfile.TarError as error:
        raise DocsError(f"lychee archive {archive.name} is corrupt: {error}") from error
    destination = bin_dir / binary_name
    if extracted != destination:
        if destination.exists():
            destination.unlink()
        extracted.rename(destination)
        top = bin_dir / Path(candidate.name).parts[0]
        try:
            top.rmdir()
        except OSError:
            pass
    destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return destination


def lychee_argv(lychee: Path, *, site_dir: Path, origin: str) -> list[str]:
    """Exact offline invocation: built dir as root, org-root URLs remapped local."""

    site_uri = site_dir.resolve().as_uri() + "/"
    return [
        str(lychee),
        "--offline",
        "--no-progress",
        "--include-fragments",
        "--exclude-path",
        LLMS_PAGE_MARKDOWN_EXCLUDE,
        "--index-files",
        "index.html",
        "--root-dir",
        str(site_dir.resolve()),
        "--remap",
        f"{origin}/ {site_uri}",
        str(site_dir.resolve()),
    ]


def online_argv(
    lychee: Path,
    *,
    site_dir: Path,
    origin: str,
    timeout: int = ONLINE_TIMEOUT_SECONDS,
    retries: int = ONLINE_MAX_RETRIES,
    retry_wait: int = ONLINE_RETRY_WAIT_SECONDS,
) -> list[str]:
    """Exact online invocation: same root/remap/fragments, plus live HTTP(S).

    Local file links keep resolving against ``site_dir`` through ``--root-dir``
    and ``--remap``, so authored ``docs/`` paths, source README links, and
    command snippets are not treated as external requests. Two narrowly scoped
    ``--exclude`` entries cover the vendored OFL boilerplate Adobe URL and the
    rendered GitHub edit links (verified offline by ``run_edit_links``);
    other broken externals fail.
    """

    if timeout <= 0:
        raise DocsError("online link timeout must be a positive number of seconds")
    if retries < 0:
        raise DocsError("online link retries must be zero or greater")
    if retry_wait < 0:
        raise DocsError("online link retry wait must be zero or greater")
    site_uri = site_dir.resolve().as_uri() + "/"
    return [
        str(lychee),
        "--no-progress",
        "--include-fragments",
        "--exclude-path",
        LLMS_PAGE_MARKDOWN_EXCLUDE,
        "--index-files",
        "index.html",
        "--root-dir",
        str(site_dir.resolve()),
        "--remap",
        f"{origin}/ {site_uri}",
        "--timeout",
        str(timeout),
        "--max-retries",
        str(retries),
        "--retry-wait-time",
        str(retry_wait),
        "--exclude",
        ADOBE_OFL_URL_EXCLUDE,
        "--exclude",
        EDIT_LINK_EXCLUDE,
        str(site_dir.resolve()),
    ]


def run_links(
    root: Path,
    *,
    lychee: Path | str | None = None,
    context: DocsContext | None = None,
    online: bool = False,
    timeout: int = ONLINE_TIMEOUT_SECONDS,
    retries: int = ONLINE_MAX_RETRIES,
    retry_wait: int = ONLINE_RETRY_WAIT_SECONDS,
) -> int:
    """Run the lychee check over the built site; propagate its status.

    With ``online=False`` (default) only local files are checked. With
    ``online=True`` external HTTP(S) destinations are fetched with bounded
    retries and timeouts while fragment checking stays on the built tree.
    """

    repo_root = Path(root).resolve()
    context = context or context_from_root(repo_root)
    site_url = context.config.get("site_url", "")
    if not isinstance(site_url, str):
        raise DocsError("mkdocs.yml: site_url must be a string")
    origin = site_origin(site_url)
    site_dir = context.site_dir
    if not (site_dir / "index.html").is_file():
        raise DocsError(f"no built site at {site_dir}; run `just docs-build` first")
    binary = Path(lychee) if lychee is not None else resolve_lychee(repo_root)
    if online:
        command = online_argv(
            binary,
            site_dir=site_dir,
            origin=origin,
            timeout=timeout,
            retries=retries,
            retry_wait=retry_wait,
        )
    else:
        command = lychee_argv(binary, site_dir=site_dir, origin=origin)
    print("running: " + " ".join(command))
    try:
        result = subprocess.run(command, cwd=repo_root, check=False)
    except OSError as error:
        raise DocsError(f"could not start {binary}: {error}") from error
    if result.returncode != 0:
        mode = "online" if online else "offline"
        print(f"links: lychee reported broken links (exit {result.returncode}, {mode})")
        return result.returncode
    if online:
        print("links: OK (online lychee with fragment checking)")
    else:
        print("links: OK (offline lychee with fragment checking)")
    return 0


_EDIT_TAG_RE = re.compile(r"<a\b[^>]*>", re.IGNORECASE)
_REL_EDIT_RE = re.compile(r"""\brel\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))""", re.IGNORECASE)
_HREF_RE = re.compile(r"""\bhref\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))""", re.IGNORECASE)
_EDIT_URL_RE = re.compile(
    r"^https://github\.com/(?P<owner>[^/]+)/(?P<repo>[^/]+)/edit/(?P<branch>[^/]+)/(?P<path>.+)$"
)


def _parse_github_repo_url(url: str) -> tuple[str, str]:
    """Split an ``https://github.com/<owner>/<repo>`` URL into its parts."""

    parsed = urllib.parse.urlsplit(url.strip())
    if parsed.scheme != "https" or parsed.hostname != "github.com":
        raise DocsError(f"mkdocs.yml: repo_url must be https://github.com/<owner>/<repo>, got {url!r}")
    parts = [part for part in parsed.path.split("/") if part]
    if len(parts) != 2 or not all(parts):
        raise DocsError(f"mkdocs.yml: repo_url must be https://github.com/<owner>/<repo>, got {url!r}")
    repo = parts[1]
    if repo.endswith(".git"):
        repo = repo[: -len(".git")]
    return parts[0], repo


def _parse_edit_branch(edit_uri: object) -> str:
    """Return the branch embedded in MkDocs ``edit_uri`` (``edit/<branch>/...``)."""

    if not isinstance(edit_uri, str) or not edit_uri:
        raise DocsError("mkdocs.yml: edit_uri must be a nonempty string like 'edit/master/docs/'")
    parts = [part for part in edit_uri.strip().split("/") if part]
    if len(parts) < 2 or parts[0] != "edit" or not parts[1]:
        raise DocsError(f"mkdocs.yml: edit_uri must look like 'edit/<branch>/...', got {edit_uri!r}")
    return parts[1]


def _known_edit_sources(
    repo_root: Path, context: DocsContext
) -> dict[tuple[str, str, str], Path]:
    """Map ``(owner, repo, branch)`` to the source checkout holding its files."""

    from .mounts import load_mounts

    repo_url = context.config.get("repo_url", "")
    edit_uri = context.config.get("edit_uri", "")
    if not isinstance(repo_url, str) or not repo_url.strip():
        raise DocsError("mkdocs.yml: repo_url must be a nonempty string")
    owner, repo = _parse_github_repo_url(repo_url)
    branch = _parse_edit_branch(edit_uri)
    source_dirs = dict(context.source_dirs) if context.source_dirs else {}
    bob_cli_dir = source_dirs.get("bob-cli", repo_root)
    allowed: dict[tuple[str, str, str], Path] = {(owner, repo, branch): Path(bob_cli_dir)}
    for mount in load_mounts(repo_root):
        remote = mount.remote
        if remote.endswith(".git"):
            remote = remote[: -len(".git")]
        mount_owner, mount_repo = _parse_github_repo_url(remote)
        repo_dir = source_dirs.get(mount.repo)
        if repo_dir is None:
            raise DocsError(
                f"edit-link gate needs source checkout for mount repo {mount.repo!r}; "
                "run through `bob-docs check` with an assembled context"
            )
        allowed.setdefault((mount_owner, mount_repo, mount.branch), Path(repo_dir))
    return allowed


def _collect_edit_links(site_dir: Path) -> list[tuple[str, str]]:
    """Collect ``(page, href)`` for every ``rel="edit"`` anchor in the built site."""

    found: list[tuple[str, str]] = []
    for page in sorted(site_dir.rglob("*.html")):
        try:
            text = page.read_text(encoding="utf-8")
        except OSError:
            continue
        for tag in _EDIT_TAG_RE.findall(text):
            rel = _REL_EDIT_RE.search(tag)
            if rel is None:
                continue
            value = rel.group(2) or rel.group(3) or rel.group(4) or ""
            if "edit" not in value.split():
                continue
            href = _HREF_RE.search(tag)
            if href is None:
                continue
            raw = href.group(2) or href.group(3) or href.group(4) or ""
            found.append((page.relative_to(site_dir).as_posix(), html_module.unescape(raw.strip())))
    return found


def run_edit_links(
    root: Path,
    *,
    context: DocsContext | None = None,
) -> int:
    """Verify every rendered ``rel="edit"`` href targets a real source file.

    Each href must parse as
    ``https://github.com/<owner>/<repo>/edit/<branch>/<path>`` where
    owner/repo and branch match a known source (bob-cli from ``repo_url`` and
    ``edit_uri`` in ``mkdocs.yml``; mounts from ``docs/_meta/mounts.yml``),
    and ``<path>`` must exist as a file in that source checkout. Every
    failure is reported with its page; any failure fails the gate.
    """

    repo_root = Path(root).resolve()
    context = context or context_from_root(repo_root)
    site_dir = context.site_dir
    if not (site_dir / "index.html").is_file():
        raise DocsError(f"no built site at {site_dir}; run `just docs-build` first")
    allowed = _known_edit_sources(repo_root, context)
    links = _collect_edit_links(site_dir)
    failures: list[str] = []
    for page, href in links:
        match = _EDIT_URL_RE.match(href)
        if match is None:
            failures.append(f"{page}: edit href does not parse as a GitHub edit URL: {href!r}")
            continue
        key = (match.group("owner"), match.group("repo"), match.group("branch"))
        repo_dir = allowed.get(key)
        if repo_dir is None:
            known = ", ".join(f"{o}/{r}@{b}" for o, r, b in sorted(allowed))
            failures.append(
                f"{page}: unknown edit source {key[0]}/{key[1]}@{key[2]} in {href!r} "
                f"(known: {known})"
            )
            continue
        relative = match.group("path").split("?", 1)[0].split("#", 1)[0]
        relative = urllib.parse.unquote(relative).strip()
        if not relative or ".." in Path(relative).parts:
            failures.append(f"{page}: edit href has an unsafe path: {href!r}")
            continue
        if not (repo_dir / relative).is_file():
            failures.append(f"{page}: edit target is missing: {href!r} (no {repo_dir / relative})")
    if failures:
        for failure in failures:
            print(f"edit-links: FAILED {failure}")
        print(f"edit-links: FAILED ({len(failures)} bad edit link(s) on {len(links)} edit link(s))")
        return 1
    print(f"edit-links: OK ({len(links)} edit link(s) point at real source files)")
    return 0

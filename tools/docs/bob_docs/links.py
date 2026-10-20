"""Pinned-lychee acquisition and the offline built-site link check.

Lychee resolves from ``$LYCHEE``, then ``PATH``, then a pinned upstream
release downloaded into ``~/.cache/bob-docs/bin/`` (``$XDG_CACHE_HOME`` aware).
Every downloaded asset is SHA-256 verified before extraction or execution, and
unsupported platforms fail with the supported list. A missing checker is an
error, never a green skip.

The offline run checks the built site with the site directory as root and the
absolute org-root URLs remapped onto local output. Fragment checking is
deliberately left to :mod:`bob_docs.urls`: lychee 0.24.2 cannot check
fragments on ``file://`` directory URLs (every ``/page/#anchor`` link reports
``Cannot find fragment`` even when the anchor exists; verified against the
real strict build), while the ``urls`` resolver indexes directory URLs, index
pages, and redirect destinations precisely. The online weekly publish phase
can use ``--include-fragments`` over HTTP, where the same links resolve.
"""

from __future__ import annotations

import hashlib
import os
import platform
import shutil
import stat
import subprocess
import tarfile
import tempfile
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
        "--root-dir",
        str(site_dir.resolve()),
        "--remap",
        f"{origin}/ {site_uri}",
        str(site_dir.resolve()),
    ]


def run_links(
    root: Path, *, lychee: Path | str | None = None, context: DocsContext | None = None
) -> int:
    """Run the offline lychee check over the built site; propagate its status."""

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
    command = lychee_argv(binary, site_dir=site_dir, origin=origin)
    print("running: " + " ".join(command))
    try:
        result = subprocess.run(command, cwd=repo_root, check=False)
    except OSError as error:
        raise DocsError(f"could not start {binary}: {error}") from error
    if result.returncode != 0:
        print(f"links: lychee reported broken links (exit {result.returncode})")
        return result.returncode
    print("links: OK (offline lychee; fragments are checked by `bob-docs urls`)")
    return 0

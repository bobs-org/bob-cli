import hashlib
import stat
import tarfile

import pytest

from bob_docs import links
from bob_docs.errors import DocsError
from bob_docs.links import (
    cache_bin_dir,
    current_platform_key,
    load_lychee_metadata,
    lychee_argv,
    resolve_lychee,
    run_links,
)


def _write_metadata(root, *, base_url, tag="test-tag", asset="lychee-test.tar.gz", sha="0" * 64):
    from pathlib import Path

    assert isinstance(root, Path)
    meta = root / "docs/_meta"
    meta.mkdir(parents=True, exist_ok=True)
    (meta / "lychee.yml").write_text(
        f"""\
version: 9.9.9
tag: {tag}
base_url: {base_url}
binary: lychee
platforms:
  linux-x86_64:
    target: x86_64-unknown-linux-gnu
    asset: {asset}
    sha256: '{sha}'
""",
        encoding="utf-8",
    )
    return root


def _executable(path, body='#!/bin/sh\necho "lychee 9.9.9"\n'):
    from pathlib import Path

    assert isinstance(path, Path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def _tarball(directory, archive, top="lychee-test"):
    from pathlib import Path

    assert isinstance(directory, Path)
    assert isinstance(archive, Path)
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(directory / top, arcname=top)
    digest = hashlib.sha256()
    digest.update(archive.read_bytes())
    return digest.hexdigest()


def test_load_lychee_metadata_validates(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    _write_metadata(tmp_path, base_url="https://example.test/releases")
    release = load_lychee_metadata(tmp_path)
    assert release.version == "9.9.9"
    assert release.platforms["linux-x86_64"].asset == "lychee-test.tar.gz"

    meta = tmp_path / "docs/_meta/lychee.yml"
    meta.write_text("version: 9.9.9\n", encoding="utf-8")
    with pytest.raises(DocsError, match="tag must be"):
        load_lychee_metadata(tmp_path)
    _write_metadata(tmp_path, base_url="https://example.test", sha="xyz")
    with pytest.raises(DocsError, match="64 lowercase hex"):
        load_lychee_metadata(tmp_path)


def test_current_platform_key_supports_planned_targets() -> None:
    assert current_platform_key("Linux", "x86_64") == "linux-x86_64"
    assert current_platform_key("Linux", "aarch64") == "linux-aarch64"
    assert current_platform_key("Darwin", "arm64") == "darwin-arm64"
    with pytest.raises(DocsError, match="unsupported platform"):
        current_platform_key("Windows", "AMD64")
    with pytest.raises(DocsError, match="unsupported platform"):
        current_platform_key("Linux", "i686")


def test_resolve_lychee_prefers_env_then_path(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _write_metadata(tmp_path, base_url="https://example.test")
    explicit = _executable(tmp_path / "custom-lychee")
    assert resolve_lychee(root, env={"LYCHEE": str(explicit)}) == explicit.resolve()
    with pytest.raises(DocsError, match="is not a file"):
        resolve_lychee(root, env={"LYCHEE": str(tmp_path / "absent")})
    plain = tmp_path / "plain"
    plain.write_text("x", encoding="utf-8")
    with pytest.raises(DocsError, match="is not executable"):
        resolve_lychee(root, env={"LYCHEE": str(plain)})
    assert (
        resolve_lychee(root, env={}, path_lookup=lambda name: "/usr/bin/lychee")
        == Path("/usr/bin/lychee").resolve()
    )


def test_resolve_lychee_reuses_verified_cache(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _write_metadata(tmp_path, base_url="https://example.test")
    cache = tmp_path / "cache"
    binary = _executable(cache / "bin" / "lychee")
    digest = hashlib.sha256()
    digest.update(binary.read_bytes())
    (cache / "bin" / "lychee.sha256").write_text(f"{digest.hexdigest()}  lychee\n", encoding="utf-8")
    assert cache_bin_dir(cache) == cache / "bin"
    assert (
        resolve_lychee(root, env={}, path_lookup=lambda name: None, cache_base=cache)
        == binary.resolve()
    )


def test_resolve_lychee_downloads_and_verifies(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    pkg = tmp_path / "pkg"
    tagged = pkg / "test-tag"
    tagged.mkdir(parents=True)
    top = pkg / "lychee-test"
    top.mkdir(parents=True)
    _executable(top / "lychee")
    archive = tagged / "lychee-test.tar.gz"
    sha = _tarball(pkg, archive)
    root = _write_metadata(tmp_path, base_url=pkg.as_uri(), sha=sha)
    cache = tmp_path / "cache"
    binary = resolve_lychee(
        root, env={}, path_lookup=lambda name: None, cache_base=cache, system="Linux", machine="x86_64"
    )
    assert binary == (cache / "bin" / "lychee").resolve()
    assert (cache / "bin" / "lychee.sha256").is_file()
    import subprocess

    completed = subprocess.run([str(binary), "--version"], text=True, capture_output=True)
    assert "lychee 9.9.9" in completed.stdout


def test_resolve_lychee_rejects_bad_checksum_and_platform(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    pkg = tmp_path / "pkg"
    tagged = pkg / "test-tag"
    tagged.mkdir(parents=True)
    top = pkg / "lychee-test"
    top.mkdir(parents=True)
    _executable(top / "lychee")
    archive = tagged / "lychee-test.tar.gz"
    _tarball(pkg, archive)
    root = _write_metadata(tmp_path, base_url=pkg.as_uri(), sha="1" * 64)
    with pytest.raises(DocsError, match="checksum mismatch"):
        resolve_lychee(
            root, env={}, path_lookup=lambda name: None,
            cache_base=tmp_path / "cache", system="Linux", machine="x86_64",
        )
    with pytest.raises(DocsError, match="unsupported platform"):
        resolve_lychee(
            root, env={}, path_lookup=lambda name: None,
            cache_base=tmp_path / "cache2", system="Windows", machine="AMD64",
        )


def test_resolve_lychee_reacquires_corrupt_cache(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    pkg = tmp_path / "pkg"
    tagged = pkg / "test-tag"
    tagged.mkdir(parents=True)
    top = pkg / "lychee-test"
    top.mkdir(parents=True)
    _executable(top / "lychee")
    archive = tagged / "lychee-test.tar.gz"
    sha = _tarball(pkg, archive)
    root = _write_metadata(tmp_path, base_url=pkg.as_uri(), sha=sha)
    cache = tmp_path / "cache"
    (cache / "bin").mkdir(parents=True)
    _executable(cache / "bin" / "lychee", body="#!/bin/sh\necho tampered\n")
    (cache / "bin" / "lychee.sha256").write_text(f"{'0' * 64}  lychee\n", encoding="utf-8")
    binary = resolve_lychee(
        root, env={}, path_lookup=lambda name: None, cache_base=cache, system="Linux", machine="x86_64"
    )
    assert "tampered" not in (cache / "bin" / "lychee").read_text(encoding="utf-8")
    assert binary == (cache / "bin" / "lychee").resolve()


def test_lychee_argv_uses_offline_root_and_remap(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    site = tmp_path / "site"
    site.mkdir()
    argv = lychee_argv(tmp_path / "lychee", site_dir=site, origin="https://docs.example.test")
    assert argv == [
        str(tmp_path / "lychee"),
        "--offline",
        "--no-progress",
        "--include-fragments",
        "--exclude-path",
        "(^|/)index[.]md$",
        "--index-files",
        "index.html",
        "--root-dir",
        str(site.resolve()),
        "--remap",
        f"https://docs.example.test/ {site.resolve().as_uri()}/",
        str(site.resolve()),
    ]
    assert argv[argv.index("--exclude-path") + 2] == "--index-files"
    assert argv[argv.index("--index-files") + 1] == "index.html"


def _links_root(root, origin="https://docs.example.test"):
    from pathlib import Path

    assert isinstance(root, Path)
    (root / "docs").mkdir(parents=True, exist_ok=True)
    (root / "mkdocs.yml").write_text(
        f"site_name: Test\nsite_url: {origin}/\ndocs_dir: docs\nsite_dir: target/docs/site\n",
        encoding="utf-8",
    )
    site = root / "target/docs/site"
    site.mkdir(parents=True)
    (site / "index.html").write_text("<html></html>", encoding="utf-8")
    return root


def test_run_links_propagates_checker_status(tmp_path, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _links_root(tmp_path)
    failing = _executable(tmp_path / "lychee-fail", "#!/bin/sh\necho lychee-mock\nexit 3\n")
    assert run_links(root, lychee=failing) == 3
    assert "exit 3" in capsys.readouterr().out
    passing = _executable(tmp_path / "lychee-ok", "#!/bin/sh\nexit 0\n")
    assert run_links(root, lychee=passing) == 0
    assert "offline lychee with fragment checking" in capsys.readouterr().out


def test_run_links_requires_built_site(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    (tmp_path / "docs").mkdir()
    (tmp_path / "mkdocs.yml").write_text(
        "site_name: Test\nsite_url: https://docs.example.test/\n"
        "docs_dir: docs\nsite_dir: target/docs/site\n",
        encoding="utf-8",
    )
    with pytest.raises(DocsError, match="no built site"):
        run_links(tmp_path, lychee=tmp_path / "lychee")


def test_run_links_uses_pinned_binary_without_options(tmp_path, monkeypatch) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _links_root(tmp_path)
    _write_metadata(root, base_url="https://example.test")
    seen = {}

    def fake_resolve(repo_root, **kwargs):
        seen["root"] = repo_root
        assert kwargs.get("env", None) is None or True
        return tmp_path / "pinned-lychee"

    monkeypatch.setattr(links, "resolve_lychee", fake_resolve)

    class _Completed:
        returncode = 0

    def fake_run(command, **kwargs):
        seen["argv"] = command
        return _Completed()

    monkeypatch.setattr(links.subprocess, "run", fake_run)
    assert run_links(root) == 0
    assert seen["root"] == root.resolve()
    assert seen["argv"][0] == str(tmp_path / "pinned-lychee")

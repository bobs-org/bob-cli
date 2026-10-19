"""Fixture-repository tests for mount discovery, copying, nav, and production."""

import json
import subprocess
from pathlib import Path

import pytest

from bob_docs.assemble import assemble
from bob_docs.errors import DocsError
from bob_docs.mounts import (
    Mount,
    discover_repo,
    env_var_for,
    load_mounts,
    stamp_markdown,
)


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _mounts_yaml(entries: list[dict[str, str]]) -> str:
    lines = []
    for entry in entries:
        lines.append(f"- repo: {entry['repo']}")
        for key in ("remote", "branch", "source", "target", "nav_title"):
            lines.append(f"  {key}: {entry[key]}")
    return "\n".join(lines) + "\n"


def _minimal_root(
    root: Path,
    *,
    mounts: str = "[]\n",
    nav_extra: str = "",
) -> Path:
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/develop").mkdir(parents=True)
    (root / "docs/overrides").mkdir(parents=True)
    _write(root / "docs/index.md", "# Home\n")
    _write(root / "docs/only.md", "# Only\n")
    _write(
        root / "docs/_meta/contracts.yml",
        "groups:\n  - title: Group\n    entries:\n"
        "      - stem: only\n        summary: One contract\n        order: 1\n",
    )
    _write(root / "docs/_meta/mounts.yml", mounts)
    _write(
        root / "mkdocs.yml",
        "site_name: Test\ndocs_dir: docs\nsite_dir: target/docs/site\n"
        "theme:\n  name: material\n  custom_dir: docs/overrides\n"
        "markdown_extensions:\n  - custom:\n"
        "      format: !!python/name:bob_docs.fences.fence_format ''\n"
        "nav:\n  - Home: index.md\n" + nav_extra,
    )
    return root


def _plugin_repo(repo: Path, *, with_index: bool = False) -> Path:
    _write(repo / "docs/guide.md", "# Guide\n\nBody text.\n")
    _write(
        repo / "docs/fm.md",
        "---\ntitle: FM\ndescription: Has front matter.\n---\n\n# FM\n",
    )
    _write(repo / "docs/nav.yml", "- title: Guide\n  path: guide.md\n- title: FM\n  path: fm.md\n")
    _write(repo / "docs/media.yml", "assets: []\n")
    _write(repo / "docs/.hidden.md", "# Hidden\n")
    if with_index:
        _write(repo / "docs/index.md", "# Source index\n")
    return repo


def _mount_entry(repo_dir: Path, *, repo: str = "bob-plugins", target: str = "reference/plugins") -> dict[str, str]:
    return {
        "repo": repo,
        "remote": repo_dir.as_uri(),
        "branch": "main",
        "source": "docs",
        "target": target,
        "nav_title": "Plugins",
    }


def test_env_var_naming() -> None:
    assert env_var_for("bob-plugins") == "BOB_DOCS_BOB_PLUGINS_DIR"
    assert env_var_for("bob-mac-capture") == "BOB_DOCS_BOB_MAC_CAPTURE_DIR"


def _probe() -> Mount:
    from pathlib import PurePosixPath

    return Mount(
        repo="bob-plugins", remote="x", branch="main",
        source=PurePosixPath("docs"), target=PurePosixPath("reference/plugins"),
        nav_title="Plugins",
    )


def test_discovery_prefers_env_over_sibling(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    root = _minimal_root(tmp_path / "root")
    sibling = tmp_path / "bob-plugins"
    sibling.mkdir()
    winner = tmp_path / "winner"
    winner.mkdir()
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(winner))
    found, _ = discover_repo(_probe(), root=root)
    assert found == winner.resolve()


def test_discovery_sibling_before_linked(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    root = _minimal_root(tmp_path / "root")
    sibling = tmp_path / "bob-plugins"
    sibling.mkdir()
    linked = root / "sase/repos/linked/bob-plugins"
    linked.mkdir(parents=True)
    found, _ = discover_repo(_probe(), root=root)
    assert found == sibling.resolve()


def test_discovery_linked_when_no_sibling(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    root = _minimal_root(tmp_path / "root")
    linked = root / "sase/repos/linked/bob-plugins"
    linked.mkdir(parents=True)
    found, _ = discover_repo(_probe(), root=root)
    assert found == linked.resolve()


def test_invalid_env_override_fails_without_fallthrough(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _minimal_root(tmp_path / "root")
    sibling = tmp_path / "bob-plugins"
    sibling.mkdir()
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(tmp_path / "missing"))
    with pytest.raises(DocsError, match="never falls through"):
        discover_repo(_probe(), root=root)


def test_discovery_failure_reports_attempted_locations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    import dataclasses

    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote="/nonexistent/remote")
    with pytest.raises(DocsError, match="attempted"):
        discover_repo(probe, root=root)


def test_full_mount_copies_with_provenance_and_nav(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "---\ntitle: Plugins hub\n---\n\n# Hub\n")
    context, _ = assemble(root)
    staged = context.output_dir / "docs"
    guide = staged / "reference/plugins/guide.md"
    assert guide.is_file()
    text = guide.read_text(encoding="utf-8")
    assert text.startswith("---\nsource: bob-plugins:docs/guide.md\n---\n")
    assert "Body text." in text
    fm = (staged / "reference/plugins/fm.md").read_text(encoding="utf-8")
    assert fm.startswith("---\nsource: bob-plugins:docs/fm.md\n")
    assert "title: FM" in fm
    assert not (staged / "reference/plugins/nav.yml").exists()
    assert not (staged / "reference/plugins/media.yml").exists()
    assert not (staged / "reference/plugins/.hidden.md").exists()
    from bob_docs.yaml_tools import InertPythonName, load_yaml

    config = load_yaml(context.assembled_config)
    assert config["markdown_extensions"][0]["custom"]["format"] == InertPythonName(
        "tag:yaml.org,2002:python/name:bob_docs.fences.fence_format", ""
    )
    develop = config["nav"]
    plugins = next(item for item in develop if isinstance(item, dict) and "Plugins" in item)
    children = plugins["Plugins"]
    assert children[0] == {"Plugins hub": "reference/plugins/index.md"}
    assert children[1] == {"Guide": "reference/plugins/guide.md"}
    assert children[2] == {"FM": "reference/plugins/fm.md"}


def test_nested_sources_partition_by_most_specific_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = tmp_path / "bob-plugins"
    _write(repo / "docs/overview.md", "# Overview\n")
    _write(repo / "docs/develop/setup.md", "# Setup\n")
    _write(repo / "docs/nav.yml", "- title: Overview\n  path: overview.md\n")
    _write(repo / "docs/develop/nav.yml", "- title: Setup\n  path: setup.md\n")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([
            {**_mount_entry(repo, target="reference/plugins"), "nav_title": "Reference"},
            {
                "repo": "bob-plugins",
                "remote": repo.as_uri(),
                "branch": "main",
                "source": "docs/develop",
                "target": "develop/plugins",
                "nav_title": "Develop",
            },
        ]),
        nav_extra="  - Reference: reference/plugins/index.md\n  - Develop: develop/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Ref hub\n")
    _write(root / "docs/develop/plugins/index.md", "# Dev hub\n")
    context, _ = assemble(root)
    staged = context.output_dir / "docs"
    assert (staged / "reference/plugins/overview.md").is_file()
    assert not (staged / "reference/plugins/develop/setup.md").exists()
    assert (staged / "develop/plugins/setup.md").is_file()
    assert not (staged / "develop/plugins/nav.yml").exists()


def test_repeat_assembly_is_stable_and_removes_stale(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    assemble(root)
    first = (root / "docs/reference/plugins/guide.md").read_bytes()
    assemble(root)
    assert (root / "docs/reference/plugins/guide.md").read_bytes() == first
    (repo / "docs/guide.md").unlink()
    _write(repo / "docs/nav.yml", "- title: FM\n  path: fm.md\n")
    assemble(root)
    assert not (root / "docs/reference/plugins/guide.md").exists()
    assert (root / "docs/reference/plugins/fm.md").is_file()


def test_authored_file_under_target_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    _write(root / "docs/reference/plugins/guide.md", "# Mine\n")
    with pytest.raises(DocsError, match="authored file"):
        assemble(root)


def test_source_spoofing_is_rejected() -> None:
    with pytest.raises(DocsError, match="spoofing"):
        stamp_markdown("---\nsource: x:y\n---\n\n# T\n", repo="r", repo_path="p")


def test_source_symlink_escape_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    (repo / "docs/evil.md").symlink_to(tmp_path / "outside.md")
    _write(tmp_path / "outside.md", "# Outside\n")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    with pytest.raises(DocsError, match="escapes its repo"):
        assemble(root)


def test_overlapping_targets_rejected(tmp_path: Path) -> None:
    repo = tmp_path / "bob-plugins"
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([
            _mount_entry(repo, target="reference/plugins"),
            {**_mount_entry(repo, target="reference/plugins/sub"), "source": "other"},
        ]),
    )
    with pytest.raises(DocsError, match="overlaps"):
        load_mounts(root)


def test_missing_landing_page_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    with pytest.raises(DocsError, match="landing page"):
        assemble(root)


def test_missing_nav_placeholder_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(tmp_path / "root", mounts=_mounts_yaml([_mount_entry(repo)]))
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    with pytest.raises(DocsError, match="no nav placeholder"):
        assemble(root)


def test_unresolved_nav_placeholder_rejected(tmp_path: Path) -> None:
    root = _minimal_root(
        tmp_path / "root", nav_extra="  - Ghost: reference/ghost/index.md\n"
    )
    with pytest.raises(DocsError, match="matches no mount target"):
        assemble(root)


def test_source_index_collision_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins", with_index=True)
    _write(repo / "docs/nav.yml", "- title: Guide\n  path: guide.md\n")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    with pytest.raises(DocsError, match="collides with the committed landing page"):
        assemble(root)


def test_production_flag_pairing(tmp_path: Path) -> None:
    root = _minimal_root(tmp_path / "root")
    with pytest.raises(DocsError, match="requires --production"):
        assemble(root, sources="x.json")
    with pytest.raises(DocsError, match="requires --sources"):
        assemble(root, production=True)


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=cwd, text=True, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, check=True,
    )
    return result.stdout.strip()


def _commit_all(path: Path) -> str:
    _git(path, "init", "-b", "main")
    _git(path, "add", "-A")
    _git(
        path, "-c", "user.email=test@example.com", "-c", "user.name=Test",
        "commit", "-m", "fixture",
    )
    return _git(path, "rev-parse", "HEAD")


def _production_fixture(tmp_path: Path) -> tuple[Path, Path, str, str]:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    plugin_sha = _commit_all(repo)
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    _write(
        root / ".gitignore",
        "target/\ndocs/reference/plugins/*\n!docs/reference/plugins/index.md\n",
    )
    cli_sha = _commit_all(root)
    return root, repo, cli_sha, plugin_sha


def _sources_json(path: Path, cli_sha: str, plugin_sha: str, plugin_path: str) -> Path:
    out = path / "sources.json"
    out.write_text(
        json.dumps(
            {
                "sources": {
                    # Relative to this JSON file, exercising JSON-relative resolution.
                    "bob-cli": {"path": "root", "sha": cli_sha},
                    "bob-plugins": {"path": plugin_path, "sha": plugin_sha},
                }
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return out


def test_production_success_records_repo_shas(tmp_path: Path) -> None:
    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    context, manifest_path = assemble(root, production=True, sources=sources)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["sources"]["bob-cli"]["sha"] == cli_sha
    assert manifest["sources"]["bob-cli"]["dirty"] is False
    assert manifest["sources"]["bob-plugins"]["sha"] == plugin_sha
    assert manifest["sources"]["bob-plugins"]["dirty"] is False
    assert manifest["sources"]["bob-plugins"]["branch"] == "main"
    assert (root / "docs/reference/plugins/guide.md").is_file()


def test_production_rejects_dirty_source(tmp_path: Path) -> None:
    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    (repo / "docs/guide.md").write_text("# Changed\n", encoding="utf-8")
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    with pytest.raises(DocsError, match="not clean"):
        assemble(root, production=True, sources=sources)


def test_production_rejects_wrong_sha(tmp_path: Path) -> None:
    root, repo, cli_sha, _ = _production_fixture(tmp_path)
    sources = _sources_json(tmp_path, cli_sha, "0" * 40, str(repo))
    with pytest.raises(DocsError, match="does not match"):
        assemble(root, production=True, sources=sources)


def test_production_rejects_missing_entry(tmp_path: Path) -> None:
    root, _repo, cli_sha, _plugin_sha = _production_fixture(tmp_path)
    out = tmp_path / "sources.json"
    out.write_text(
        json.dumps({"sources": {"bob-cli": {"path": ".", "sha": cli_sha}}}) + "\n",
        encoding="utf-8",
    )
    with pytest.raises(DocsError, match="missing production source entries"):
        assemble(root, production=True, sources=out)


def test_production_rejects_env_overrides(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    with pytest.raises(DocsError, match="refuses discovery env overrides"):
        assemble(root, production=True, sources=sources)


def test_production_rejects_wrong_cli_path(tmp_path: Path) -> None:
    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    other = tmp_path / "elsewhere"
    other.mkdir()
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    raw = json.loads(sources.read_text(encoding="utf-8"))
    raw["sources"]["bob-cli"]["path"] = str(other)
    sources.write_text(json.dumps(raw) + "\n", encoding="utf-8")
    with pytest.raises(DocsError, match="actual build root"):
        assemble(root, production=True, sources=sources)


def test_cache_clone_from_local_remote(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    _commit_all(remote)
    root = _minimal_root(tmp_path / "root")
    import dataclasses

    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    assert (found / "docs/guide.md").is_file()
    again, _ = discover_repo(probe, root=root)
    assert again == found


def test_missing_source_nav_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = tmp_path / "bob-plugins"
    _write(repo / "docs/guide.md", "# Guide\n")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    with pytest.raises(DocsError, match="missing source nav.yml"):
        assemble(root)


def test_nav_entry_problems_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    _write(repo / "docs/nav.yml", "- title: Gone\n  path: gone.md\n")
    with pytest.raises(DocsError, match="missing from"):
        assemble(root)
    _write(
        repo / "docs/nav.yml",
        "- title: A\n  path: guide.md\n- title: B\n  path: guide.md\n",
    )
    with pytest.raises(DocsError, match="duplicate nav path"):
        assemble(root)
    _write(repo / "docs/nav.yml", "- title: Up\n  path: ../evil.md\n")
    with pytest.raises(DocsError, match="without empty"):
        assemble(root)

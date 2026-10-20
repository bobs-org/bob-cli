"""Fixture-repository tests for mount discovery, copying, nav, and production."""

import dataclasses
import json
import subprocess
from pathlib import Path

import pytest

from bob_docs.assemble import assemble, build_manifest
from bob_docs.build import mirror_docs_once
from bob_docs.errors import DocsError
from bob_docs.markdown import parse_front_matter
from bob_docs.mounts import (
    Mount,
    discover_repo,
    env_var_for,
    load_mounts,
    origins_match,
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


def _gitignore_for_mounts(*, ignore_indexes: bool = False) -> str:
    lines = ["target/", "docs/reference/plugins/*"]
    if not ignore_indexes:
        lines.append("!docs/reference/plugins/index.md")
    lines.append("docs/develop/plugins/*")
    if not ignore_indexes:
        lines.append("!docs/develop/plugins/index.md")
    return "\n".join(lines) + "\n"


def _minimal_root(
    root: Path,
    *,
    mounts: str = "[]\n",
    nav_extra: str = "",
    ignore_indexes: bool = False,
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
    _write(root / ".gitignore", _gitignore_for_mounts(ignore_indexes=ignore_indexes))
    _write(
        root / "mkdocs.yml",
        "site_name: Test\ndocs_dir: docs\nsite_dir: target/docs/site\n"
        "theme:\n  name: material\n  custom_dir: docs/overrides\n"
        "markdown_extensions:\n  - custom:\n"
        "      format: !!python/name:bob_docs.fences.fence_format ''\n"
        "nav:\n  - Home: index.md\n" + nav_extra,
    )
    return root


def _plugin_repo(
    repo: Path, *, with_index: bool = False, origin: str | None = None
) -> Path:
    _write(repo / "docs/guide.md", "# Guide\n\nBody text.\n")
    _write(
        repo / "docs/fm.md",
        "---\ntitle: FM\ndescription: Has front matter.\n---\n\n# FM\n",
    )
    _write(
        repo / "docs/nav.yml",
        "- title: Guide\n  path: guide.md\n- title: FM\n  path: fm.md\n",
    )
    _write(repo / "docs/media.yml", "assets: []\n")
    _write(repo / "docs/.hidden.md", "# Hidden\n")
    if with_index:
        _write(
            repo / "docs/index.md",
            "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n---\n\n# Plugins\n",
        )
    return _ensure_git_origin(repo, origin or repo.as_uri())


def _mount_entry(
    repo_dir: Path, *, repo: str = "bob-plugins", target: str = "reference/plugins"
) -> dict[str, str]:
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
        repo="bob-plugins",
        remote="x",
        branch="main",
        source=PurePosixPath("docs"),
        target=PurePosixPath("reference/plugins"),
        nav_title="Plugins",
    )


def test_discovery_prefers_env_over_sibling(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _minimal_root(tmp_path / "root")
    probe = _probe()
    _ensure_git_origin(tmp_path / "bob-plugins", probe.remote)
    winner = _ensure_git_origin(tmp_path / "winner", probe.remote)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(winner))
    found, _ = discover_repo(probe, root=root)
    assert found == winner.resolve()


def test_discovery_sibling_before_linked(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    root = _minimal_root(tmp_path / "root")
    probe = _probe()
    sibling = _ensure_git_origin(tmp_path / "bob-plugins", probe.remote)
    _ensure_git_origin(root / "sase/repos/linked/bob-plugins", probe.remote)
    found, _ = discover_repo(probe, root=root)
    assert found == sibling.resolve()


def test_discovery_linked_when_no_sibling(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    root = _minimal_root(tmp_path / "root")
    probe = _probe()
    linked = _ensure_git_origin(root / "sase/repos/linked/bob-plugins", probe.remote)
    found, _ = discover_repo(probe, root=root)
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
    _write(
        root / "docs/reference/plugins/index.md",
        "---\ntitle: Plugins hub\n---\n\n# Hub\n",
    )
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
    plugins = next(
        item for item in develop if isinstance(item, dict) and "Plugins" in item
    )
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
    _ensure_git_origin(repo, repo.as_uri())
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml(
            [
                {
                    **_mount_entry(repo, target="reference/plugins"),
                    "nav_title": "Reference",
                },
                {
                    "repo": "bob-plugins",
                    "remote": repo.as_uri(),
                    "branch": "main",
                    "source": "docs/develop",
                    "target": "develop/plugins",
                    "nav_title": "Develop",
                },
            ]
        ),
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
        mounts=_mounts_yaml(
            [
                _mount_entry(repo, target="reference/plugins"),
                {
                    **_mount_entry(repo, target="reference/plugins/sub"),
                    "source": "other",
                },
            ]
        ),
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


def _nested_source_hub_repo(repo: Path) -> Path:
    _write(
        repo / "docs/index.md",
        "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n---\n\n# Plugins\n",
    )
    _write(repo / "docs/overview.md", "# Overview\n")
    _write(repo / "docs/nav.yml", "- title: Overview\n  path: overview.md\n")
    _write(
        repo / "docs/develop/index.md",
        "---\ntitle: Plugin development\ndescription: Develop hub.\n---\n\n# Develop\n",
    )
    _write(repo / "docs/develop/setup.md", "# Setup\n")
    _write(repo / "docs/develop/nav.yml", "- title: Setup\n  path: setup.md\n")
    return _ensure_git_origin(repo, repo.as_uri())


def _nested_source_hub_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, repo: Path
) -> Path:
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    return _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml(
            [
                {
                    **_mount_entry(repo, target="reference/plugins"),
                    "nav_title": "Reference",
                },
                {
                    "repo": "bob-plugins",
                    "remote": repo.as_uri(),
                    "branch": "main",
                    "source": "docs/develop",
                    "target": "develop/plugins",
                    "nav_title": "Develop",
                },
            ]
        ),
        nav_extra="  - Reference: reference/plugins/index.md\n  - Develop: develop/plugins/index.md\n",
        ignore_indexes=True,
    )


def test_source_hub_and_nested_developer_hub_copy_with_provenance_and_nav(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _nested_source_hub_repo(tmp_path / "bob-plugins")
    root = _nested_source_hub_root(tmp_path, monkeypatch, repo)
    context, _ = assemble(root)
    staged = context.output_dir / "docs"
    ref_hub = staged / "reference/plugins/index.md"
    dev_hub = staged / "develop/plugins/index.md"
    assert ref_hub.is_file()
    assert dev_hub.is_file()
    assert ref_hub.read_text(encoding="utf-8").startswith(
        "---\nsource: bob-plugins:docs/index.md\n"
    )
    assert "title: Plugins inventory" in ref_hub.read_text(encoding="utf-8")
    assert dev_hub.read_text(encoding="utf-8").startswith(
        "---\nsource: bob-plugins:docs/develop/index.md\n"
    )
    assert "title: Plugin development" in dev_hub.read_text(encoding="utf-8")
    assert (staged / "reference/plugins/overview.md").is_file()
    assert not (staged / "reference/plugins/develop/setup.md").exists()
    assert not (staged / "reference/plugins/develop/index.md").exists()
    assert (staged / "develop/plugins/setup.md").is_file()
    from bob_docs.yaml_tools import load_yaml

    config = load_yaml(context.assembled_config)
    reference = next(
        item for item in config["nav"] if isinstance(item, dict) and "Reference" in item
    )
    develop = next(
        item for item in config["nav"] if isinstance(item, dict) and "Develop" in item
    )
    assert reference["Reference"][0] == {
        "Plugins inventory": "reference/plugins/index.md"
    }
    assert reference["Reference"][1] == {"Overview": "reference/plugins/overview.md"}
    assert len(reference["Reference"]) == 2
    assert develop["Develop"][0] == {"Plugin development": "develop/plugins/index.md"}
    assert develop["Develop"][1] == {"Setup": "develop/plugins/setup.md"}
    assert len(develop["Develop"]) == 2
    edits = config["extra"]["bob_edits"]
    assert edits["reference/plugins/"].endswith("/docs/index.md")
    assert "/main/" in edits["reference/plugins/"] or edits[
        "reference/plugins/"
    ].endswith("/docs/index.md")
    assert edits["develop/plugins/"].endswith("/docs/develop/index.md")
    manifest = json.loads(
        (context.output_dir / "mount-manifest.json").read_text(encoding="utf-8")
    )
    assert (
        manifest["sources"]["reference/plugins/index.md"] == "bob-plugins:docs/index.md"
    )
    assert (
        manifest["sources"]["develop/plugins/index.md"]
        == "bob-plugins:docs/develop/index.md"
    )


def test_source_hub_repeat_assembly_and_cleanup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins", with_index=True)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
        ignore_indexes=True,
    )
    assemble(root)
    hub = root / "docs/reference/plugins/index.md"
    first_hub = hub.read_bytes()
    first_guide = (root / "docs/reference/plugins/guide.md").read_bytes()
    assemble(root)
    assert hub.read_bytes() == first_hub
    assert (root / "docs/reference/plugins/guide.md").read_bytes() == first_guide
    _write(
        repo / "docs/index.md",
        "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n---\n\n# Plugins v2\n",
    )
    assemble(root)
    assert "Plugins v2" in hub.read_text(encoding="utf-8")
    assert hub.read_text(encoding="utf-8").startswith(
        "---\nsource: bob-plugins:docs/index.md\n"
    )
    (repo / "docs/guide.md").unlink()
    _write(repo / "docs/nav.yml", "- title: FM\n  path: fm.md\n")
    assemble(root)
    assert not (root / "docs/reference/plugins/guide.md").exists()
    assert (root / "docs/reference/plugins/fm.md").is_file()
    assert hub.is_file()
    (repo / "docs/index.md").unlink()
    with pytest.raises(DocsError, match="previously copied hub"):
        assemble(root)
    assert hub.is_file()
    assert (root / "docs/reference/plugins/fm.md").is_file()


def test_source_hub_collision_leaves_existing_copies_untouched(
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
    guide = root / "docs/reference/plugins/guide.md"
    first = guide.read_bytes()
    authored = (root / "docs/reference/plugins/index.md").read_bytes()
    _write(
        repo / "docs/index.md",
        "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n---\n\n# Plugins\n",
    )
    with pytest.raises(DocsError, match="collides with the committed landing page"):
        assemble(root)
    assert guide.read_bytes() == first
    assert (root / "docs/reference/plugins/index.md").read_bytes() == authored


def test_source_hub_nonignored_index_leaves_outputs_untouched(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins", with_index=True)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
        ignore_indexes=True,
    )
    assemble(root)
    hub = root / "docs/reference/plugins/index.md"
    guide = root / "docs/reference/plugins/guide.md"
    first_hub, first_guide = hub.read_bytes(), guide.read_bytes()
    _write(root / ".gitignore", _gitignore_for_mounts(ignore_indexes=False))
    with pytest.raises(DocsError, match="not gitignored"):
        assemble(root)
    assert hub.read_bytes() == first_hub
    assert guide.read_bytes() == first_guide


def test_source_hub_symlink_leaves_outputs_untouched(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins", with_index=True)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
        ignore_indexes=True,
    )
    assemble(root)
    hub = root / "docs/reference/plugins/index.md"
    guide = root / "docs/reference/plugins/guide.md"
    first_guide = guide.read_bytes()
    outside = tmp_path / "elsewhere.md"
    _write(outside, "# Elsewhere\n")
    hub.unlink()
    hub.symlink_to(outside)
    with pytest.raises(DocsError, match="is a symlink"):
        assemble(root)
    assert outside.read_text(encoding="utf-8") == "# Elsewhere\n"
    assert guide.read_bytes() == first_guide


def test_tracked_source_hub_destination_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins", with_index=True)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
        ignore_indexes=True,
    )
    assemble(root)
    hub = root / "docs/reference/plugins/index.md"
    first = hub.read_bytes()
    _commit_all(root)
    subprocess.run(
        ["git", "add", "-f", "docs/reference/plugins/index.md"],
        cwd=root,
        check=True,
    )
    subprocess.run(
        [
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "track hub",
        ],
        cwd=root,
        check=True,
    )
    with pytest.raises(DocsError, match="tracked files"):
        assemble(root)
    assert hub.read_bytes() == first


def test_nav_listing_index_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins", with_index=True)
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
        ignore_indexes=True,
    )
    _write(
        repo / "docs/nav.yml",
        "- title: Hub\n  path: index.md\n- title: Guide\n  path: guide.md\n",
    )
    with pytest.raises(DocsError, match="must not"):
        assemble(root)


def test_production_flag_pairing(tmp_path: Path) -> None:
    root = _minimal_root(tmp_path / "root")
    with pytest.raises(DocsError, match="requires --production"):
        assemble(root, sources="x.json")
    with pytest.raises(DocsError, match="requires --sources"):
        assemble(root, production=True)


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    return result.stdout.strip()


def _ensure_git_origin(path: Path, origin: str) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    if not (path / ".git").exists():
        _git(path, "init", "-b", "main")
    current = subprocess.run(
        ["git", "config", "--get", "remote.origin.url"],
        cwd=path,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if current.returncode != 0:
        _git(path, "remote", "add", "origin", origin)
    elif current.stdout.strip() != origin:
        _git(path, "remote", "set-url", "origin", origin)
    _git(path, "add", "-A")
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=path,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    if status.stdout.strip():
        _git(
            path,
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "fixture",
        )
    else:
        head = subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD"],
            cwd=path,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if head.returncode != 0:
            _git(
                path,
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            )
    return path


def _commit_all(path: Path) -> str:
    if not (path / ".git").exists():
        _git(path, "init", "-b", "main")
    _git(path, "add", "-A")
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=path,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    if status.stdout.strip():
        _git(
            path,
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "fixture",
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
    _ensure_git_origin(repo, repo.as_uri())
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


def _two_mount_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    repo_one: Path,
    repo_two: Path,
    *,
    repo_two_name: str = "bob-other",
) -> Path:
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo_one))
    monkeypatch.setenv(
        f"BOB_DOCS_{repo_two_name.upper().replace('-', '_')}_DIR", str(repo_two)
    )
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml(
            [
                _mount_entry(repo_one),
                {
                    "repo": repo_two_name,
                    "remote": repo_two.as_uri(),
                    "branch": "main",
                    "source": "docs",
                    "target": "develop/plugins",
                    "nav_title": "Other",
                },
            ]
        ),
        nav_extra=(
            "  - Plugins: reference/plugins/index.md\n"
            "  - Other: develop/plugins/index.md\n"
        ),
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub one\n")
    _write(root / "docs/develop/plugins/index.md", "# Hub two\n")
    return root


def _other_repo(path: Path) -> Path:
    _write(path / "docs/extra.md", "# Extra\n")
    _write(path / "docs/nav.yml", "- title: Extra\n  path: extra.md\n")
    return _ensure_git_origin(path, path.as_uri())


def test_stamp_without_front_matter_exact() -> None:
    assert (
        stamp_markdown("# T\n", repo="r", repo_path="p")
        == "---\nsource: r:p\n---\n# T\n"
    )


def test_stamp_with_front_matter_keeps_closing_delimiter() -> None:
    stamped = stamp_markdown(
        "---\ntitle: FM\n---\n\n# FM\n", repo="r", repo_path="docs/fm.md"
    )
    assert stamped == "---\nsource: r:docs/fm.md\ntitle: FM\n---\n\n# FM\n"
    head, _body, error = parse_front_matter(stamped, source="docs/copy.md")
    assert error is None
    assert head == {"source": "r:docs/fm.md", "title": "FM"}


def test_spoofed_second_mount_leaves_first_outputs_intact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo_one = _plugin_repo(tmp_path / "bob-plugins")
    repo_two = _other_repo(tmp_path / "bob-other")
    root = _two_mount_root(tmp_path, monkeypatch, repo_one, repo_two)
    assemble(root)
    guide = root / "docs/reference/plugins/guide.md"
    extra = root / "docs/develop/plugins/extra.md"
    first_guide, first_extra = guide.read_bytes(), extra.read_bytes()
    _write(repo_two / "docs/extra.md", "---\nsource: forged:x\n---\n\n# Extra\n")
    with pytest.raises(DocsError, match="spoofing"):
        assemble(root)
    assert guide.read_bytes() == first_guide
    assert extra.read_bytes() == first_extra


def test_destination_file_symlink_rejected(
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
    guide = root / "docs/reference/plugins/guide.md"
    first = guide.read_bytes()
    outside = tmp_path / "elsewhere.md"
    _write(outside, "# Elsewhere\n")
    guide.unlink()
    guide.symlink_to(outside)
    with pytest.raises(DocsError, match="is a symlink"):
        assemble(root)
    assert outside.read_text(encoding="utf-8") == "# Elsewhere\n"
    (repo / "docs/guide.md").write_text("# Guide\n\nBody text.\n", encoding="utf-8")
    guide.unlink()
    assemble(root)
    assert guide.read_bytes() == first


def test_parent_directory_symlink_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo_one = _plugin_repo(tmp_path / "bob-plugins")
    repo_two = _other_repo(tmp_path / "bob-other")
    root = _two_mount_root(tmp_path, monkeypatch, repo_one, repo_two)
    assemble(root)
    first = (root / "docs/reference/plugins/guide.md").read_bytes()
    outside = tmp_path / "outside"
    (outside / "plugins").mkdir(parents=True)
    _write(outside / "plugins" / "index.md", "# Hub two\n")
    import shutil

    shutil.rmtree(root / "docs/develop")
    (root / "docs/develop").symlink_to(outside, target_is_directory=True)
    with pytest.raises(DocsError, match="is a symlink"):
        assemble(root)
    assert sorted(path.name for path in (outside / "plugins").iterdir()) == ["index.md"]
    assert (root / "docs/reference/plugins/guide.md").read_bytes() == first


def test_nonignored_target_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _plugin_repo(tmp_path / "bob-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo, target="reference/other")]),
        nav_extra="  - Other: reference/other/index.md\n",
    )
    _write(root / "docs/reference/other/index.md", "# Hub\n")
    with pytest.raises(DocsError, match="not gitignored"):
        assemble(root)


@pytest.mark.parametrize("target", ["../evil", "/absolute"])
def test_dotdot_and_absolute_targets_rejected(tmp_path: Path, target: str) -> None:
    root = _minimal_root(
        tmp_path / "root",
        mounts=(
            "- repo: r\n  remote: x\n  branch: main\n  source: docs\n"
            f"  target: {target}\n  nav_title: T\n"
        ),
    )
    with pytest.raises(DocsError, match="relative path"):
        load_mounts(root)


@pytest.mark.parametrize("source", ["..", ".", "/absolute"])
def test_dotdot_and_absolute_sources_rejected(tmp_path: Path, source: str) -> None:
    root = _minimal_root(
        tmp_path / "root",
        mounts=(
            "- repo: r\n  remote: x\n  branch: main\n  source: "
            f"{source}\n  target: reference/plugins\n  nav_title: T\n"
        ),
    )
    with pytest.raises(DocsError, match="relative path"):
        load_mounts(root)


def test_symlinked_source_directory_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = tmp_path / "bob-plugins"
    real = repo / "real"
    _write(real / "guide.md", "# Guide\n")
    _write(real / "nav.yml", "- title: Guide\n  path: guide.md\n")
    (repo / "docs").symlink_to(real)
    _ensure_git_origin(repo, repo.as_uri())
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml([_mount_entry(repo)]),
        nav_extra="  - Plugins: reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Hub\n")
    with pytest.raises(DocsError, match="is a symlink"):
        assemble(root)


def test_stale_tracked_file_survives(
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
    _commit_all(root)
    assemble(root)
    guide = root / "docs/reference/plugins/guide.md"
    first = guide.read_bytes()
    subprocess.run(
        ["git", "add", "-f", "docs/reference/plugins/guide.md"],
        cwd=root,
        check=True,
    )
    subprocess.run(
        [
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "track copy",
        ],
        cwd=root,
        check=True,
    )
    (repo / "docs/guide.md").unlink()
    _write(repo / "docs/nav.yml", "- title: FM\n  path: fm.md\n")
    with pytest.raises(DocsError, match="tracked file"):
        assemble(root)
    assert guide.read_bytes() == first


def test_tracked_file_inside_target_rejected(
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
    _commit_all(root)
    subprocess.run(
        ["git", "add", "-f", "docs/reference/plugins/guide.md"],
        cwd=root,
        check=True,
    )
    with pytest.raises(DocsError, match="tracked files outside the landing page"):
        assemble(root)


def test_stale_nonignored_path_refuses_before_any_write(
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
    first = (root / "docs/reference/plugins/fm.md").read_bytes()
    manifest = json.loads(
        (root / "target/docs/mount-manifest.json").read_text(encoding="utf-8")
    )
    manifest["files"].append("reference/other/old.md")
    (root / "target/docs/mount-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    _write(root / "docs/reference/other/old.md", "# Old\n")
    with pytest.raises(DocsError, match="not gitignored"):
        assemble(root)
    assert (root / "docs/reference/plugins/fm.md").read_bytes() == first
    assert (root / "docs/reference/other/old.md").is_file()


def test_repo_shared_by_two_mounts_resolved_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import bob_docs.assemble as assemble_module

    calls: list[str] = []
    real_discover = assemble_module.discover_repo

    def counting(mount: Mount, **kwargs: object) -> tuple[Path, str]:
        calls.append(mount.repo)
        return real_discover(mount, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(assemble_module, "discover_repo", counting)
    repo = tmp_path / "bob-plugins"
    _write(repo / "docs/overview.md", "# Overview\n")
    _write(repo / "docs/develop/setup.md", "# Setup\n")
    _write(repo / "docs/nav.yml", "- title: Overview\n  path: overview.md\n")
    _write(repo / "docs/develop/nav.yml", "- title: Setup\n  path: setup.md\n")
    _ensure_git_origin(repo, repo.as_uri())
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(repo))
    root = _minimal_root(
        tmp_path / "root",
        mounts=_mounts_yaml(
            [
                {
                    **_mount_entry(repo, target="reference/plugins"),
                    "nav_title": "Reference",
                },
                {
                    "repo": "bob-plugins",
                    "remote": repo.as_uri(),
                    "branch": "main",
                    "source": "docs/develop",
                    "target": "develop/plugins",
                    "nav_title": "Develop",
                },
            ]
        ),
        nav_extra="  - Reference: reference/plugins/index.md\n  - Develop: develop/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Ref hub\n")
    _write(root / "docs/develop/plugins/index.md", "# Dev hub\n")
    assemble(root)
    assert calls == ["bob-plugins"]
    assert (root / "docs/reference/plugins/overview.md").is_file()
    assert (root / "docs/develop/plugins/setup.md").is_file()


def test_cached_clone_refreshes_to_remote_head(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    _commit_all(remote)
    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    assert (found / "docs/guide.md").is_file()
    _write(remote / "docs/guide.md", "# Guide v2\n")
    _commit_all(remote)
    again, _ = discover_repo(probe, root=root)
    assert again == found
    assert (found / "docs/guide.md").read_text(encoding="utf-8") == "# Guide v2\n"
    from bob_docs.mounts import repo_git_info

    assert repo_git_info(found)["sha"] == _git(remote, "rev-parse", "HEAD")


def test_cached_clone_with_wrong_origin_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    _commit_all(remote)
    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    other = dataclasses.replace(probe, remote="https://example.com/other.git")
    with pytest.raises(DocsError, match="not the mount remote"):
        discover_repo(other, root=root)
    assert (found / "docs/guide.md").is_file()


def test_cached_clone_without_git_origin_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    cached = tmp_path / "cache" / "bob-docs" / "bob-plugins"
    cached.mkdir(parents=True)
    _git(cached, "init", "-b", "main")
    _git(
        cached,
        "-c",
        "user.email=test@example.com",
        "-c",
        "user.name=Test",
        "commit",
        "--allow-empty",
        "-m",
        "fixture",
    )
    root = _minimal_root(tmp_path / "root")
    with pytest.raises(DocsError, match="has no origin"):
        discover_repo(_probe(), root=root)


def test_cached_clone_offline_keeps_cache_with_warning(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import bob_docs.mounts as mounts_module

    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    _commit_all(remote)
    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    first = (found / "docs/guide.md").read_text(encoding="utf-8")
    real_run = mounts_module._run_git

    def fake_run(cwd: Path, *args: str):
        if args and args[0] == "fetch":
            return subprocess.CompletedProcess(
                ["git", *args],
                1,
                stdout="",
                stderr="fatal: unable to access 'https://example.com/x.git/': Could not resolve host: example.com\n",
            )
        return real_run(cwd, *args)

    monkeypatch.setattr(mounts_module, "_run_git", fake_run)
    capsys.readouterr()
    again, _ = discover_repo(probe, root=root)
    assert again == found
    assert (found / "docs/guide.md").read_text(encoding="utf-8") == first
    assert "offline" in capsys.readouterr().out


def test_production_rejects_missing_sha_and_path(tmp_path: Path) -> None:
    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    raw = json.loads(sources.read_text(encoding="utf-8"))
    del raw["sources"]["bob-plugins"]["sha"]
    sources.write_text(json.dumps(raw) + "\n", encoding="utf-8")
    with pytest.raises(DocsError, match="missing keys"):
        assemble(root, production=True, sources=sources)
    raw = json.loads(sources.read_text(encoding="utf-8"))
    raw["sources"]["bob-plugins"]["sha"] = plugin_sha
    del raw["sources"]["bob-plugins"]["path"]
    sources.write_text(json.dumps(raw) + "\n", encoding="utf-8")
    with pytest.raises(DocsError, match="missing keys"):
        assemble(root, production=True, sources=sources)


def test_production_rejects_untracked_only_dirt(tmp_path: Path) -> None:
    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    (repo / "docs/draft.md").write_text("# Draft\n", encoding="utf-8")
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    with pytest.raises(DocsError, match="not clean"):
        assemble(root, production=True, sources=sources)


def test_production_rechecks_clean_after_copy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import bob_docs.assemble as assemble_module

    root, repo, cli_sha, plugin_sha = _production_fixture(tmp_path)
    sources = _sources_json(tmp_path, cli_sha, plugin_sha, str(repo))
    real_sync = assemble_module.sync_mount_copies

    def dirty_sync(*args: object, **kwargs: object) -> set[str]:
        out = real_sync(*args, **kwargs)  # type: ignore[arg-type]
        (root / "sneaky.md").write_text("# Sneaky\n", encoding="utf-8")
        return out

    monkeypatch.setattr(assemble_module, "sync_mount_copies", dirty_sync)
    with pytest.raises(DocsError, match="not clean"):
        assemble(root, production=True, sources=sources)


def test_build_manifest_records_builder_and_installed_builders(tmp_path: Path) -> None:
    from importlib.metadata import PackageNotFoundError
    from importlib.metadata import version as dist_version

    manifest = build_manifest(tmp_path)
    assert manifest["builder"] == "zensical"
    assert "zensical" in manifest["tools"]
    assert build_manifest(tmp_path, builder="material")["builder"] == "material"
    for dist in ("mkdocs", "mkdocs-material"):
        try:
            dist_version(dist)
            installed = True
        except PackageNotFoundError:
            installed = False
        assert (dist in manifest["tools"]) == installed


def test_mirror_once_adds_updates_deletes_and_preserves_manifest(
    tmp_path: Path,
) -> None:
    authored = tmp_path / "authored"
    staged = tmp_path / "staged"
    _write(authored / "keep.md", "# Keep v1\n")
    _write(authored / "gone.md", "# Gone\n")
    _write(staged / "keep.md", "# Keep v1\n")
    _write(staged / "stale.md", "# Stale\n")
    _write(staged / "build.json", '{"builder": "zensical"}\n')
    assert mirror_docs_once(authored, staged) == {"copied": 1, "deleted": 1}
    assert (staged / "gone.md").read_text(encoding="utf-8") == "# Gone\n"
    assert not (staged / "stale.md").exists()
    _write(authored / "keep.md", "# Keep v2 with more words here\n")
    _write(authored / "new.md", "# New\n")
    (authored / "gone.md").unlink()
    assert mirror_docs_once(authored, staged) == {"copied": 2, "deleted": 1}
    assert (staged / "keep.md").read_text(
        encoding="utf-8"
    ) == "# Keep v2 with more words here\n"
    assert (staged / "new.md").is_file()
    assert not (staged / "gone.md").exists()
    assert (staged / "build.json").read_text(
        encoding="utf-8"
    ) == '{"builder": "zensical"}\n'


def test_origins_match_known_equivalents_not_basenames() -> None:
    assert origins_match(
        "git@github.com:bobs-org/bob-plugins.git",
        "https://github.com/bobs-org/bob-plugins",
    )
    assert origins_match(
        "ssh://git@github.com/bobs-org/bob-plugins.git",
        "https://www.github.com/bobs-org/bob-plugins.git",
    )
    local = "/tmp/bob-docs-origin-fixture"
    assert origins_match(local, Path(local).as_uri())
    assert not origins_match(
        "https://example.com/bob-plugins.git",
        "https://github.com/bobs-org/bob-plugins.git",
    )
    assert not origins_match(
        "https://example.com/wrong.git",
        "https://github.com/bobs-org/bob-plugins.git",
    )


def test_discovery_accepts_equivalent_github_and_file_origins(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _minimal_root(tmp_path / "root")
    winner = _ensure_git_origin(
        tmp_path / "winner", "git@github.com:bobs-org/bob-plugins.git"
    )
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(winner))
    probe = dataclasses.replace(
        _probe(), remote="https://github.com/bobs-org/bob-plugins.git"
    )
    found, _ = discover_repo(probe, root=root)
    assert found == winner.resolve()

    local = tmp_path / "local-src"
    _ensure_git_origin(local, str(local.resolve()))
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(local))
    file_probe = dataclasses.replace(_probe(), remote=local.as_uri())
    found_local, _ = discover_repo(file_probe, root=root)
    assert found_local == local.resolve()


def test_discovery_accepts_git_worktree_git_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    main = _plugin_repo(tmp_path / "main")
    worktree = tmp_path / "bob-plugins"
    _git(main, "worktree", "add", str(worktree), "HEAD")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(worktree))
    probe = dataclasses.replace(_probe(), remote=main.as_uri())
    found, _ = discover_repo(probe, root=_minimal_root(tmp_path / "root"))
    assert found == worktree.resolve()
    assert (worktree / ".git").is_file()


@pytest.mark.parametrize("tier", ["env", "sibling", "linked"])
def test_existing_nongit_candidate_fails_without_fallthrough(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, tier: str
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    good = _plugin_repo(tmp_path / "good-source")
    probe = dataclasses.replace(_probe(), remote=good.as_uri())
    root = _minimal_root(tmp_path / "root")
    bad = tmp_path / "not-git"
    bad.mkdir()
    if tier == "env":
        _ensure_git_origin(tmp_path / "bob-plugins", probe.remote)
        monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(bad))
        match = r"\$BOB_DOCS_BOB_PLUGINS_DIR"
    elif tier == "sibling":
        sibling = tmp_path / "bob-plugins"
        sibling.mkdir()
        _ensure_git_origin(root / "sase/repos/linked/bob-plugins", probe.remote)
        match = "sibling"
    else:
        linked = root / "sase/repos/linked/bob-plugins"
        linked.mkdir(parents=True)
        match = "linked checkout"
    with pytest.raises(DocsError, match=match):
        discover_repo(probe, root=root)
    assert not (tmp_path / "cache" / "bob-docs" / "bob-plugins").exists()


@pytest.mark.parametrize("tier", ["env", "sibling", "linked"])
def test_existing_wrong_origin_fails_without_fallthrough(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, tier: str
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    good = _plugin_repo(tmp_path / "good-source")
    wanted = good.as_uri()
    wrong = "https://example.com/wrong.git"
    probe = dataclasses.replace(_probe(), remote=wanted)
    root = _minimal_root(tmp_path / "root")
    if tier == "env":
        winner = _ensure_git_origin(tmp_path / "winner", wrong)
        _ensure_git_origin(tmp_path / "bob-plugins", wanted)
        monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(winner))
    elif tier == "sibling":
        _ensure_git_origin(tmp_path / "bob-plugins", wrong)
        _ensure_git_origin(root / "sase/repos/linked/bob-plugins", wanted)
    else:
        _ensure_git_origin(root / "sase/repos/linked/bob-plugins", wrong)
    with pytest.raises(DocsError, match="not the mount remote"):
        discover_repo(probe, root=root)
    assert not (tmp_path / "cache" / "bob-docs" / "bob-plugins").exists()


def test_discovery_rejects_enclosing_repository(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    _ensure_git_origin(tmp_path, "https://example.com/enclosing.git")
    nested = tmp_path / "bob-plugins"
    nested.mkdir()
    _write(nested / "README.md", "not its own repository\n")
    root = _minimal_root(tmp_path / "root")
    with pytest.raises(DocsError, match="enclosing repository"):
        discover_repo(_probe(), root=root)


def test_cached_clone_missing_branch_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    first = (found / "docs/guide.md").read_text(encoding="utf-8")
    missing = dataclasses.replace(probe, branch="missing")
    with pytest.raises(
        DocsError, match="could not refresh|couldn't find remote ref|missing"
    ):
        discover_repo(missing, root=root)
    assert (found / "docs/guide.md").read_text(encoding="utf-8") == first
    assert "offline" not in capsys.readouterr().out


def test_cached_clone_switches_to_requested_branch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    _git(remote, "checkout", "-b", "other")
    _write(remote / "docs/guide.md", "# Guide on other\n")
    _commit_all(remote)
    other_sha = _git(remote, "rev-parse", "HEAD")
    _git(remote, "checkout", "main")
    switched = dataclasses.replace(probe, branch="other")
    again, _ = discover_repo(switched, root=root)
    assert again == found
    assert (found / "docs/guide.md").read_text(encoding="utf-8") == "# Guide on other\n"
    from bob_docs.mounts import repo_git_info

    assert repo_git_info(found)["sha"] == other_sha


def test_cached_clone_auth_failure_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import bob_docs.mounts as mounts_module

    monkeypatch.delenv("BOB_DOCS_BOB_PLUGINS_DIR", raising=False)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
    remote = _plugin_repo(tmp_path / "remote")
    root = _minimal_root(tmp_path / "root")
    probe = dataclasses.replace(_probe(), remote=remote.as_uri(), branch="main")
    found, _ = discover_repo(probe, root=root)
    first = (found / "docs/guide.md").read_text(encoding="utf-8")
    real_run = mounts_module._run_git

    def fake_run(cwd: Path, *args: str):
        if args and args[0] == "fetch":
            return subprocess.CompletedProcess(
                ["git", *args],
                1,
                stdout="",
                stderr="fatal: Authentication failed for 'https://example.com/x.git/'\n",
            )
        return real_run(cwd, *args)

    monkeypatch.setattr(mounts_module, "_run_git", fake_run)
    with pytest.raises(DocsError, match="Authentication failed|could not refresh"):
        discover_repo(probe, root=root)
    assert (found / "docs/guide.md").read_text(encoding="utf-8") == first
    assert "offline" not in capsys.readouterr().out


def test_negated_ignore_on_later_mount_rejects_before_any_write(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo_one = _plugin_repo(tmp_path / "bob-plugins")
    repo_two = tmp_path / "bob-other"
    _write(repo_two / "docs/nested/page.md", "# Nested page\n")
    _write(repo_two / "docs/nested/icon.png", "png-bytes")
    _write(
        repo_two / "docs/nav.yml",
        "- title: Nested\n  path: nested/page.md\n",
    )
    _ensure_git_origin(repo_two, repo_two.as_uri())
    root = _two_mount_root(tmp_path, monkeypatch, repo_one, repo_two)
    _commit_all(root)
    assemble(root)
    first_guide = (root / "docs/reference/plugins/guide.md").read_bytes()
    first_page = (root / "docs/develop/plugins/nested/page.md").read_bytes()
    first_icon = (root / "docs/develop/plugins/nested/icon.png").read_bytes()
    stale = root / "docs/reference/plugins/fm.md"
    stale_bytes = stale.read_bytes()
    (repo_one / "docs/fm.md").unlink()
    _write(repo_one / "docs/nav.yml", "- title: Guide\n  path: guide.md\n")
    _write(repo_two / "docs/nested/new.md", "# New nested\n")
    _write(
        repo_two / "docs/nav.yml",
        "- title: Nested\n  path: nested/page.md\n- title: New\n  path: nested/new.md\n",
    )
    _write(
        root / ".gitignore",
        (root / ".gitignore").read_text(encoding="utf-8")
        + "!docs/develop/plugins/nested/\n"
        + "!docs/develop/plugins/nested/page.md\n"
        + "!docs/develop/plugins/nested/icon.png\n",
    )
    with pytest.raises(DocsError, match="docs/develop/plugins/nested/"):
        assemble(root)
    assert (root / "docs/reference/plugins/guide.md").read_bytes() == first_guide
    assert (root / "docs/develop/plugins/nested/page.md").read_bytes() == first_page
    assert (root / "docs/develop/plugins/nested/icon.png").read_bytes() == first_icon
    assert stale.read_bytes() == stale_bytes
    assert not (root / "docs/develop/plugins/nested/new.md").exists()


def test_mirror_replaces_file_with_directory_and_is_idempotent(tmp_path: Path) -> None:
    authored = tmp_path / "authored"
    staged = tmp_path / "staged"
    _write(authored / "keep.md", "# Keep\n")
    _write(authored / "section", "# File section\n")
    _write(staged / "keep.md", "# Keep\n")
    _write(staged / "section", "# File section\n")
    _write(staged / "build.json", '{"builder": "zensical"}\n')
    manifest = (staged / "build.json").read_bytes()
    (authored / "section").unlink()
    _write(authored / "section/page.md", "# Dir page\n")
    assert mirror_docs_once(authored, staged)["copied"] >= 1
    assert (staged / "section/page.md").read_text(encoding="utf-8") == "# Dir page\n"
    assert (staged / "section").is_dir()
    assert (staged / "keep.md").read_text(encoding="utf-8") == "# Keep\n"
    assert (staged / "build.json").read_bytes() == manifest
    assert mirror_docs_once(authored, staged) == {"copied": 0, "deleted": 0}
    assert (staged / "build.json").read_bytes() == manifest


def test_mirror_replaces_directory_with_file(tmp_path: Path) -> None:
    authored = tmp_path / "authored"
    staged = tmp_path / "staged"
    _write(authored / "section/page.md", "# Dir page\n")
    _write(staged / "section/page.md", "# Dir page\n")
    _write(staged / "build.json", '{"builder": "zensical"}\n')
    manifest = (staged / "build.json").read_bytes()
    import shutil

    shutil.rmtree(authored / "section")
    _write(authored / "section", "# File section\n")
    stats = mirror_docs_once(authored, staged)
    assert stats["copied"] >= 1
    assert (staged / "section").is_file()
    assert (staged / "section").read_text(encoding="utf-8") == "# File section\n"
    assert not (staged / "section/page.md").exists()
    assert (staged / "build.json").read_bytes() == manifest
    assert mirror_docs_once(authored, staged) == {"copied": 0, "deleted": 0}


def test_mirror_rejects_escaping_symlink_before_staged_mutation(tmp_path: Path) -> None:
    authored = tmp_path / "authored"
    staged = tmp_path / "staged"
    _write(authored / "keep.md", "# Keep\n")
    _write(staged / "keep.md", "# Keep\n")
    _write(staged / "stale.md", "# Stale\n")
    _write(staged / "build.json", '{"builder": "zensical"}\n')
    outside = tmp_path / "outside.md"
    _write(outside, "# Outside\n")
    (authored / "escape.md").symlink_to(outside)
    with pytest.raises(DocsError, match="escapes docs/"):
        mirror_docs_once(authored, staged)
    assert (staged / "keep.md").read_text(encoding="utf-8") == "# Keep\n"
    assert (staged / "stale.md").is_file()
    assert (staged / "build.json").read_text(
        encoding="utf-8"
    ) == '{"builder": "zensical"}\n'
    assert not (staged / "escape.md").exists()

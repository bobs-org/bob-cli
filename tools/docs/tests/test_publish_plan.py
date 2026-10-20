import json
import os
import subprocess
from pathlib import Path

import pytest

from bob_docs.errors import DocsError
from bob_docs.assemble import assemble
from bob_docs.mounts import load_production_sources, verify_production_clean
from bob_docs.publish_plan import fingerprint_inputs, run_publish_plan


def _run(cwd: Path, *args: str, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        args,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    return result.stdout.strip()


def _git_env() -> dict[str, str]:
    return {
        **os.environ,
        "GIT_AUTHOR_NAME": "Docs planner tests",
        "GIT_AUTHOR_EMAIL": "docs-planner@example.invalid",
        "GIT_COMMITTER_NAME": "Docs planner tests",
        "GIT_COMMITTER_EMAIL": "docs-planner@example.invalid",
    }


def _init_bare(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    _run(path.parent, "git", "init", "--bare", "--initial-branch=main", str(path))
    return path


def _mount_yaml(mounts: list[tuple[str, str, str, str, str]]) -> str:
    lines: list[str] = []
    for repo, remote, branch, source, target in mounts:
        lines.extend(
            [
                f"- repo: {repo}",
                f"  remote: {remote}",
                f"  branch: {branch}",
                f"  source: {source}",
                f"  target: {target}",
                f"  nav_title: {repo}",
            ]
        )
    return "\n".join(lines) + ("\n" if lines else "[]\n")


def _make_remote(
    path: Path,
    *,
    files: dict[str, str],
    branch: str = "main",
) -> tuple[Path, Path, str]:
    bare = _init_bare(path.with_suffix(".git"))
    working = path
    working.mkdir(parents=True)
    _run(working, "git", "init", "--initial-branch", branch)
    for relative, content in files.items():
        destination = working / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding="utf-8")
    _run(working, "git", "add", ".")
    _run(working, "git", "commit", "-m", "Initial source", env=_git_env())
    _run(working, "git", "remote", "add", "origin", str(bare))
    _run(working, "git", "push", "-u", "origin", branch)
    _run(path.parent, "git", "--git-dir", str(bare), "symbolic-ref", "HEAD", f"refs/heads/{branch}")
    return bare, working, _run(working, "git", "rev-parse", "HEAD")


def _bob_files(mounts: list[tuple[str, str, str, str, str]]) -> dict[str, str]:
    return {
        "docs/index.md": "# Bob docs\n",
        "docs/develop/index.md": "# Develop\n",
        "docs/only.md": "# Only\n",
        "docs/_meta/contracts.yml": (
            "groups:\n"
            "  - title: Group\n"
            "    entries:\n"
            "      - stem: only\n"
            "        summary: One contract\n"
            "        order: 1\n"
        ),
        "docs/_meta/mounts.yml": _mount_yaml(mounts),
        "docs/overrides/.keep": "\n",
        "mkdocs.yml": (
            "site_name: Bob\n"
            "docs_dir: docs\n"
            "site_dir: target/docs/site\n"
            "theme:\n"
            "  name: material\n"
            "  custom_dir: docs/overrides\n"
            "markdown_extensions:\n"
            "  - pymdownx.superfences\n"
            "  - custom:\n"
            "      format: !!python/name:bob_docs.fences.fence_format ''\n"
            "nav:\n"
            "  - Home: index.md\n"
        ),
        "tools/docs/pyproject.toml": "[project]\nname = 'bob-docs'\n",
        ".github/workflows/docs-publish.yml": "name: publish\n",
        ".gitignore": "/target/\n",
        "src/bob.rs": "// source code outside publication inputs\n",
    }


def _fixture(
    tmp_path: Path,
    *,
    mounts: list[tuple[str, str, str, str, str]] | None = None,
    extra_plugin_files: dict[str, str] | None = None,
) -> dict[str, Path | str]:
    mounts = mounts or []
    plugin_bare: Path | None = None
    plugin_working: Path | None = None
    if any(item[0] == "bob-plugins" for item in mounts):
        plugin_bare, plugin_working, _ = _make_remote(
            tmp_path / "bob-plugins",
            files={"docs/index.md": "# Plugins\n", **(extra_plugin_files or {})},
        )
        mounts = [
            (repo, str(plugin_bare) if repo == "bob-plugins" else remote, branch, source, target)
            for repo, remote, branch, source, target in mounts
        ]

    bob_bare, bob_working, bob_sha = _make_remote(
        tmp_path / "bob-cli-source",
        files=_bob_files(mounts),
    )
    tooling = tmp_path / "tooling"
    tooling.mkdir()
    _run(tooling, "git", "init", "--initial-branch", "main")
    (tooling / "README.md").write_text("tooling checkout\n", encoding="utf-8")
    _run(tooling, "git", "add", "README.md")
    _run(tooling, "git", "commit", "-m", "Tooling checkout", env=_git_env())
    _run(tooling, "git", "remote", "add", "origin", str(bob_bare))
    return {
        "tooling": tooling,
        "bob_bare": bob_bare,
        "bob_working": bob_working,
        "bob_sha": bob_sha,
        "plugin_bare": plugin_bare or Path(),
        "plugin_working": plugin_working or Path(),
        "manifest": tmp_path / "deploy" / "published.json",
        "output": tmp_path / "outputs",
        "work": tmp_path / "work",
    }


def _plan(
    fixture: dict[str, Path | str],
    **flags: bool,
) -> dict[str, object]:
    return run_publish_plan(
        Path(fixture["tooling"]),
        manifest=Path(fixture["manifest"]),
        output_dir=Path(fixture["output"]),
        work_dir=Path(fixture["work"]),
        **flags,
    )


def _record_publication(fixture: dict[str, Path | str], plan: dict[str, object], *, hold: bool = False) -> None:
    candidate = json.loads(Path(str(plan["next_publication_file"])).read_text(encoding="utf-8"))
    candidate.update(
        {
            "hold": hold,
            "tools": {"bob-docs": "0.1.0", "python": "3.12.0", "zensical": "0.0.71"},
            "built_at": "2026-10-20T00:00:00Z",
            "run_url": "https://github.com/bobs-org/bob-cli/actions/runs/42",
        }
    )
    path = Path(fixture["manifest"])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(candidate), encoding="utf-8")


def _commit_and_push(working: Path, *args: str) -> str:
    _run(working, "git", *args)
    _run(working, "git", "add", ".")
    _run(working, "git", "commit", "-m", "Advance source", env=_git_env())
    _run(working, "git", "push", "origin", "main")
    return _run(working, "git", "rev-parse", "HEAD")


def test_first_publish_writes_exact_sources_schema_and_candidate_without_touching_state(
    tmp_path: Path,
) -> None:
    fixture = _fixture(tmp_path)
    plan = _plan(fixture)

    assert plan["should_build"] is True
    assert plan["should_deploy"] is True
    assert plan["should_record"] is True
    assert plan["reason"] == "first-publish"
    assert not Path(fixture["manifest"]).exists()
    sources = json.loads(Path(str(plan["sources_file"])).read_text(encoding="utf-8"))
    assert set(sources) == {"sources"}
    assert set(sources["sources"]) == {"bob-cli"}
    assert set(sources["sources"]["bob-cli"]) == {"path", "sha"}
    selected = Path(fixture["work"]) / "bob-cli"
    loaded = load_production_sources(
        Path(str(plan["sources_file"])), root=selected, repos=[]
    )
    verify_production_clean(loaded)
    context, build_manifest = assemble(
        selected,
        production=True,
        sources=Path(str(plan["sources_file"])),
    )
    assert context.assembled_config.is_file()
    assert json.loads(build_manifest.read_text(encoding="utf-8"))["sources"]["bob-cli"]["sha"] == plan[
        "sources"
    ]["bob-cli"]["sha"]
    candidate = json.loads(Path(str(plan["next_publication_file"])).read_text(encoding="utf-8"))
    assert candidate["schema_version"] == 1
    assert candidate["hold"] is False
    assert candidate["fingerprint"] == plan["fingerprint"]


def test_equal_source_shas_skip_without_acquiring_or_mutating_manifest(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    before = Path(fixture["manifest"]).read_bytes()

    second = _plan(fixture)

    assert second["should_build"] is False
    assert second["reason"] == "same-sources"
    assert second["sources_file"] is None
    assert Path(fixture["manifest"]).read_bytes() == before


def test_code_only_commit_with_same_input_fingerprint_skips(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    bob_working = Path(fixture["bob_working"])
    (bob_working / "src/bob.rs").write_text("// code only change\n", encoding="utf-8")
    _commit_and_push(bob_working, "add", "src/bob.rs")

    second = _plan(fixture)

    assert second["should_build"] is False
    assert second["reason"] == "same-inputs"
    assert second["fingerprint"] == first["fingerprint"]


def test_documentation_tree_change_builds(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    bob_working = Path(fixture["bob_working"])
    (bob_working / "docs/index.md").write_text("# Updated docs\n", encoding="utf-8")
    _commit_and_push(bob_working, "add", "docs/index.md")

    second = _plan(fixture)

    assert second["should_build"] is True
    assert second["reason"] == "inputs-changed"
    assert second["fingerprint"] != first["fingerprint"]


def test_two_mounts_from_one_repo_are_resolved_once_and_fingerprinted_twice(
    tmp_path: Path,
) -> None:
    mounts = [
        ("bob-plugins", "unused", "main", "docs", "reference/plugins"),
        ("bob-plugins", "unused", "main", "docs/extra", "develop/plugins"),
    ]
    fixture = _fixture(
        tmp_path,
        mounts=mounts,
        extra_plugin_files={"docs/extra/index.md": "# Plugin development\n"},
    )
    plan = _plan(fixture)
    sources = json.loads(Path(str(plan["sources_file"])).read_text(encoding="utf-8"))
    candidate = json.loads(Path(str(plan["next_publication_file"])).read_text(encoding="utf-8"))

    assert set(sources["sources"]) == {"bob-cli", "bob-plugins"}
    plugin_inputs = [entry for entry in candidate["inputs"] if entry["repo"] == "bob-plugins"]
    assert {(entry["path"], entry["target"]) for entry in plugin_inputs} == {
        ("docs", "reference/plugins"),
        ("docs/extra", "develop/plugins"),
    }


def test_conflicting_mount_remote_or_branch_is_rejected(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    bob_working = Path(fixture["bob_working"])
    mounts = [
        ("bob-plugins", "file:///one.git", "main", "docs", "reference/plugins"),
        ("bob-plugins", "file:///two.git", "main", "docs/extra", "develop/plugins"),
    ]
    (bob_working / "docs/_meta/mounts.yml").write_text(_mount_yaml(mounts), encoding="utf-8")
    _commit_and_push(bob_working, "add", "docs/_meta/mounts.yml")

    with pytest.raises(DocsError, match="conflicting remote or branch"):
        _plan(fixture)


def test_workflow_addition_and_removal_change_fingerprint(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    bob_working = Path(fixture["bob_working"])
    extra = bob_working / ".github/workflows/docs-weekly.yml"
    extra.write_text("name: weekly\n", encoding="utf-8")
    _commit_and_push(bob_working, "add", ".github/workflows/docs-weekly.yml")
    added = _plan(fixture)
    assert added["should_build"] is True
    assert added["fingerprint"] != first["fingerprint"]
    _record_publication(fixture, added)
    extra.unlink()
    _commit_and_push(bob_working, "rm", ".github/workflows/docs-weekly.yml")
    removed = _plan(fixture)
    assert removed["should_build"] is True
    assert removed["fingerprint"] != added["fingerprint"]


def test_mount_addition_and_removal_change_fingerprint(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    plugin_bare, _plugin_working, _plugin_sha = _make_remote(
        tmp_path / "bob-plugins",
        files={"docs/index.md": "# Plugins\n"},
    )
    bob_working = Path(fixture["bob_working"])
    (bob_working / "docs/_meta/mounts.yml").write_text(
        _mount_yaml(
            [("bob-plugins", str(plugin_bare), "main", "docs", "reference/plugins")]
        ),
        encoding="utf-8",
    )
    _commit_and_push(bob_working, "add", "docs/_meta/mounts.yml")
    added = _plan(fixture)
    assert added["should_build"] is True
    assert set(added["sources"]) == {"bob-cli", "bob-plugins"}
    assert added["fingerprint"] != first["fingerprint"]
    _record_publication(fixture, added)

    (bob_working / "docs/_meta/mounts.yml").write_text("[]\n", encoding="utf-8")
    _commit_and_push(bob_working, "add", "docs/_meta/mounts.yml")
    removed = _plan(fixture)
    assert removed["should_build"] is True
    assert set(removed["sources"]) == {"bob-cli"}


def test_branch_movement_after_discovery_fetches_the_observed_sha(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    mounts = [("bob-plugins", "unused", "main", "docs", "reference/plugins")]
    fixture = _fixture(tmp_path, mounts=mounts)
    plugin_working = Path(fixture["plugin_working"])
    observed = _run(plugin_working, "git", "rev-parse", "HEAD")
    from bob_docs import publish_plan

    original_resolve = publish_plan._resolve_branch
    moved = False

    def move_after_resolve(remote: str, branch: str, *, cwd: Path) -> str:
        nonlocal moved
        sha = original_resolve(remote, branch, cwd=cwd)
        if not moved:
            moved = True
            (plugin_working / "docs/index.md").write_text("# Moved after discovery\n", encoding="utf-8")
            _commit_and_push(plugin_working, "add", "docs/index.md")
        return sha

    monkeypatch.setattr(publish_plan, "_resolve_branch", move_after_resolve)
    plan = _plan(fixture)
    selected = Path(fixture["work"]) / "bob-plugins"

    assert plan["sources"]["bob-plugins"]["sha"] == observed
    assert _run(selected, "git", "rev-parse", "HEAD") == observed
    assert _run(plugin_working, "git", "rev-parse", "HEAD") != observed


def test_unavailable_mount_ref_fails_closed(tmp_path: Path) -> None:
    mounts = [("bob-plugins", "unused", "missing", "docs", "reference/plugins")]
    fixture = _fixture(tmp_path, mounts=mounts)

    with pytest.raises(DocsError, match="branch 'missing' is unavailable"):
        _plan(fixture)


def test_malformed_publication_manifest_fails_closed(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    path = Path(fixture["manifest"])
    path.parent.mkdir(parents=True)
    path.write_text("{not json", encoding="utf-8")

    with pytest.raises(DocsError, match="invalid JSON"):
        _plan(fixture)


def test_unsupported_manifest_schema_fails_closed(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    path = Path(fixture["manifest"])
    state = json.loads(path.read_text(encoding="utf-8"))
    state["schema_version"] = 2
    path.write_text(json.dumps(state), encoding="utf-8")

    with pytest.raises(DocsError, match="unsupported schema_version"):
        _plan(fixture)


@pytest.mark.parametrize("flag", ["force", "drill", "recovery"])
def test_equal_sources_rebuild_for_force_drill_or_recovery(tmp_path: Path, flag: str) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)

    plan = _plan(fixture, **{flag: True})

    assert plan["should_build"] is True
    assert plan["reason"] == {"force": "forced", "drill": "drill", "recovery": "recovery"}[flag]
    assert plan["should_deploy"] is (flag != "drill")
    assert plan["should_record"] is (flag != "drill")


def test_hold_blocks_force_drill_and_recovery_but_pinned_preserves_hold(
    tmp_path: Path,
) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first, hold=True)

    held = _plan(fixture, force=True, drill=True, recovery=True)
    assert held["should_build"] is False
    assert held["reason"] == "held"

    pinned = _plan(fixture, pinned=True)
    assert pinned["should_build"] is True
    assert pinned["reason"] == "pinned"
    candidate = json.loads(Path(str(pinned["next_publication_file"])).read_text(encoding="utf-8"))
    assert candidate["hold"] is True


def test_pinned_rejects_missing_manifest(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    with pytest.raises(DocsError, match="--pinned requires"):
        _plan(fixture, pinned=True)


def test_pinned_drill_never_deploys_or_records(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)

    plan = _plan(fixture, pinned=True, drill=True)

    assert plan["should_build"] is True
    assert plan["reason"] == "drill-pinned"
    assert plan["should_deploy"] is False
    assert plan["should_record"] is False


def test_pinned_uses_recorded_sources_after_the_default_branch_moves(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    pinned_sha = first["sources"]["bob-cli"]["sha"]
    bob_working = Path(fixture["bob_working"])
    (bob_working / "docs/index.md").write_text("# Later docs\n", encoding="utf-8")
    _commit_and_push(bob_working, "add", "docs/index.md")

    pinned = _plan(fixture, pinned=True)

    assert pinned["should_build"] is True
    assert pinned["sources"]["bob-cli"]["sha"] == pinned_sha
    assert _run(Path(fixture["work"]) / "bob-cli", "git", "rev-parse", "HEAD") == pinned_sha


def test_pinned_source_membership_must_match_selected_mount_table(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    first = _plan(fixture)
    _record_publication(fixture, first)
    state_path = Path(fixture["manifest"])
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["sources"]["stale"] = {
        "remote": "file:///stale.git",
        "branch": "main",
        "sha": state["sources"]["bob-cli"]["sha"],
    }
    state_path.write_text(json.dumps(state), encoding="utf-8")

    with pytest.raises(DocsError, match="does not match the selected mount table"):
        _plan(fixture, pinned=True)


def test_deterministic_fingerprint_ignores_input_order_and_local_paths() -> None:
    inputs = [
        {"repo": "bob-cli", "path": "docs", "kind": "tree", "object_id": "a" * 40},
        {
            "repo": "bob-plugins",
            "path": "docs",
            "kind": "tree",
            "object_id": "b" * 40,
            "target": "reference/plugins",
        },
    ]

    assert fingerprint_inputs(inputs) == fingerprint_inputs(list(reversed(inputs)))
    assert len(fingerprint_inputs(inputs)) == 64


def test_github_output_contains_stable_safe_fields(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    output_file = tmp_path / "github-output"
    plan = run_publish_plan(
        Path(fixture["tooling"]),
        manifest=Path(fixture["manifest"]),
        output_dir=Path(fixture["output"]),
        work_dir=Path(fixture["work"]),
        github_output=output_file,
    )

    values = dict(line.split("=", 1) for line in output_file.read_text(encoding="utf-8").splitlines())
    assert values["should_build"] == "true"
    assert values["should_deploy"] == "true"
    assert values["should_record"] == "true"
    assert values["sources_file"] == plan["sources_file"]
    assert values["next_publication_file"] == plan["next_publication_file"]


def test_manifest_identity_cannot_be_a_planner_output(tmp_path: Path) -> None:
    fixture = _fixture(tmp_path)
    with pytest.raises(DocsError, match="must not point at a planner output"):
        run_publish_plan(
            Path(fixture["tooling"]),
            manifest=Path(fixture["output"]) / "next-publication.json",
            output_dir=Path(fixture["output"]),
            work_dir=Path(fixture["work"]),
        )

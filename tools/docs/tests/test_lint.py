"""Positive and negative fixtures for every documentation contract family."""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

from bob_docs import cli, mounts
from bob_docs.assemble import assemble
from bob_docs.cli import _parser
from bob_docs.lint import (
    build_lint_context,
    check_privacy,
    check_stale_ceilings,
    check_test_pins,
    check_tree,
    run_lint,
)
from bob_docs.markdown import count_words
from bob_docs.yaml_tools import load_yaml

ROOT = Path(__file__).resolve().parents[3]
GOOD_FM = "---\ntitle: T\ndescription: Short line.\n---\n\n"


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _site(root: Path) -> Path:
    docs = root / "docs"
    _write(docs / "index.md", GOOD_FM + "# Home\n\nWelcome.\n")
    _write(docs / "develop/index.md", GOOD_FM + "# Develop\n\nHub sentence.\n")
    _write(docs / "develop/build.md", GOOD_FM + "# Build\n\nHow to build.\n")
    _write(docs / "only.md", "# Only contract\n\nA spec.\n")
    _write(
        docs / "_meta/contracts.yml",
        "groups:\n  - title: Group\n    entries:\n"
        "      - stem: only\n        summary: One contract\n        order: 1\n",
    )
    _write(docs / "_meta/mounts.yml", "[]\n")
    _write(docs / "_meta/budgets.yml", "ceilings: {}\n")
    _write(docs / "_meta/media.yml", "{}\n")
    _write(
        docs / "_meta/privacy.yml",
        (ROOT / "docs/_meta/privacy.yml").read_text(encoding="utf-8"),
    )
    _write(
        docs / "_meta/privacy_baseline.txt",
        "# Contract privacy baseline. Columns: path, rule, count, matched.\n",
    )
    _write(docs / "overrides/.keep", "")
    _write(
        root / "mkdocs.yml",
        "site_name: Test\n"
        "docs_dir: docs\n"
        "site_dir: target/docs/site\n"
        "exclude_docs: |\n  README.md\n  _meta/\n  _templates/\n  overrides/\n"
        "plugins:\n  - search\n  - redirects: {}\n"
        "nav:\n"
        "  - Home: index.md\n"
        "  - Develop:\n"
        "      - Overview: develop/index.md\n"
        "      - Build: develop/build.md\n"
        "      - Only: only.md\n",
    )
    (root / "src").mkdir()
    (root / "tests").mkdir()
    return root


def _errors(root: Path) -> list[str]:
    ctx = build_lint_context(root)
    manifest = ctx.root / "target/docs/mount-manifest.json"
    if manifest.is_file():
        raw = json.loads(manifest.read_text(encoding="utf-8"))
        if isinstance(raw, dict) and isinstance(raw.get("sources"), dict):
            ctx.source_stamps = raw["sources"]
    findings = check_tree(ctx)
    return [finding.format() for finding in findings if finding.severity == "error"]


def _mounted_fixture(root: Path, *, source: str = "bob-plugins:docs/copied.md") -> Path:
    _write(
        root / "docs/_meta/mounts.yml",
        "- repo: bob-plugins\n"
        "  remote: https://example.com/bob-plugins.git\n"
        "  branch: main\n"
        "  source: docs\n"
        "  target: reference/plugins\n"
        "  nav_title: Plugins\n",
    )
    _write(
        root / ".gitignore",
        "/docs/reference/plugins/*\n!/docs/reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", GOOD_FM + "# Plugins\n")
    _write(
        root / "docs/reference/plugins/copied.md",
        f"---\ntitle: Copied\ndescription: Mounted copy.\nsource: {source}\n---\n\n# Copied\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml",
        nav.replace(
            "      - Only: only.md\n",
            "      - Only: only.md\n"
            "  - Plugins:\n"
            "      - Overview: reference/plugins/index.md\n"
            "      - Copied: reference/plugins/copied.md\n",
        ),
    )
    (root / "target/docs").mkdir(parents=True, exist_ok=True)
    _write(
        root / "target/docs/mount-manifest.json",
        json.dumps(
            {
                "files": ["reference/plugins/copied.md"],
                "sources": {
                    "reference/plugins/copied.md": "bob-plugins:docs/copied.md"
                },
            }
        ),
    )
    subprocess.run(["git", "init", "-q"], cwd=root, check=True, capture_output=True)
    return root


def test_clean_fixture_passes(tmp_path: Path) -> None:
    assert _errors(_site(tmp_path)) == []


def test_environment_guard_runs_in_normal_lint(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "src/main.rs", 'const ENV: &str = "BOB_LINT_DISCOVERY";\n')
    findings = [
        item
        for item in check_tree(build_lint_context(root))
        if item.rule == "environment"
    ]
    assert len(findings) == 1
    assert findings[0].path == "docs/reference/configuration.md"
    assert "missing configuration reference" in findings[0].explanation


def test_staged_build_json_is_not_linted(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/build.json", '{"builder": "zensical"}\n')
    assert _errors(root) == []


def test_real_site_passes_lint() -> None:
    assert run_lint(ROOT) == 0


def test_documented_mount_destinations_are_gitignored() -> None:
    for target in (
        "docs/reference/plugins/page.md",
        "docs/reference/plugins/index.md",
        "docs/reference/mac-capture/page.md",
        "docs/develop/plugins/page.md",
        "docs/develop/plugins/index.md",
        "docs/develop/mac-capture/page.md",
    ):
        result = subprocess.run(
            ["git", "check-ignore", "-q", "--no-index", target],
            cwd=ROOT,
            check=False,
        )
        assert result.returncode == 0, target
    for landing in (
        "docs/reference/mac-capture/index.md",
        "docs/develop/mac-capture/index.md",
    ):
        result = subprocess.run(
            ["git", "check-ignore", "-q", "--no-index", landing],
            cwd=ROOT,
            check=False,
        )
        assert result.returncode == 1, landing


def test_contract_grammar_examples_survive() -> None:
    text = (ROOT / "docs/capture.md").read_text(encoding="utf-8")
    assert "`!!!`" in text or "!!!`" in text
    assert "++N" in text
    completion = (ROOT / "docs/completion.md").read_text(encoding="utf-8")
    assert "$SHELL" in completion
    assert run_lint(ROOT) == 0


def test_untyped_root_page_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/brand-new.md", GOOD_FM + "# New\n")
    joined = "\n".join(_errors(root))
    assert "docs/brand-new.md:1: typing:" in joined


def test_stem_typed_dir_collision_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/start.md", "# Start contract\n")
    contracts = (root / "docs/_meta/contracts.yml").read_text(encoding="utf-8")
    _write(
        root / "docs/_meta/contracts.yml",
        contracts.replace("stem: only", "stem: start").replace("One contract", "Start"),
    )
    (root / "docs/only.md").unlink()
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav.replace("only.md", "start.md"))
    joined = "\n".join(_errors(root))
    assert "collides with a typed directory" in joined


def test_missing_front_matter_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/develop/build.md", "# Build\n")
    joined = "\n".join(_errors(root))
    assert "missing front matter key 'title'" in joined
    assert "missing front matter key 'description'" in joined


def test_malformed_yaml_front_matter_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/develop/build.md", "---\n: : :\n---\n\n# Build\n")
    joined = "\n".join(_errors(root))
    assert "docs/develop/build.md:1: yaml:" in joined
    assert "malformed front matter" in joined


def test_long_description_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        "---\ntitle: T\ndescription: " + "x" * 161 + "\n---\n\n# Build\n",
    )
    joined = "\n".join(_errors(root))
    assert "'description' must be one line" in joined


def test_unknown_front_matter_key_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        "---\ntitle: T\ndescription: Short.\nnope: 1\n---\n\n# Build\n",
    )
    joined = "\n".join(_errors(root))
    assert "unknown front matter key 'nope'" in joined


def test_forged_source_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        "---\ntitle: T\ndescription: Short line.\nsource: bob-plugins:docs/build.md\n---\n\n# Build\n",
    )
    joined = "\n".join(_errors(root))
    assert "docs/develop/build.md:1: source:" in joined
    assert "forged" in joined


def test_verified_mount_source_passes(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path))
    assert _errors(root) == []


def test_unassembled_lint_skips_gitignored_mount_residue(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path))
    (root / "target/docs/mount-manifest.json").unlink()
    image = root / "docs/reference/plugins/images/panel.png"
    image.parent.mkdir()
    image.write_bytes(b"\x89PNG\r\n\x1a\n")
    assert _errors(root) == []

    # An authored image in the mount must still have provenance.
    with (root / ".gitignore").open("a", encoding="utf-8") as stream:
        stream.write(
            "!/docs/reference/plugins/images/\n"
            "!/docs/reference/plugins/images/panel.png\n"
        )
    assert any(
        "docs/reference/plugins/images/panel.png:1: media:" in item
        for item in _errors(root)
    )


def test_copied_hub_source_stamp_is_verified(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path))
    _write(
        root / ".gitignore",
        "/docs/reference/plugins/*\n",
    )
    (root / "docs/reference/plugins/index.md").unlink()
    _write(
        root / "docs/reference/plugins/index.md",
        "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n"
        "source: bob-plugins:docs/index.md\n---\n\n# Plugins\n",
    )
    manifest = json.loads(
        (root / "target/docs/mount-manifest.json").read_text(encoding="utf-8")
    )
    manifest["files"].append("reference/plugins/index.md")
    manifest["sources"]["reference/plugins/index.md"] = "bob-plugins:docs/index.md"
    (root / "target/docs/mount-manifest.json").write_text(
        json.dumps(manifest) + "\n", encoding="utf-8"
    )
    assert _errors(root) == []


def test_copied_hub_forged_source_fails(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path))
    _write(
        root / ".gitignore",
        "/docs/reference/plugins/*\n",
    )
    (root / "docs/reference/plugins/index.md").unlink()
    _write(
        root / "docs/reference/plugins/index.md",
        "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n"
        "source: bob-plugins:docs/forged.md\n---\n\n# Plugins\n",
    )
    manifest = json.loads(
        (root / "target/docs/mount-manifest.json").read_text(encoding="utf-8")
    )
    manifest["files"].append("reference/plugins/index.md")
    manifest["sources"]["reference/plugins/index.md"] = "bob-plugins:docs/index.md"
    (root / "target/docs/mount-manifest.json").write_text(
        json.dumps(manifest) + "\n", encoding="utf-8"
    )
    joined = "\n".join(_errors(root))
    assert "docs/reference/plugins/index.md:1: source:" in joined
    assert "forged" in joined


def test_source_owned_hub_must_be_gitignored(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path))
    _write(
        root / "docs/reference/plugins/index.md",
        "---\ntitle: Plugins inventory\ndescription: Plugin hub.\n"
        "source: bob-plugins:docs/index.md\n---\n\n# Plugins\n",
    )
    manifest = json.loads(
        (root / "target/docs/mount-manifest.json").read_text(encoding="utf-8")
    )
    manifest["files"].append("reference/plugins/index.md")
    manifest["sources"]["reference/plugins/index.md"] = "bob-plugins:docs/index.md"
    (root / "target/docs/mount-manifest.json").write_text(
        json.dumps(manifest) + "\n", encoding="utf-8"
    )
    joined = "\n".join(_errors(root))
    assert "source-owned hub" in joined
    assert "must be gitignored" in joined


def test_mount_source_must_match_the_assembled_stamp(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path), source="bob-plugins:docs/elsewhere.md")
    joined = "\n".join(_errors(root))
    assert "docs/reference/plugins/copied.md:1: source:" in joined
    assert "forged" in joined


def test_homepage_word_cap(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/index.md", GOOD_FM + "# Home\n\n" + "word " * 410)
    joined = "\n".join(_errors(root))
    assert "400-word cap" in joined


def test_start_page_and_total_budgets_fail(tmp_path: Path) -> None:
    root = _site(tmp_path)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    start_nav = ["  - Start:"]
    for index in range(5):
        name = f"page{index}.md"
        _write(root / "docs/start" / name, GOOD_FM + "# Start\n\n" + "word " * 2200)
        start_nav.append(f"      - Page {index}: start/{name}")
    _write(root / "mkdocs.yml", nav + "\n" + "\n".join(start_nav) + "\n")
    findings = check_tree(build_lint_context(root))
    errors = [finding.format() for finding in findings if finding.severity == "error"]
    assert any("cap 10000" in item and "start/" in item for item in errors)
    assert not any("cap 2500 for start/" in item for item in errors)

    _write(root / "docs/start/oversize.md", GOOD_FM + "# Start\n\n" + "word " * 2501)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "      - Oversize: start/oversize.md\n")
    assert any("cap 2500 for start/" in item for item in _errors(root))


def test_concepts_hub_and_reference_budgets_fail(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/concepts/model.md", GOOD_FM + "# Model\n\n" + "word " * 3001)
    _write(
        root / "docs/reference/index.md", GOOD_FM + "# Reference\n\n" + "word " * 401
    )
    _write(root / "docs/reference/thing.md", GOOD_FM + "# Thing\n\n" + "word " * 6001)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml",
        nav
        + "\n  - Concepts:\n      - Model: concepts/model.md\n"
        + "  - Reference:\n      - Overview: reference/index.md\n      - Thing: reference/thing.md\n",
    )
    joined = "\n".join(_errors(root))
    assert "cap 3000 for concepts/" in joined
    assert "400 words" in joined and "reference/index.md" in joined
    assert "cap 6000 for reference/" in joined


def test_mounted_handwritten_reference_page_keeps_reference_budget(
    tmp_path: Path,
) -> None:
    root = _mounted_fixture(_site(tmp_path))
    _write(
        root / "docs/reference/plugins/copied.md",
        "---\ntitle: Copied\ndescription: Mounted copy.\n"
        "source: bob-plugins:docs/copied.md\n---\n\n# Copied\n\n" + "word " * 6001,
    )
    joined = "\n".join(_errors(root))
    assert "cap 6000 for reference/" in joined


def test_develop_budget_is_a_warning(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/develop/long.md", GOOD_FM + "# Long\n\n" + "word " * 8001)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml",
        nav + "\n  - Develop Extra:\n      - Long: develop/long.md\n",
    )
    findings = check_tree(build_lint_context(root))
    assert any(
        item.severity == "warning"
        and item.path == "docs/develop/long.md"
        and "8000" in item.explanation
        for item in findings
    )
    assert not any(
        item.severity == "error"
        and item.path == "docs/develop/long.md"
        and "8000" in item.explanation
        for item in findings
    )


def test_guides_over_budget_fails_and_warns(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/guides/doing.md", GOOD_FM + "# Doing\n\n" + "word " * 1600)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Guides:\n      - Doing: guides/doing.md\n")
    findings = check_tree(build_lint_context(root))
    assert any(
        item.severity == "warning" and "1500-word" in item.explanation
        for item in findings
    )
    _write(root / "docs/guides/doing.md", GOOD_FM + "# Doing\n\n" + "word " * 2600)
    joined = "\n".join(_errors(root))
    assert "over budget" in joined


def test_generated_page_is_budget_exempt_but_needs_metadata(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/reference/gen.md",
        "<!-- generated by bob-docs; run just docs-gen -->\n\n# Big\n\n"
        + "word " * 9000
        + "\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Reference:\n      - Gen: reference/gen.md\n")
    joined = "\n".join(_errors(root))
    assert "over budget" not in joined
    assert "missing front matter key 'title'" in joined


def test_generated_page_privacy_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/reference/gen.md",
        GOOD_FM + "<!-- generated by bob-docs; run just docs-gen -->\n\n"
        "# Gen\n\nSee /home/bryan/secret.\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Reference:\n      - Gen: reference/gen.md\n")
    joined = "\n".join(_errors(root))
    assert "privacy:" in joined
    assert "/home/bryan" in joined


def test_hygiene_patterns_fail_in_guides(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/guides/doing.md",
        GOOD_FM + "# Doing\n\nSee bob-cli-a0.2.1 and src/native.rs. "
        "Formerly in phase bob-cli-a0 (E3) and renamed.\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Guides:\n      - Doing: guides/doing.md\n")
    joined = "\n".join(_errors(root))
    assert "bead IDs" in joined
    assert "src/ paths" in joined
    assert "phase labels" in joined
    assert "history phrases" in joined


def test_hygiene_skips_code_and_link_destinations(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/guides/doing.md",
        GOOD_FM + "# Doing\n\n"
        "See the [diagram](bob-cli-a0.png).\n\n"
        "```bash\ncat src/native.rs  # bob-cli-a0.2\n```\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Guides:\n      - Doing: guides/doing.md\n")
    assert _errors(root) == []


def test_reference_forbids_bead_ids_only(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/reference/thing.md",
        GOOD_FM + "# Thing\n\nSee src/native.rs and bob-cli-a0.2.\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml", nav + "  - Reference:\n      - Thing: reference/thing.md\n"
    )
    joined = "\n".join(_errors(root))
    assert "bead IDs" in joined
    assert "src/ paths" not in joined


def test_test_pin_fails_and_exemption_is_exact(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "src/lib.rs", 'include_str!("docs/guides/doing.md");\n')
    joined = "\n".join(_errors(root))
    assert "test-pin" in joined
    _write(root / "src/lib.rs", "fn ok() {}\n")
    _write(
        root / "tests/cli/docs_tutorial.rs", 'include_str!("docs/guides/doing.md");\n'
    )
    assert _errors(root) == []
    _write(
        root / "tests/cli/docs_tutorial_extra.rs",
        'include_str!("docs/guides/doing.md");\n',
    )
    joined = "\n".join(_errors(root))
    assert "docs_tutorial_extra.rs" in joined


def test_lint_never_discovers_mount_repositories(tmp_path: Path, monkeypatch) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/_meta/mounts.yml",
        "- repo: bob-plugins\n"
        "  remote: https://example.com/bob-plugins.git\n"
        "  branch: main\n"
        "  source: docs\n"
        "  target: reference/plugins\n"
        "  nav_title: Plugins\n",
    )

    def fail_discovery(*_args, **_kwargs):
        raise AssertionError("lint attempted repository discovery")

    monkeypatch.setattr(mounts, "discover_repo", fail_discovery)
    assert check_test_pins(build_lint_context(root)) == []


def test_mounted_js_test_pin_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/_meta/mounts.yml",
        "- repo: bob-plugins\n"
        "  remote: https://example.com/bob-plugins.git\n"
        "  branch: main\n"
        "  source: docs\n"
        "  target: reference/plugins\n"
        "  nav_title: Plugins\n",
    )
    source = root / "plugins-source"
    _write(source / "tests/example.test.js", 'readDoc("docs/guides/doing.md");\n')
    ctx = build_lint_context(root)
    ctx.source_dirs["bob-plugins"] = source
    findings = check_test_pins(ctx)
    assert len(findings) == 1
    assert findings[0].path == "bob-plugins/tests/example.test.js"
    assert findings[0].rule == "test-pin"


def test_hooks_and_unknown_plugin_fail(tmp_path: Path) -> None:
    root = _site(tmp_path)
    config = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", config + "hooks:\n  - tools/hook.py\n")
    joined = "\n".join(_errors(root))
    assert "'hooks:' is forbidden" in joined
    _write(
        root / "mkdocs.yml",
        config.replace("  - search\n", "  - search\n  - gen-files\n"),
    )
    joined = "\n".join(_errors(root))
    assert "plugin 'gen-files'" in joined


def test_shell_prompt_fails_but_inline_dollar_survives(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        GOOD_FM + "# Build\n\n```bash\n$ just docs-build\n```\n",
    )
    joined = "\n".join(_errors(root))
    assert "portability" in joined
    assert "$ prompt" in joined
    _write(
        root / "docs/develop/build.md",
        GOOD_FM + "# Build\n\n```bash\njust docs-build  # $SHELL stays\n```\n\n"
        '```text title="Output" .no-copy\n$ not a prompt in text\n```\n',
    )
    assert _errors(root) == []


def test_admonition_tabs_and_keys_fail_outside_code(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        GOOD_FM + "# Build\n\n!!! note\n    Hi\n\n=== Tab\nbody\n\nPress ++ctrl++.\n",
    )
    joined = "\n".join(_errors(root))
    assert "!!! admonitions" in joined
    assert "=== content tabs" in joined
    assert "++key++" in joined
    _write(
        root / "docs/develop/build.md",
        GOOD_FM + "# Build\n\n`!!!` and `++N` and a table stay.\n",
    )
    assert _errors(root) == []


def test_nav_omission_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/guides/doing.md", GOOD_FM + "# Doing\n")
    joined = "\n".join(_errors(root))
    assert "docs/guides/doing.md:1: nav:" in joined
    assert "missing from nav" in joined


def test_assembled_lint_uses_expanded_nav_and_exact_mount_manifest(
    tmp_path: Path, monkeypatch
) -> None:
    root = _site(tmp_path / "bob-cli")
    _write(
        root / "docs/_meta/mounts.yml",
        "- repo: bob-plugins\n"
        "  remote: https://example.com/bob-plugins.git\n"
        "  branch: main\n"
        "  source: docs\n"
        "  target: reference/plugins\n"
        "  nav_title: Plugins\n",
    )
    _write(
        root / ".gitignore",
        "/docs/reference/plugins/*\n!/docs/reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", GOOD_FM + "# Plugins\n")
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "\n  - Plugins: reference/plugins/index.md\n")
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml",
        nav.replace(
            "  - Develop:\n",
            "  - Reference:\n      - Configuration: reference/configuration.md\n"
            "  - Develop:\n",
        ),
    )
    _write(root / "src/main.rs", 'const ENV: &str = "BOB_ASSEMBLED_DISCOVERY";\n')
    _write(
        root / "docs/reference/configuration.md",
        GOOD_FM + "# Configuration\n\n`BOB_ASSEMBLED_DISCOVERY` is documented.\n",
    )
    subprocess.run(["git", "init", "-q"], cwd=root, check=True, capture_output=True)

    source = tmp_path / "bob-plugins"
    _write(source / "docs/nav.yml", "- title: Listed\n  path: listed.md\n")
    _write(source / "docs/listed.md", GOOD_FM + "# Listed\n\nMounted content.\n")
    image = source / "docs/images/panel.png"
    image.parent.mkdir()
    image.write_bytes(b"\x89PNG\r\n\x1a\n")
    media_path = _write(
        source / "docs/media.yml",
        "images/panel.png:\n  source: synthetic fixture\n  date: 2026-10-19\n",
    )
    subprocess.run(
        ["git", "init", "-b", "main", "-q"], cwd=source, check=True, capture_output=True
    )
    subprocess.run(
        ["git", "remote", "add", "origin", "https://example.com/bob-plugins.git"],
        cwd=source,
        check=True,
        capture_output=True,
    )
    subprocess.run(["git", "add", "-A"], cwd=source, check=True, capture_output=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "fixture",
        ],
        cwd=source,
        check=True,
        capture_output=True,
    )
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(source))

    context, _manifest = assemble(root)
    assembled_config = load_yaml(context.assembled_config)
    assert repr(assembled_config["nav"]).count("reference/plugins/listed.md") == 1
    manifest = json.loads(
        (context.output_dir / "mount-manifest.json").read_text(encoding="utf-8")
    )
    assert manifest["sources"] == {
        "reference/plugins/listed.md": "bob-plugins:docs/listed.md"
    }
    assert not [
        item
        for item in check_tree(build_lint_context(root, docs_context=context))
        if item.severity == "error"
    ]

    # Assembled checks must require source-owned provenance for mounted images.
    media = media_path.read_text(encoding="utf-8")
    media_path.write_text("{}\n", encoding="utf-8")
    findings = check_tree(build_lint_context(root, docs_context=context))
    assert any(
        item.rule == "media"
        and item.path == "docs/reference/plugins/images/panel.png"
        for item in findings
    )
    media_path.write_text(media, encoding="utf-8")

    staged_config = context.output_dir / "docs/reference/configuration.md"
    staged_config.write_text(
        staged_config.read_text(encoding="utf-8").replace(
            "BOB_ASSEMBLED_DISCOVERY", ""
        ),
        encoding="utf-8",
    )
    findings = check_tree(build_lint_context(root, docs_context=context))
    assert any(
        item.rule == "environment" and "BOB_ASSEMBLED_DISCOVERY" in item.explanation
        for item in findings
    )

    staged_page = context.output_dir / "docs/reference/plugins/listed.md"
    staged_page.write_text(
        staged_page.read_text(encoding="utf-8").replace(
            "bob-plugins:docs/listed.md", "bob-plugins:docs/forged.md"
        ),
        encoding="utf-8",
    )
    findings = check_tree(build_lint_context(root, docs_context=context))
    assert any(
        item.rule == "source" and "forged" in item.explanation for item in findings
    )

    _write(source / "docs/unlisted.md", GOOD_FM + "# Unlisted\n")
    context, _manifest = assemble(root)
    findings = check_tree(build_lint_context(root, docs_context=context))
    assert any(
        item.path == "docs/reference/plugins/unlisted.md"
        and item.rule == "nav"
        and "bob-plugins's nav.yml" in item.explanation
        for item in findings
    )


def test_assembled_lint_accepts_copied_source_hubs(tmp_path: Path, monkeypatch) -> None:
    root = _site(tmp_path / "bob-cli")
    _write(
        root / "docs/_meta/mounts.yml",
        "- repo: bob-plugins\n"
        "  remote: https://example.com/bob-plugins.git\n"
        "  branch: main\n"
        "  source: docs\n"
        "  target: reference/plugins\n"
        "  nav_title: Plugins\n",
    )
    _write(root / ".gitignore", "/docs/reference/plugins/*\n")
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "\n  - Plugins: reference/plugins/index.md\n")
    subprocess.run(["git", "init", "-q"], cwd=root, check=True, capture_output=True)

    source = tmp_path / "bob-plugins"
    _write(
        source / "docs/index.md",
        GOOD_FM + "# Plugins inventory\n\nHub sentence.\n",
    )
    _write(source / "docs/nav.yml", "- title: Listed\n  path: listed.md\n")
    _write(source / "docs/listed.md", GOOD_FM + "# Listed\n\nMounted content.\n")
    subprocess.run(
        ["git", "init", "-b", "main", "-q"], cwd=source, check=True, capture_output=True
    )
    subprocess.run(
        ["git", "remote", "add", "origin", "https://example.com/bob-plugins.git"],
        cwd=source,
        check=True,
        capture_output=True,
    )
    subprocess.run(["git", "add", "-A"], cwd=source, check=True, capture_output=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "fixture",
        ],
        cwd=source,
        check=True,
        capture_output=True,
    )
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(source))

    context, _manifest = assemble(root)
    staged_hub = context.output_dir / "docs/reference/plugins/index.md"
    assert staged_hub.is_file()
    assert "source: bob-plugins:docs/index.md" in staged_hub.read_text(encoding="utf-8")
    manifest = json.loads(
        (context.output_dir / "mount-manifest.json").read_text(encoding="utf-8")
    )
    assert (
        manifest["sources"]["reference/plugins/index.md"] == "bob-plugins:docs/index.md"
    )
    assert not [
        item
        for item in check_tree(build_lint_context(root, docs_context=context))
        if item.severity == "error"
    ]


def test_duplicate_nav_entry_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "\n  - Build duplicate: develop/build.md\n")
    joined = "\n".join(_errors(root))
    assert "page 'develop/build.md' appears 2 times in nav" in joined


def test_empty_tab_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/start/index.md", GOOD_FM + "# Start\n")
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Start: start/index.md\n")
    joined = "\n".join(_errors(root))
    assert "tab:" in joined
    assert "no non-hub page" in joined


def test_unresolved_placeholder_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Plugins: reference/plugins/index.md\n")
    joined = "\n".join(_errors(root))
    assert "unresolved mount placeholder" in joined


def test_mount_root_collision_and_nonignored_target_fail(tmp_path: Path) -> None:
    root = _mounted_fixture(_site(tmp_path))
    _write(root / "docs/reference.md", "# Reference contract\n")
    (root / "docs/only.md").unlink()
    _write(
        root / "docs/_meta/contracts.yml",
        "groups:\n  - title: Group\n    entries:\n"
        "      - stem: reference\n        summary: Reference\n        order: 1\n",
    )
    nav = (
        (root / "mkdocs.yml")
        .read_text(encoding="utf-8")
        .replace("Only: only.md", "Reference: reference.md")
    )
    _write(root / "mkdocs.yml", nav)
    assert any("collides with a mount root" in item for item in _errors(root))

    _write(root / ".gitignore", "")
    errors = _errors(root)
    assert any("is not gitignored" in item for item in errors), errors


def test_redirect_source_is_not_a_nav_orphan(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/old.md", GOOD_FM + "# Old\n")
    config = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml",
        config.replace(
            "  - redirects: {}",
            "  - redirects:\n      redirect_maps:\n        old.md: develop/build.md",
        ),
    )
    assert _errors(root) == []


def test_new_privacy_hit_in_code_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        GOOD_FM + "# Build\n\n```bash\ncat /home/bryan/secret\n```\n",
    )
    joined = "\n".join(_errors(root))
    assert "privacy:" in joined
    assert "/home/bryan" in joined


def test_privacy_finding_uses_first_hit_line(tmp_path: Path) -> None:
    root = _site(tmp_path)
    ctx = build_lint_context(root)
    text = (
        GOOD_FM + "# Build\n\nOne.\nSee /home/bryan/first.\nSee /home/bryan/second.\n"
    )
    findings = check_privacy("develop/build.md", text, ctx, contract=False)
    assert len(findings) == 1
    assert findings[0].line == 9


def test_privacy_placeholders_pass_every_pattern() -> None:
    data = load_yaml(ROOT / "docs/_meta/privacy.yml")
    placeholders = data["placeholders"]
    for item in data["patterns"]:
        pattern = re.compile(item["regex"])
        allows = [re.compile(value) for value in item.get("allow", [])]
        for placeholder in placeholders.values():
            match = pattern.search(placeholder)
            assert match is None or any(
                allow.search(match.group(0)) for allow in allows
            ), (
                item["id"],
                placeholder,
            )


def test_nested_list_fences_mask_hygiene_and_check_shell_prompts(
    tmp_path: Path,
) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/guides/doing.md",
        GOOD_FM + "# Doing\n\n- Example:\n\n    ```bash\n    $ just docs-check\n"
        "    bob-cli-a0.2.1 is code here\n    ```\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "\n  - Guides:\n      - Doing: guides/doing.md\n")
    joined = "\n".join(_errors(root))
    assert "$ prompt" in joined
    assert "bead IDs" not in joined


def test_duplicate_baseline_hit_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/only.md", "# Only\n\nContact a@x.com then a@x.com again.\n")
    _write(
        root / "docs/_meta/privacy_baseline.txt",
        "only.md\temail\t1\ta@x.com\n",
    )
    joined = "\n".join(_errors(root))
    assert "duplicate" in joined
    assert "a@x.com" in joined


def test_privacy_fix_only_removes_obsolete_hits(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/only.md", "# Only\n\nContact a@x.com.\n")
    _write(
        root / "docs/_meta/privacy_baseline.txt",
        "only.md\temail\t2\ta@x.com\nonly.md\temail\t1\tb@x.com\n",
    )
    assert run_lint(root, fix=True) == 0
    baseline = (root / "docs/_meta/privacy_baseline.txt").read_text(encoding="utf-8")
    assert "a@x.com" in baseline and "\t1\t" in baseline
    assert "b@x.com" not in baseline
    assert "only.md\temail\t3" not in baseline
    _write(root / "docs/only.md", "# Only\n\nContact a@x.com and c@x.com.\n")
    joined = "\n".join(_errors(root))
    assert "c@x.com" in joined
    assert run_lint(root, fix=True) == 0
    baseline = (root / "docs/_meta/privacy_baseline.txt").read_text(encoding="utf-8")
    assert "c@x.com" not in baseline


def test_unregistered_image_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    image = root / "docs/picture.png"
    image.parent.mkdir(parents=True, exist_ok=True)
    image.write_bytes(b"\x89PNG\r\n\x1a\n")
    joined = "\n".join(_errors(root))
    assert "docs/picture.png:1: media:" in joined
    _write(
        root / "docs/_meta/media.yml",
        "picture.png:\n  source: fixture\n  date: 2026-10-19\n",
    )
    assert _errors(root) == []
    _write(
        root / "docs/_meta/media.yml",
        "missing.png:\n  source: fixture\n  date: 2026-10-19\n",
    )
    joined = "\n".join(_errors(root))
    assert "does not exist as a published asset" in joined


def test_markdown_image_syntax_in_code_is_not_an_asset(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/develop/build.md",
        GOOD_FM + "# Build\n\n```md\n![alt](assets/figure.png)\n```\n",
    )
    assert _errors(root) == []


def test_ratchet_lowers_but_never_raises(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/_meta/budgets.yml", "ceilings:\n  develop/build.md: 100\n")
    _write(root / "docs/develop/build.md", GOOD_FM + "# Build\n\nTiny.\n")
    assert run_lint(root, fix=True) == 0
    text = (root / "docs/_meta/budgets.yml").read_text(encoding="utf-8")
    assert "develop/build.md:" in text
    ctx = build_lint_context(root)
    lowered = ctx.ceilings["develop/build.md"]
    assert lowered < 100
    _write(
        root / "docs/develop/build.md", GOOD_FM + "# Build\n\n" + "word " * 50 + "\n"
    )
    assert run_lint(root, fix=True) == 0
    assert build_lint_context(root).ceilings["develop/build.md"] == lowered


def test_exceeded_ceiling_fails(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/_meta/budgets.yml", "ceilings:\n  develop/build.md: 1\n")
    assert any("ceiling 1" in item for item in _errors(root))


def test_lint_fix_prunes_every_stale_ceiling(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/_templates/t.md", "# Template\n")
    _write(
        root / "docs/_meta/budgets.yml",
        "ceilings:\n"
        "  _templates/t.md: 10\n"
        "  missing.md: 10\n"
        "  only.md: 10\n"
        "  develop/build.md: 100\n",
    )
    ctx = build_lint_context(root)
    pages = {
        path.relative_to(ctx.docs).as_posix()
        for path in ctx.docs.rglob("*.md")
        if path.is_file() and "_templates" not in path.parts
    }
    assert len(check_stale_ceilings(ctx, pages)) == 3
    assert run_lint(root, fix=True) == 0
    fixed = build_lint_context(root)
    fixed_pages = {
        path.relative_to(fixed.docs).as_posix()
        for path in fixed.docs.rglob("*.md")
        if path.is_file() and "_templates" not in path.parts
    }
    assert check_stale_ceilings(fixed, fixed_pages) == []
    budgets = (root / "docs/_meta/budgets.yml").read_text(encoding="utf-8")
    assert "_templates/t.md" not in budgets
    assert "missing.md" not in budgets
    assert "only.md" not in budgets


def test_lint_cli_assembles_before_running(tmp_path: Path, monkeypatch) -> None:
    root = _site(tmp_path)
    context = object()
    called: list[str] = []

    def fake_assemble(actual_root: Path):
        called.append("assemble")
        assert actual_root == root
        return context, root / "target/docs/build.json"

    def fake_lint(actual_root: Path, *, fix: bool = False, docs_context=None) -> int:
        called.append("lint")
        assert actual_root == root
        assert not fix
        assert docs_context is context
        return 7

    monkeypatch.chdir(root)
    monkeypatch.setattr(cli, "assemble", fake_assemble)
    monkeypatch.setattr(cli, "run_lint", fake_lint)
    assert cli.main(["lint"]) == 7
    assert called == ["assemble", "lint"]


def test_word_count_skips_fences_and_generated_blocks() -> None:
    text = (
        GOOD_FM + "hello world\n\n```bash\n" + "word " * 50 + "\n```\n\n"
        "<!-- BEGIN GENERATED: x -->\n" + "word " * 50 + "\n<!-- END GENERATED: x -->\n"
    )
    assert count_words(text) == 2


def test_cli_help_lists_lint_and_fix() -> None:
    parser = _parser()
    names = list(parser._subparsers._group_actions[0].choices)
    assert names == sorted(names)
    assert names == [
        "a11y",
        "assemble",
        "build",
        "check",
        "fonts",
        "gen",
        "host-bootstrap",
        "links",
        "lint",
        "media",
        "publish",
        "publish-plan",
        "publish-record",
        "publish-report",
        "publish-verify",
        "serve",
        "social-card",
        "status",
        "tutorial",
        "urls",
    ]
    lint = parser._subparsers._group_actions[0].choices["lint"]
    flags = [flag for action in lint._actions for flag in action.option_strings]
    assert "-f" in flags and "--fix" in flags
    assert flags.index("-f") < flags.index("--fix")
    assert "enforce the documentation contract" in parser.format_help()
    lint_help = lint.format_help()
    assert "contract registry" in lint_help
    assert "-f, --fix" in lint_help

    publish_plan = parser._subparsers._group_actions[0].choices["publish-plan"]
    publish_flags = [
        flag for action in publish_plan._actions for flag in action.option_strings
    ]
    assert publish_flags == [
        "-h",
        "--help",
        "-d",
        "--drill",
        "-f",
        "--force",
        "-g",
        "--github-output",
        "-m",
        "--manifest",
        "-o",
        "--output-dir",
        "-p",
        "--pinned",
        "-r",
        "--recovery",
        "-w",
        "--work-dir",
    ]

    record = parser._subparsers._group_actions[0].choices["publish-record"]
    record_flags = [
        flag for action in record._actions for flag in action.option_strings
    ]
    assert record_flags == [
        "-h",
        "--help",
        "-b",
        "--built-at",
        "-n",
        "--next",
        "-o",
        "--output",
        "-r",
        "--run-url",
        "-t",
        "--tool",
    ]

    report = parser._subparsers._group_actions[0].choices["publish-report"]
    report_flags = [
        flag for action in report._actions for flag in action.option_strings
    ]
    assert report_flags == [
        "-h",
        "--help",
        "-c",
        "--close",
        "-m",
        "--comment",
        "-d",
        "--detail",
        "-f",
        "--fingerprint",
        "-l",
        "--label",
        "-o",
        "--open",
        "-r",
        "--reason",
        "-u",
        "--run-url",
        "-s",
        "--step",
        "-t",
        "--title",
    ]

    verify = parser._subparsers._group_actions[0].choices["publish-verify"]
    verify_flags = [
        flag for action in verify._actions for flag in action.option_strings
    ]
    assert verify_flags == [
        "-h",
        "--help",
        "-e",
        "--expected",
        "-o",
        "--origin",
        "-r",
        "--retries",
        "-s",
        "--site-urls",
        "-t",
        "--timeout-seconds",
    ]

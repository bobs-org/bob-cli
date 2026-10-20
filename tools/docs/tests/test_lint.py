"""Positive and negative fixtures for every documentation contract family."""

from __future__ import annotations

import subprocess
from pathlib import Path

from bob_docs.cli import _parser
from bob_docs.lint import build_lint_context, check_tree, run_lint
from bob_docs.markdown import count_words

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
    _write(docs / "_meta/privacy.yml", (ROOT / "docs/_meta/privacy.yml").read_text(encoding="utf-8"))
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
    findings = check_tree(build_lint_context(root))
    return [finding.format() for finding in findings if finding.severity == "error"]


def test_clean_fixture_passes(tmp_path: Path) -> None:
    assert _errors(_site(tmp_path)) == []


def test_staged_build_json_is_not_linted(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/build.json", '{"builder": "zensical"}\n')
    assert _errors(root) == []


def test_real_site_passes_lint() -> None:
    assert run_lint(ROOT) == 0


def test_documented_mount_destinations_are_gitignored() -> None:
    for target in (
        "docs/reference/plugins/page.md",
        "docs/reference/mac-capture/page.md",
        "docs/develop/plugins/page.md",
        "docs/develop/mac-capture/page.md",
    ):
        result = subprocess.run(
            ["git", "check-ignore", "-q", "--no-index", target],
            cwd=ROOT,
            check=False,
        )
        assert result.returncode == 0, target
    for landing in (
        "docs/reference/plugins/index.md",
        "docs/reference/mac-capture/index.md",
        "docs/develop/plugins/index.md",
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
    _write(
        root / ".gitignore",
        "/docs/reference/plugins/*\n!/docs/reference/plugins/index.md\n",
    )
    _write(root / "docs/reference/plugins/index.md", GOOD_FM + "# Plugins\n")
    _write(
        root / "docs/reference/plugins/copied.md",
        "---\ntitle: Copied\ndescription: Mounted copy.\n"
        "source: bob-plugins:docs/copied.md\n---\n\n# Copied\n",
    )
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(
        root / "mkdocs.yml",
        nav.replace(
            "      - Only: only.md\n",
            "      - Only: only.md\n      - Plugins: reference/plugins/index.md\n",
        ),
    )
    subprocess.run(["git", "init"], cwd=root, check=True, capture_output=True)
    assert _errors(root) == []


def test_homepage_word_cap(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/index.md", GOOD_FM + "# Home\n\n" + "word " * 410)
    joined = "\n".join(_errors(root))
    assert "400-word cap" in joined


def test_guides_over_budget_fails_and_warns(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "docs/guides/doing.md", GOOD_FM + "# Doing\n\n" + "word " * 1600)
    nav = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", nav + "  - Guides:\n      - Doing: guides/doing.md\n")
    findings = check_tree(build_lint_context(root))
    assert any(item.severity == "warning" and "1500-word" in item.explanation for item in findings)
    _write(root / "docs/guides/doing.md", GOOD_FM + "# Doing\n\n" + "word " * 2600)
    joined = "\n".join(_errors(root))
    assert "over budget" in joined


def test_generated_page_is_budget_exempt_but_needs_metadata(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(
        root / "docs/reference/gen.md",
        "<!-- generated by bob-docs; run just docs-gen -->\n\n# Big\n\n" + "word " * 9000 + "\n",
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
    _write(root / "mkdocs.yml", nav + "  - Reference:\n      - Thing: reference/thing.md\n")
    joined = "\n".join(_errors(root))
    assert "bead IDs" in joined
    assert "src/ paths" not in joined


def test_test_pin_fails_and_exemption_is_exact(tmp_path: Path) -> None:
    root = _site(tmp_path)
    _write(root / "src/lib.rs", 'include_str!("docs/guides/doing.md");\n')
    joined = "\n".join(_errors(root))
    assert "test-pin" in joined
    _write(root / "src/lib.rs", "fn ok() {}\n")
    _write(root / "tests/cli/docs_tutorial.rs", 'include_str!("docs/guides/doing.md");\n')
    assert _errors(root) == []
    _write(root / "tests/cli/docs_tutorial_extra.rs", 'include_str!("docs/guides/doing.md");\n')
    joined = "\n".join(_errors(root))
    assert "docs_tutorial_extra.rs" in joined


def test_hooks_and_unknown_plugin_fail(tmp_path: Path) -> None:
    root = _site(tmp_path)
    config = (root / "mkdocs.yml").read_text(encoding="utf-8")
    _write(root / "mkdocs.yml", config + "hooks:\n  - tools/hook.py\n")
    joined = "\n".join(_errors(root))
    assert "'hooks:' is forbidden" in joined
    _write(root / "mkdocs.yml", config.replace("  - search\n", "  - search\n  - gen-files\n"))
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
        "```text title=\"Output\" .no-copy\n$ not a prompt in text\n```\n",
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
    _write(root / "docs/develop/build.md", GOOD_FM + "# Build\n\n" + "word " * 50 + "\n")
    assert run_lint(root, fix=True) == 0
    assert build_lint_context(root).ceilings["develop/build.md"] == lowered


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
    assert names == ["assemble", "build", "check", "gen", "links", "lint", "serve", "urls"]
    lint = parser._subparsers._group_actions[0].choices["lint"]
    flags = [flag for action in lint._actions for flag in action.option_strings]
    assert "-f" in flags and "--fix" in flags
    assert flags.index("-f") < flags.index("--fix")
    assert "enforce the documentation contract" in parser.format_help()
    lint_help = lint.format_help()
    assert "contract registry" in lint_help
    assert "-f, --fix" in lint_help

"""Offline edit-link gate: rendered rel=edit hrefs target real source files."""

from __future__ import annotations

from pathlib import Path

from bob_docs.context import DocsContext, context_from_root
from bob_docs.links import run_edit_links


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _fixture_root(root: Path, *, plugin_repo: Path) -> DocsContext:
    _write(
        root / "mkdocs.yml",
        "site_name: Test\nsite_url: https://bobs-org.github.io/\n"
        "repo_url: https://github.com/bobs-org/bob-cli\n"
        "repo_name: bobs-org/bob-cli\nedit_uri: edit/master/docs/\n"
        "docs_dir: docs\nsite_dir: target/docs/site\nstrict: true\n",
    )
    _write(root / "docs/page.md", "# Page\n")
    _write(
        root / "docs/_meta/mounts.yml",
        "- repo: bob-plugins\n  remote: https://github.com/bobs-org/bob-plugins\n"
        "  branch: master\n  source: docs\n  target: reference/plugins\n"
        "  nav_title: Plugins\n",
    )
    _write(plugin_repo / "docs/guide.md", "# Guide\n")
    base = context_from_root(root)
    return DocsContext(
        base.repo_root,
        base.docs_root,
        base.authored_config,
        base.output_dir,
        base.config,
        source_dirs={"bob-cli": root, "bob-plugins": plugin_repo},
        assembled=True,
    )


def _site_with(context: DocsContext, pages: dict[str, str]) -> Path:
    site = context.site_dir
    _write(site / "index.html", "<html><body>home</body></html>")
    for name, body in pages.items():
        _write(site / name, body)
    return context.repo_root


def test_edit_links_pass_with_plain_and_escaped_hrefs(tmp_path, capsys) -> None:
    plugin = tmp_path / "bob-plugins"
    context = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    root = _site_with(
        context,
        {
            "page/index.html": (
                '<html><body><a href="https://github.com/bobs-org/bob-cli/edit/master/docs/page.md" '
                'rel="edit">edit</a></body></html>'
            ),
            "reference/plugins/guide/index.html": (
                '<html><body><a href="https:&#x2f;&#x2f;github.com&#x2f;bobs-org&#x2f;'
                "bob-plugins&#x2f;edit&#x2f;master&#x2f;docs&#x2f;guide.md\" "
                'rel="edit">edit</a></body></html>'
            ),
        },
    )
    assert run_edit_links(root, context=context) == 0
    assert "edit-links: OK (2 edit link(s)" in capsys.readouterr().out


def test_edit_links_fail_for_missing_file(tmp_path, capsys) -> None:
    plugin = tmp_path / "bob-plugins"
    context = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    root = _site_with(
        context,
        {
            "page/index.html": (
                '<html><body><a href="https://github.com/bobs-org/bob-cli/edit/master/docs/missing.md" '
                'rel="edit">edit</a></body></html>'
            ),
        },
    )
    assert run_edit_links(root, context=context) == 1
    out = capsys.readouterr().out
    assert "missing" in out and "page/index.html" in out


def test_edit_links_fail_for_wrong_branch(tmp_path, capsys) -> None:
    plugin = tmp_path / "bob-plugins"
    context = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    root = _site_with(
        context,
        {
            "page/index.html": (
                '<html><body><a href="https://github.com/bobs-org/bob-cli/edit/main/docs/page.md" '
                'rel="edit">edit</a></body></html>'
            ),
        },
    )
    assert run_edit_links(root, context=context) == 1
    assert "unknown edit source" in capsys.readouterr().out


def test_edit_links_fail_for_unknown_repo(tmp_path, capsys) -> None:
    plugin = tmp_path / "bob-plugins"
    context = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    root = _site_with(
        context,
        {
            "page/index.html": (
                '<html><body><a href="https://github.com/bobs-org/unknown/edit/master/docs/page.md" '
                'rel="edit">edit</a></body></html>'
            ),
        },
    )
    assert run_edit_links(root, context=context) == 1
    assert "unknown edit source" in capsys.readouterr().out

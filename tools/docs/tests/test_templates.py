"""Template output tests for both builders (contract, normal, reader, mounts)."""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

from bob_docs.assemble import assemble

REPO = Path(__file__).resolve().parents[3]


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=cwd, check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def _ensure_origin(path: Path, origin: str) -> None:
    path.mkdir(parents=True, exist_ok=True)
    if not (path / ".git").exists():
        _git(path, "init", "-b", "main")
    current = subprocess.run(
        ["git", "config", "--get", "remote.origin.url"], cwd=path,
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    if current.returncode != 0:
        _git(path, "remote", "add", "origin", origin)
    elif current.stdout.strip() != origin:
        _git(path, "remote", "set-url", "origin", origin)
    _git(path, "add", "-A")
    status = subprocess.run(
        ["git", "status", "--porcelain"], cwd=path,
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
    )
    if status.stdout.strip():
        _git(path, "-c", "user.email=t@t.t", "-c", "user.name=t", "commit", "-qm", "x")


def _fixture_root(root: Path, *, plugin_repo: Path, site_url: str = "https://bobs-org.github.io/") -> Path:
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/develop").mkdir(parents=True)
    # Copy the real overrides and stylesheet so the test tracks the shipped templates.
    for name in ("main.html", "partials/bob-meta.html", "partials/bob-contract.html",
                 "partials/bob-provenance.html", "partials/actions.html",
                 "partials/tags.html"):
        src = REPO / "docs/overrides" / name
        _write(root / "docs/overrides" / name, src.read_text(encoding="utf-8"))
    _write(root / "docs/stylesheets/bob.css", (REPO / "docs/stylesheets/bob.css").read_text(encoding="utf-8"))
    for name in ("bob-mark.svg", "favicon.svg"):
        src = REPO / "docs/assets" / name
        if src.is_file():
            _write(root / "docs/assets" / name, src.read_text(encoding="utf-8"))
    _write(root / "docs/index.md",
           "---\ntitle: Home\ndescription: Fixture homepage.\n---\n\n# Home\n\nBody.\n")
    _write(root / "docs/normal.md",
           "---\ntitle: Normal\ndescription: A normal page.\n---\n\n# Normal\n\nBody.\n")
    _write(root / "docs/alpha.md", "# Alpha contract\n")
    _write(root / "docs/beta.md", "# Beta contract\n")
    _write(root / "docs/gamma.md", "# Gamma contract\n")
    _write(root / "docs/develop/index.md", "# Dev\n")
    _write(
        root / "docs/_meta/contracts.yml",
        "groups:\n  - title: Group\n    entries:\n"
        "      - stem: alpha\n        summary: Alpha summary\n        order: 1\n"
        "        reader: guides/alpha.md\n"
        "      - stem: beta\n        summary: Beta summary\n        order: 2\n"
        "      - stem: gamma\n        summary: Gamma summary\n        order: 3\n"
        "        historical: true\n",
    )
    _write(root / "docs/guides/alpha.md",
           "---\ntitle: Alpha guide\ndescription: Reader guide.\n---\n\n# Alpha\n")
    _write(
        root / "docs/guides/nested/tagged.md",
        "---\ntitle: Nested tagged\ndescription: Tagged nested page.\n"
        "tags:\n  - CLI\n  - Obsidian\n---\n\n# Nested tagged\n",
    )
    _write(
        root / "docs/reference/by-component.md",
        "---\ntitle: By component\ndescription: Component index.\n---\n\n"
        "# By component\n\n## CLI {#cli}\n\n- [Nested tagged](../guides/nested/tagged.md)\n\n"
        "## Obsidian {#obsidian}\n\n- [Nested tagged](../guides/nested/tagged.md)\n\n"
        "## macOS {#macos}\n\nNo tagged pages yet.\n",
    )
    _write(
        root / "docs/_meta/mounts.yml",
        f"- repo: bob-plugins\n  remote: {plugin_repo.as_uri()}\n  branch: main\n"
        "  source: docs\n  target: reference/plugins\n  nav_title: Plugins\n",
    )
    _write(root / "docs/reference/plugins/index.md", "# Plugins\n")
    _write(
        root / ".gitignore",
        "target/\ndocs/reference/plugins/*\n!docs/reference/plugins/index.md\n",
    )
    _write(
        root / "mkdocs.yml",
        f"site_name: Fixture\nsite_url: {site_url}\n"
        "repo_url: https://github.com/bobs-org/bob-cli\n"
        "repo_name: bobs-org/bob-cli\nedit_uri: edit/master/docs/\n"
        "docs_dir: docs\nsite_dir: target/docs/site\nstrict: true\n"
        "use_directory_urls: true\n"
        "theme:\n  name: material\n  custom_dir: docs/overrides\n"
        "  variant: classic\n  font: false\n"
        "  logo: assets/bob-mark.svg\n  favicon: assets/favicon.svg\n"
        "  palette:\n    - media: \"(prefers-color-scheme: light)\"\n"
        "      scheme: default\n      primary: teal\n      accent: teal\n"
        "    - media: \"(prefers-color-scheme: dark)\"\n"
        "      scheme: slate\n      primary: teal\n      accent: teal\n"
        "markdown_extensions:\n  - pymdownx.superfences\n"
        "  - github-callouts\n"
        "extra_css:\n  - stylesheets/bob.css\n"
        "plugins:\n  - search\n  - tags:\n      listings: false\n      export: false\n"
        "nav:\n  - Home: index.md\n  - Normal: normal.md\n"
        "  - Alpha: alpha.md\n  - Beta: beta.md\n  - Gamma: gamma.md\n"
        "  - Guide: guides/alpha.md\n"
        "  - Nested: guides/nested/tagged.md\n"
        "  - Components: reference/by-component.md\n"
        "  - Plugins: reference/plugins/index.md\n",
    )
    return root


def _plugin_repo(repo: Path) -> Path:
    _write(repo / "docs/guide.md", "# Guide\n\nBody.\n")
    _write(repo / "docs/nav.yml", "- title: Guide\n  path: guide.md\n")
    _ensure_origin(repo, repo.as_uri())
    return repo


def _build(root: Path, *, material: bool, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(root.parent / "bob-plugins"))
    context, _ = assemble(root, builder="material" if material else "zensical")
    executable = "mkdocs" if material else "zensical"
    args = ["build", "--strict", "--config-file", str(context.assembled_config)]
    if material:
        args.append("--config-file")
    # mkdocs uses `mkdocs build --strict --config-file ...`; zensical too.
    cmd = [executable, "build", "--strict", "--config-file", str(context.assembled_config)]
    result = subprocess.run(cmd, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    return context.site_dir


def _read(site: Path, *parts: str) -> str:
    return (site / Path(*parts)).read_text(encoding="utf-8")


def test_templates_in_zensical(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    plugin = _plugin_repo(tmp_path / "bob-plugins")
    root = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    site = _build(root, material=False, monkeypatch=monkeypatch)
    alpha = _read(site, "alpha/index.html")
    assert "Contract · written for implementers and tests" in alpha
    assert "Alpha summary" in alpha
    assert "guides/alpha" in alpha
    beta = _read(site, "beta/index.html")
    assert "Contract · written for implementers and tests" in beta
    assert "Reader guide" not in beta
    gamma = _read(site, "gamma/index.html")
    assert "historical" in gamma.lower()
    home = _read(site, "index.html")
    assert "Contract · written for implementers and tests" not in home
    normal = _read(site, "normal/index.html")
    assert "Contract · written for implementers and tests" not in normal
    # Provenance has no local filesystem paths and links the manifest.
    assert "build.json" in home
    assert "/tmp" not in home and str(tmp_path) not in home
    # Open Graph uses the configured site URL, not a hardcoded host.
    # (MiniJinja escapes slashes as &#x2f;; Jinja2 does not. Assert parts.)
    assert "bobs-org.github.io" in home and "social-card.png" in home
    # Mounted edit links use the mount table branch (slashes escape as
    # &#x2f; in MiniJinja; assert the parts).
    guide = _read(site, "reference/plugins/guide/index.html")
    assert "edit" in guide and "guide.md" in guide and "/main/" in guide.replace("&#x2f;", "/")


@pytest.mark.skipif(shutil.which("mkdocs") is None, reason="material builder not installed")
def test_templates_in_material(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    plugin = _plugin_repo(tmp_path / "bob-plugins")
    root = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    site = _build(root, material=True, monkeypatch=monkeypatch)
    alpha = _read(site, "alpha/index.html")
    assert "Contract · written for implementers and tests" in alpha
    assert "guides/alpha" in alpha
    home = _read(site, "index.html")
    assert "Contract · written for implementers and tests" not in home
    guide = _read(site, "reference/plugins/guide/index.html")
    assert "edit" in guide and "guide.md" in guide and "/main/" in guide.replace("&#x2f;", "/")
    assert "build.json" in home


def _unescape(html: str) -> str:
    return (
        html.replace("&#x2f;", "/")
        .replace("&#x23;", "#")
        .replace("&amp;", "&")
        .replace("%23", "#")
    )


def _assert_component_chips(site: Path) -> None:
    tagged = _unescape(_read(site, "guides/nested/tagged/index.html"))
    chips = tagged.count('class="bob-component-chip"')
    assert chips == 2, tagged
    assert tagged.count('aria-label="Components"') == 1
    assert ">CLI<" in tagged
    assert ">Obsidian<" in tagged
    assert "by-component" in tagged
    assert "#cli" in tagged
    assert "#obsidian" in tagged
    assert ".md#cli" not in tagged
    assert ".md#obsidian" not in tagged
    untagged = _read(site, "normal/index.html")
    assert "bob-component-chip" not in untagged
    assert 'aria-label="Components"' not in untagged


def test_component_chips_in_zensical(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    plugin = _plugin_repo(tmp_path / "bob-plugins")
    root = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    site = _build(root, material=False, monkeypatch=monkeypatch)
    _assert_component_chips(site)


@pytest.mark.skipif(shutil.which("mkdocs") is None, reason="material builder not installed")
def test_component_chips_in_material(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    plugin = _plugin_repo(tmp_path / "bob-plugins")
    root = _fixture_root(tmp_path / "root", plugin_repo=plugin)
    site = _build(root, material=True, monkeypatch=monkeypatch)
    _assert_component_chips(site)


def test_component_chips_honor_url_prefix(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    plugin = _plugin_repo(tmp_path / "bob-plugins")
    root = _fixture_root(
        tmp_path / "root",
        plugin_repo=plugin,
        site_url="https://example.test/docs/",
    )
    site = _build(root, material=False, monkeypatch=monkeypatch)
    tagged = _unescape(_read(site, "guides/nested/tagged/index.html"))
    assert tagged.count('class="bob-component-chip"') == 2
    assert "#cli" in tagged
    assert "by-component" in tagged
    assert "/docs/" in tagged or "../" in tagged

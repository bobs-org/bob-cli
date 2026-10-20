"""Compatibility page and override tests for the Learn reader path."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest
import yaml
from jinja2 import Environment, FileSystemLoader

from bob_docs.assemble import assemble

REPO = Path(__file__).resolve().parents[3]
PARTIAL = REPO / "docs/overrides/partials/bob-compatibility.html"


def _render(extra_build: dict, *, repo_url: str = "https://github.com/bobs-org/bob-cli") -> str:
    env = Environment(loader=FileSystemLoader(REPO / "docs/overrides"), autoescape=True)
    # The `url` filter exists in the MkDocs/Zensical template environment;
    # stand in with identity here so unit rendering matches builder semantics.
    env.filters.setdefault("url", lambda value: value)
    template = env.get_template("partials/bob-compatibility.html")
    # Mirror the template's config access shape.
    config = {"extra": {"build": extra_build, "bob_mounts": []}, "repo_url": repo_url}
    return template.render(config=config)


def test_assemble_exposes_build_and_compatibility_flag() -> None:
    context, _manifest = assemble(REPO)
    staged = yaml.load(context.assembled_config.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
    assert isinstance(staged, dict)
    extra = staged.get("extra")
    assert isinstance(extra, dict)
    assert str(extra.get("has_compatibility")).lower() == "true"
    build = extra.get("build")
    assert isinstance(build, dict)
    for key in ("sources", "tools", "built_at", "builder"):
        assert key in build, f"extra.build is missing {key}"
    assert "bob-cli" in build["sources"]


def test_compatibility_renders_complete_provenance() -> None:
    build = {
        "sources": {
            "bob-cli": {"path": ".", "sha": "abcdef1234567890", "dirty": True, "branch": "main"},
            "bob-plugins": {"path": "link", "sha": "1234567890abcdef", "dirty": False, "branch": "main"},
        },
        "tools": {"bob-docs": "0.1.0", "python": "3.14.0", "zensical": "0.0.71"},
        "built_at": "2026-10-20T03:00:00Z",
        "builder": "zensical",
    }
    html = _render(build)
    assert "abcdef1" in html
    assert "(dirty)" in html
    assert "build.json" in html
    assert "bob-docs 0.1.0" in html
    assert "Zensical 0.0.71" in html
    assert "unknown revision" not in html


def test_compatibility_handles_partial_provenance_truthfully() -> None:
    build = {
        "sources": {
            "bob-cli": {"path": ".", "sha": "", "dirty": True, "branch": None},
            "bob-plugins": {"path": "link", "sha": None, "dirty": False, "branch": None},
        },
        "tools": {"bob-docs": "0.1.0", "python": "3.14.0"},
        "built_at": "2026-10-20T03:00:00Z",
        "builder": "zensical",
    }
    html = _render(build)
    assert "unknown revision" in html
    assert "(dirty)" in html
    assert "build.json" in html
    # Optional tools absent from the manifest must not render.
    assert "Zensical" not in html
    assert "MkDocs" not in html
    # No local filesystem paths leak into the snapshot.
    assert "/tmp" not in html and str(REPO) not in html


def test_compatibility_omits_private_paths_and_escapes_untrusted_text() -> None:
    private_path = "/Users/bryan/secret-checkout/bob-cli"
    build = {
        "sources": {
            "bob-cli": {
                "path": private_path,
                "sha": "abcdef1234567890",
                "dirty": True,
                "branch": '<script>alert("x")</script> & main',
            },
            "bob-plugins": {
                "path": "/private/tmp/bob-plugins",
                "sha": "1234567890abcdef",
                "dirty": False,
                "branch": "main",
            },
        },
        "tools": {"bob-docs": "0.1.0", "python": "3.14.0", "zensical": "0.0.71"},
        "built_at": "2026-10-20T03:00:00Z",
        "builder": "zensical",
    }
    html = _render(build)
    assert private_path not in html
    assert "/private/tmp/bob-plugins" not in html
    assert "<script>" not in html
    assert "&lt;script&gt;" in html
    assert "abcdef1" in html
    assert "(dirty)" in html
    assert "build.json" in html


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _fixture_root(root: Path) -> Path:
    (root / "docs/_meta").mkdir(parents=True)
    for name in (
        "main.html",
        "compatibility.html",
        "partials/bob-meta.html",
        "partials/bob-contract.html",
        "partials/bob-provenance.html",
        "partials/bob-compatibility.html",
        "partials/actions.html",
    ):
        src = REPO / "docs/overrides" / name
        _write(root / "docs/overrides" / name, src.read_text(encoding="utf-8"))
    _write(root / "docs/stylesheets/bob.css", (REPO / "docs/stylesheets/bob.css").read_text(encoding="utf-8"))
    _write(
        root / "docs/reference/compatibility.md",
        (REPO / "docs/reference/compatibility.md").read_text(encoding="utf-8"),
    )
    _write(
        root / "docs/index.md",
        "---\ntitle: Home\ndescription: Fixture homepage.\n---\n\n# Home\n\nBody.\n",
    )
    _write(
        root / "docs/_meta/contracts.yml",
        "groups:\n  - title: Group\n    entries:\n      - stem: alpha\n        summary: Alpha\n        order: 1\n",
    )
    _write(root / "docs/_meta/mounts.yml", "[]\n")
    _write(root / "docs/reference/index.md", "# Plugins\n")
    _write(root / "docs/reference/plugins/index.md", "# Obsidian plugins inventory\n")
    _write(root / "docs/reference/configuration.md", "# Configuration\n\nStub.\n")
    _write(root / "docs/develop/migration-notes.md", "# Migration notes\n\nStub.\n")
    _write(root / "docs/alpha.md", "# Alpha contract\n")
    for stub in ("start/index.md", "start/install.md", "start/obsidian.md", "start/mac-capture.md"):
        _write(root / "docs" / stub, f"# {stub}\n\nStub.\n")
    _write(root / ".gitignore", "target/\n")
    _write(
        root / "mkdocs.yml",
        "site_name: Fixture\nsite_url: https://bobs-org.github.io/\n"
        "repo_url: https://github.com/bobs-org/bob-cli\n"
        "repo_name: bobs-org/bob-cli\nedit_uri: edit/master/docs/\n"
        "docs_dir: docs\nsite_dir: target/docs/site\nstrict: true\n"
        "use_directory_urls: true\n"
        "theme:\n  name: material\n  custom_dir: docs/overrides\n"
        "  font: false\n"
        "markdown_extensions:\n  - pymdownx.superfences\n"
        "extra_css:\n  - stylesheets/bob.css\n"
        "plugins:\n  - search\n"
        "nav:\n  - Home: index.md\n  - Reference:\n      - Overview: reference/index.md\n"
        "      - Requirements and compatibility: reference/compatibility.md\n"
        "      - Obsidian plugins: reference/plugins/index.md\n",
    )
    return root


def test_compatibility_builds_in_zensical(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    pytest.importorskip("zensical")
    root = _fixture_root(tmp_path / "compat")
    subprocess.run(["git", "init", "-b", "main"], cwd=root, check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    subprocess.run(["git", "-c", "user.email=t@t.t", "-c", "user.name=t", "commit", "-qm", "x"],
                   cwd=root, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    monkeypatch.chdir(root)
    context, _ = assemble(root)
    result = subprocess.run(
        ["zensical", "build", "--strict", "--config-file", str(context.assembled_config)],
        cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    html = (context.site_dir / "reference/compatibility/index.html").read_text(encoding="utf-8")
    assert "Tested snapshot" in html
    assert "Minimum requirements" in html
    assert "build.json" in html


def test_compatibility_builds_in_material(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    pytest.importorskip("mkdocs")
    pytest.importorskip("material")
    root = _fixture_root(tmp_path / "compat-material")
    subprocess.run(["git", "init", "-b", "main"], cwd=root, check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    subprocess.run(["git", "-c", "user.email=t@t.t", "-c", "user.name=t", "commit", "-qm", "x"],
                   cwd=root, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    monkeypatch.chdir(root)
    context, _ = assemble(root)
    result = subprocess.run(
        [sys.executable, "-m", "mkdocs", "build", "--strict", "--config-file", str(context.assembled_config)],
        cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    html = (context.site_dir / "reference/compatibility/index.html").read_text(encoding="utf-8")
    assert "Tested snapshot" in html
    assert "Minimum requirements" in html
    assert "build.json" in html

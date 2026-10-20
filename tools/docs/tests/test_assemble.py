import json
from pathlib import Path

import pytest

from bob_docs.assemble import assemble
from bob_docs.errors import DocsError
from bob_docs.yaml_tools import InertPythonName, load_yaml


def _minimal_root(root: Path, *, mounts: str = "[]\n") -> Path:
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/develop").mkdir(parents=True)
    (root / "docs/overrides").mkdir(parents=True)
    (root / "docs/index.md").write_text("# Home\n", encoding="utf-8")
    (root / "docs/develop/index.md").write_text("# Develop\n", encoding="utf-8")
    (root / "docs/only.md").write_text("# Only\n", encoding="utf-8")
    (root / "docs/_meta/contracts.yml").write_text(
        "groups:\n  - title: Group\n    entries:\n      - stem: only\n        summary: One contract\n        order: 1\n",
        encoding="utf-8",
    )
    (root / "docs/_meta/mounts.yml").write_text(mounts, encoding="utf-8")
    (root / "mkdocs.yml").write_text(
        """\
site_name: Test
docs_dir: docs
site_dir: target/docs/site
theme:
  name: material
  custom_dir: docs/overrides
markdown_extensions:
  - pymdownx.superfences
  - custom:
      format: !!python/name:bob_docs.fences.fence_format ''
nav:
  - Home: index.md
""",
        encoding="utf-8",
    )
    return root


def test_assemble_rebases_paths_and_injects_provenance(tmp_path: Path) -> None:
    root = _minimal_root(tmp_path)
    context, manifest_path = assemble(root)
    config = load_yaml(context.assembled_config)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    assert config["docs_dir"] == "docs"
    assert (context.assembled_config.parent / config["docs_dir"]).resolve() == (root / "target/docs/docs").resolve()
    assert context.site_dir == (root / "target/docs/site").resolve()
    assert config["theme"]["custom_dir"] == "docs/overrides"
    assert (root / "target/docs/docs/only.md").is_file()
    assert config["markdown_extensions"][1]["custom"]["format"] == InertPythonName(
        "tag:yaml.org,2002:python/name:bob_docs.fences.fence_format", ""
    )
    assert config["extra"]["contracts"]["total"] == 1
    assert manifest["sources"]["bob-cli"]["path"] == "."
    assert manifest["built_at"].endswith("Z")


def test_assemble_rejects_invalid_mount_table(tmp_path: Path) -> None:
    root = _minimal_root(tmp_path, mounts="- repo: bob-plugins\n")
    with pytest.raises(DocsError, match="missing keys"):
        assemble(root)


def test_assemble_publishes_staged_build_json(tmp_path: Path) -> None:
    root = _minimal_root(tmp_path)
    context, manifest_path = assemble(root)
    staged = (context.output_dir / "docs" / "build.json").read_text(encoding="utf-8")
    assert staged == manifest_path.read_text(encoding="utf-8")
    manifest = json.loads(staged)
    assert manifest["builder"] == "zensical"
    assert "zensical" in manifest["tools"]


def test_build_assembled_publishes_build_json_into_site(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from types import SimpleNamespace

    from bob_docs import build as build_module

    root = _minimal_root(tmp_path)
    context, manifest_path = assemble(root)
    context.site_dir.mkdir(parents=True)
    monkeypatch.setattr(
        build_module, "subprocess", SimpleNamespace(run=lambda *args, **kwargs: SimpleNamespace(returncode=0)),
    )
    assert build_module.build_assembled(context, manifest_path) == 0
    site_manifest = (context.site_dir / "build.json").read_text(encoding="utf-8")
    assert site_manifest == manifest_path.read_text(encoding="utf-8")

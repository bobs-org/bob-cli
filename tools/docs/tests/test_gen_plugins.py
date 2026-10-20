"""Plugin inventory generation uses the canonical source, never mount copies."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from bob_docs import gen_contracts
from bob_docs.errors import DocsError
from bob_docs.gen_plugins import run_plugin_inventory

STUB_GENERATOR = """\
#!/usr/bin/env node
import fs from "node:fs";
const check = process.argv.includes("--check");
const marker = "docs/generated.txt";
const expected = "fresh\\n";
if (check) {
  const actual = fs.existsSync(marker) ? fs.readFileSync(marker, "utf8") : "";
  if (actual !== expected) {
    process.stderr.write("stale: " + marker + "\\n");
    process.exit(1);
  }
  process.stdout.write("up to date\\n");
  process.exit(0);
}
fs.mkdirSync("docs", { recursive: true });
fs.writeFileSync(marker, expected);
process.stdout.write("generated\\n");
"""


def _root_with_plugin_mount(root: Path) -> Path:
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/_meta/mounts.yml").write_text(
        "- repo: bob-plugins\n"
        "  remote: https://github.com/bobs-org/bob-plugins\n"
        "  branch: master\n"
        "  source: docs\n"
        "  target: reference/plugins\n"
        "  nav_title: Obsidian plugins\n",
        encoding="utf-8",
    )
    return root


def _stub_plugin(repo: Path, *, contents: str | None = "fresh\n") -> Path:
    script = repo / "scripts/docs-inventory.mjs"
    script.parent.mkdir(parents=True, exist_ok=True)
    script.write_text(STUB_GENERATOR, encoding="utf-8")
    if contents is not None:
        (repo / "docs").mkdir(parents=True, exist_ok=True)
        (repo / "docs/generated.txt").write_text(contents, encoding="utf-8")
    return repo


def test_plugin_generation_is_skipped_without_a_plugin_mount(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "docs/_meta").mkdir(parents=True)
    (tmp_path / "docs/_meta/mounts.yml").write_text("[]\n", encoding="utf-8")
    monkeypatch.setattr(
        "bob_docs.gen_plugins.subprocess.run",
        lambda *args, **kwargs: pytest.fail("must not run a generator"),
    )
    assert run_plugin_inventory(tmp_path, check=True) == 0


def test_generation_and_check_run_canonical_plugin_generator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[list[str], Path]] = []

    def fake_run(command, cwd, check):
        calls.append((list(command), Path(cwd)))
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr("bob_docs.gen_plugins.subprocess.run", fake_run)
    monkeypatch.setattr(
        "bob_docs.gen_plugins.shutil.which", lambda name: "/usr/bin/node"
    )

    plugin = tmp_path / "bob-plugins"
    (plugin / "scripts").mkdir(parents=True)
    generator = plugin / "scripts/docs-inventory.mjs"
    generator.write_text("// stub\n", encoding="utf-8")
    mount_copy = tmp_path / "bob-cli/docs/reference/plugins/scripts/docs-inventory.mjs"
    mount_copy.parent.mkdir(parents=True)
    mount_copy.write_text("// mount copy must not run\n", encoding="utf-8")

    root = _root_with_plugin_mount(tmp_path / "bob-cli")
    source_dirs = {"bob-cli": root, "bob-plugins": plugin}

    assert run_plugin_inventory(root, check=False, source_dirs=source_dirs) == 0
    assert run_plugin_inventory(root, check=True, source_dirs=source_dirs) == 0
    assert calls[0] == (["/usr/bin/node", str(generator)], plugin)
    assert calls[1] == (["/usr/bin/node", str(generator), "--check"], plugin)


def test_stale_plugin_output_fails_check_without_writing(tmp_path: Path) -> None:
    plugin = _stub_plugin(tmp_path / "bob-plugins", contents="stale\n")
    root = _root_with_plugin_mount(tmp_path / "bob-cli")
    source_dirs = {"bob-cli": root, "bob-plugins": plugin}

    assert run_plugin_inventory(root, check=True, source_dirs=source_dirs) == 1
    assert (plugin / "docs/generated.txt").read_text(encoding="utf-8") == "stale\n"


def test_check_mode_does_not_write_fresh_plugin_output(tmp_path: Path) -> None:
    plugin = _stub_plugin(tmp_path / "bob-plugins", contents="fresh\n")
    marker = plugin / "docs/generated.txt"
    before = marker.stat().st_mtime_ns
    root = _root_with_plugin_mount(tmp_path / "bob-cli")

    assert (
        run_plugin_inventory(
            root, check=True, source_dirs={"bob-cli": root, "bob-plugins": plugin}
        )
        == 0
    )
    assert marker.read_text(encoding="utf-8") == "fresh\n"
    assert marker.stat().st_mtime_ns == before


def test_generation_writes_canonical_plugin_files_not_mount_copies(
    tmp_path: Path,
) -> None:
    plugin = _stub_plugin(tmp_path / "bob-plugins", contents=None)
    mount_copy = tmp_path / "bob-cli/docs/reference/plugins/generated.txt"
    mount_copy.parent.mkdir(parents=True)
    mount_copy.write_text("copy\n", encoding="utf-8")
    root = _root_with_plugin_mount(tmp_path / "bob-cli")

    assert (
        run_plugin_inventory(
            root, check=False, source_dirs={"bob-cli": root, "bob-plugins": plugin}
        )
        == 0
    )
    assert (plugin / "docs/generated.txt").read_text(encoding="utf-8") == "fresh\n"
    assert mount_copy.read_text(encoding="utf-8") == "copy\n"


def test_production_check_cannot_fall_back_to_another_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _root_with_plugin_mount(tmp_path / "bob-cli")
    other = _stub_plugin(tmp_path / "other-plugins")
    monkeypatch.setenv("BOB_DOCS_BOB_PLUGINS_DIR", str(other))
    monkeypatch.setattr(
        "bob_docs.gen_plugins.discover_repo",
        lambda *args, **kwargs: pytest.fail("production must not rediscover"),
    )
    with pytest.raises(DocsError, match="cannot rediscover bob-plugins"):
        run_plugin_inventory(
            root,
            check=True,
            source_dirs={"bob-cli": root},
            production=True,
        )


def test_assembled_check_does_not_rediscover_a_missing_plugin_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _root_with_plugin_mount(tmp_path / "bob-cli")
    monkeypatch.setattr(
        "bob_docs.gen_plugins.discover_repo",
        lambda *args, **kwargs: pytest.fail("assembled check must not rediscover"),
    )
    with pytest.raises(DocsError, match="assembled source context"):
        run_plugin_inventory(
            root,
            check=True,
            source_dirs={"bob-cli": root},
            production=False,
        )


def test_missing_node_fails_clearly(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plugin = _stub_plugin(tmp_path / "bob-plugins")
    root = _root_with_plugin_mount(tmp_path / "bob-cli")
    monkeypatch.setattr("bob_docs.gen_plugins.shutil.which", lambda name: None)
    with pytest.raises(DocsError, match="requires Node"):
        run_plugin_inventory(
            root, check=True, source_dirs={"bob-cli": root, "bob-plugins": plugin}
        )


def test_missing_generator_fails_clearly(tmp_path: Path) -> None:
    plugin = tmp_path / "bob-plugins"
    plugin.mkdir()
    root = _root_with_plugin_mount(tmp_path / "bob-cli")
    with pytest.raises(DocsError, match="plugin inventory generator is missing"):
        run_plugin_inventory(
            root, check=True, source_dirs={"bob-cli": root, "bob-plugins": plugin}
        )


def test_check_or_generate_dispatches_plugin_inventory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: dict[str, object] = {}
    monkeypatch.setattr(gen_contracts, "generate_contracts", lambda root, *, check: [])
    monkeypatch.setattr(gen_contracts, "generate_reference", lambda root, *, check: [])

    def fake_plugin(root, *, check, source_dirs=None, production=False):
        seen.update(
            {
                "root": root,
                "check": check,
                "source_dirs": source_dirs,
                "production": production,
            }
        )
        return 0

    monkeypatch.setattr(gen_contracts, "run_plugin_inventory", fake_plugin)
    source_dirs = {"bob-cli": tmp_path, "bob-plugins": tmp_path / "plugins"}
    assert (
        gen_contracts.check_or_generate(
            tmp_path, check=True, source_dirs=source_dirs, production=True
        )
        == 0
    )
    assert seen == {
        "root": tmp_path,
        "check": True,
        "source_dirs": source_dirs,
        "production": True,
    }


def test_check_or_generate_returns_plugin_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(gen_contracts, "generate_contracts", lambda root, *, check: [])
    monkeypatch.setattr(gen_contracts, "generate_reference", lambda root, *, check: [])
    monkeypatch.setattr(
        gen_contracts,
        "run_plugin_inventory",
        lambda *args, **kwargs: 1,
    )
    assert gen_contracts.check_or_generate(tmp_path, check=True) == 1

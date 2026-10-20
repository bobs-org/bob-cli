from __future__ import annotations

import os
import shutil
import tempfile
from pathlib import Path

import pytest

from bob_docs.errors import DocsError
from bob_docs.sandbox import DEFAULT_NOW, BobSandbox, isolated_environment, resolve_bob_binary
from bob_docs.tutorial import (
    TutorialFence,
    _nav_pages,
    _run_shell,
    _runner_script,
    parse_tutorial_fences,
    run_tutorial,
)


ROOT = Path(__file__).resolve().parents[3]


def test_parser_extracts_bash_and_consumes_adjacent_markers() -> None:
    source = """<!-- tutorial-test: skip network access -->
```bash
curl https://example.invalid
```
```bash
echo state
```
<!-- tutorial-test: checkpoint fixture-copy -->
"""
    fences = parse_tutorial_fences(source, source="docs/start/example.md")
    assert [(f.skip, f.checkpoint) for f in fences] == [
        ("network access", None),
        (None, "fixture-copy"),
    ]


def test_parser_ignores_fences_inside_comments_and_nested_examples() -> None:
    source = """<!--
```bash
false
```
-->
````markdown
```bash
false
```
````
```bash
echo actual
```
"""
    fences = parse_tutorial_fences(source, source="docs/start/example.md")
    assert len(fences) == 1
    assert fences[0].body == "echo actual\n"


@pytest.mark.parametrize(
    "source, message",
    [
        ("<!-- tutorial-test: skip -->\n```bash\ntrue\n```\n", "malformed"),
        (
            "```bash\ntrue\n```\n<!-- tutorial-test: checkpoint made-up -->\n",
            "unknown tutorial checkpoint",
        ),
        (
            "<!-- tutorial-test: checkpoint fixture-copy -->\n```bash\ntrue\n```\n",
            "checkpoint marker must follow",
        ),
        (
            "<!-- tutorial-test: skip network -->\n```text\nexample\n```\n",
            "unconsumed",
        ),
    ],
)
def test_parser_rejects_bad_or_unconsumed_markers(source: str, message: str) -> None:
    with pytest.raises(DocsError, match=message):
        parse_tutorial_fences(source, source="docs/start/example.md")


def test_skipped_fence_keeps_its_source_and_reason_in_runner_report(tmp_path: Path) -> None:
    root = tmp_path / "sandbox"
    root.mkdir()
    workdir = root / "work"
    workdir.mkdir()
    binary = Path("/bin/true")
    env = isolated_environment(root=root, bob_bin=binary, now=DEFAULT_NOW, source_env={"PATH": "/bin"})
    sandbox = BobSandbox(root, workdir, Path(env["HOME"]), Path(env["BOB_DIR"]), workdir, env, binary, DEFAULT_NOW)
    fences = parse_tutorial_fences(
        "<!-- tutorial-test: skip requires a GUI -->\n```bash\nopen -a Obsidian\n```\n",
        source="docs/start/gui.md",
    )
    script, skips = _runner_script(fences, sandbox=sandbox, source_root=ROOT)
    assert "open -a Obsidian" not in script
    assert skips == ["skipped: docs/start/gui.md:2: requires a GUI"]


def test_runner_rejects_start_pages_missing_from_navigation(tmp_path: Path) -> None:
    root = tmp_path
    (root / "docs/start").mkdir(parents=True)
    (root / "docs/start/index.md").write_text("---\ntitle: Start\ndescription: Start\n---\n", encoding="utf-8")
    (root / "docs/start/orphan.md").write_text("---\ntitle: Orphan\ndescription: Orphan\n---\n", encoding="utf-8")
    (root / "mkdocs.yml").write_text(
        "nav:\n  - Get started:\n      - Overview: start/index.md\n", encoding="utf-8"
    )
    with pytest.raises(DocsError, match="missing from Get started nav: start/orphan.md"):
        run_tutorial(root)


def test_navigation_reader_accepts_mkdocs_python_name_tags(tmp_path: Path) -> None:
    page = tmp_path / "docs/start/index.md"
    page.parent.mkdir(parents=True)
    page.write_text("# Start\n", encoding="utf-8")
    (tmp_path / "mkdocs.yml").write_text(
        """\
markdown_extensions:
  - pymdownx.superfences:
      custom_fences:
        - name: mermaid
          format: !!python/name:pymdownx.superfences.fence_code_format ''
nav:
  - Get started:
      - Overview: start/index.md
""",
        encoding="utf-8",
    )

    assert _nav_pages(tmp_path) == [page]


def test_runner_requires_all_named_checkpoints_before_building_bob(tmp_path: Path) -> None:
    root = tmp_path
    (root / "docs/start").mkdir(parents=True)
    (root / "docs/start/index.md").write_text(
        "---\ntitle: Start\ndescription: Start\n---\n```bash\necho hello\n```\n",
        encoding="utf-8",
    )
    (root / "mkdocs.yml").write_text(
        "nav:\n  - Get started:\n      - Overview: start/index.md\n", encoding="utf-8"
    )
    with pytest.raises(DocsError, match="missing required tutorial checkpoints"):
        run_tutorial(root, source_env={"PATH": "/bin"})


def test_sandbox_environment_discards_hostile_bob_and_desktop_overrides(tmp_path: Path) -> None:
    source = {
        "PATH": "/usr/bin:/bin",
        "HOME": "/private/home",
        "BOB_DIR": "/private/vault",
        "BOB_CONFIG_FILE": "/private/config.yml",
        "BOB_DAY_FILE": "/private/day.md",
        "BOB_NOW": "1999-01-01",
        "BOB_REF_JOBS_KICK": "on",
        "DISPLAY": ":1",
        "WAYLAND_DISPLAY": "wayland-9",
        "DBUS_SESSION_BUS_ADDRESS": "private-bus",
        "CLIPBOARD": "private-selection",
    }
    env = isolated_environment(
        root=tmp_path / "sandbox",
        bob_bin=Path("/isolated/bin/bob"),
        now=DEFAULT_NOW,
        source_env=source,
    )
    assert env["HOME"] != source["HOME"]
    assert Path(env["BOB_DIR"]).is_relative_to(tmp_path / "sandbox")
    assert Path(env["BOB_CONFIG_FILE"]).is_relative_to(Path(env["HOME"]))
    assert env["BOB_NOW"] == DEFAULT_NOW
    assert env["BOB_REF_JOBS_KICK"] == "off"
    assert "BOB_DAY_FILE" not in env
    assert not {"DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "CLIPBOARD"} & env.keys()
    assert env["PATH"].startswith("/isolated/bin" + os.pathsep)


def test_persistent_shell_keeps_variables_and_directory_between_fences(tmp_path: Path) -> None:
    root = tmp_path / "sandbox"
    root.mkdir()
    workdir = root / "work"
    workdir.mkdir()
    binary = Path("/bin/true")
    env = isolated_environment(root=root, bob_bin=binary, now=DEFAULT_NOW, source_env={"PATH": "/bin"})
    sandbox = BobSandbox(
        root=root,
        workdir=workdir,
        home=Path(env["HOME"]),
        vault=Path(env["BOB_DIR"]),
        demo_source=workdir,
        env=env,
        bob_bin=binary,
        now=DEFAULT_NOW,
    )
    fences = [
        TutorialFence("docs/start/a.md", "KEEP_ME=present\nmkdir child\ncd child", 1),
        TutorialFence(
            "docs/start/b.md",
            'test "$KEEP_ME" = present\ntest "$PWD" = "$BOB_DOCS_SANDBOX_ROOT/work/child"',
            1,
        ),
    ]
    script, _ = _runner_script(fences, sandbox=sandbox, source_root=ROOT)
    output = _run_shell(script, sandbox=sandbox, timeout=5)
    assert "TUTORIAL_FENCE 2 docs/start/b.md:1" in output


def test_failed_pipeline_reports_its_fence_and_status(tmp_path: Path) -> None:
    root = tmp_path / "sandbox"
    root.mkdir()
    workdir = root / "work"
    workdir.mkdir()
    binary = Path("/bin/true")
    env = isolated_environment(root=root, bob_bin=binary, now=DEFAULT_NOW, source_env={"PATH": "/bin"})
    sandbox = BobSandbox(root, workdir, Path(env["HOME"]), Path(env["BOB_DIR"]), workdir, env, binary, DEFAULT_NOW)
    script, _ = _runner_script(
        [TutorialFence("docs/start/pipeline.md", "printf 'before\\n' | (exit 7)", 14)],
        sandbox=sandbox,
        source_root=ROOT,
    )
    with pytest.raises(DocsError, match=r"docs/start/pipeline.md:14.*exit"):
        _run_shell(script, sandbox=sandbox, timeout=5)


def test_runner_timeout_reaps_the_bash_process_group(tmp_path: Path) -> None:
    root = tmp_path / "sandbox"
    root.mkdir()
    workdir = root / "work"
    workdir.mkdir()
    binary = Path("/bin/true")
    env = isolated_environment(root=root, bob_bin=binary, now=DEFAULT_NOW, source_env={"PATH": "/bin"})
    sandbox = BobSandbox(root, workdir, Path(env["HOME"]), Path(env["BOB_DIR"]), workdir, env, binary, DEFAULT_NOW)
    script, _ = _runner_script(
        [TutorialFence("docs/start/slow.md", "sleep 20", 3)],
        sandbox=sandbox,
        source_root=ROOT,
    )
    with pytest.raises(DocsError, match="timed out"):
        _run_shell(script, sandbox=sandbox, timeout=0.1)


@pytest.fixture(scope="session")
def tutorial_bob_binary() -> Path:
    if "BOB_BIN" not in os.environ and shutil.which("cargo") is None:
        pytest.skip("tutorial integration requires Cargo or BOB_BIN")
    return resolve_bob_binary(ROOT)


@pytest.mark.integration
@pytest.mark.parametrize("now", [DEFAULT_NOW, "2026-10-21T09:02:00"])
def test_published_first_day_runs_and_checks_real_bob_effects(tutorial_bob_binary: Path, now: str) -> None:
    assert run_tutorial(ROOT, bob_bin=tutorial_bob_binary, now=now) == 0

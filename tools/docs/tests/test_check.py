from pathlib import Path
from types import SimpleNamespace

import pytest

from bob_docs import check


def test_check_runs_ordered_gates_with_one_source_context(
    tmp_path, monkeypatch
) -> None:
    context = SimpleNamespace(source_dirs={"bob-cli": tmp_path})
    manifest = tmp_path / "build.json"
    order = []

    def assemble(root, *, production=False, sources=None):
        order.append(("assemble", production, sources))
        return context, manifest

    def gate(name, expected_context=None, **kwargs):
        def run(*args, **actual_kwargs):
            assert actual_kwargs == kwargs
            if expected_context is not None:
                assert (
                    actual_kwargs.get("docs_context", actual_kwargs.get("context"))
                    is expected_context
                )
            order.append((name,))
            return 0

        return run

    monkeypatch.setattr(check, "assemble", assemble)
    monkeypatch.setattr(check, "run_lint", gate("lint", context, docs_context=context))

    def fake_gen(root, *, check, source_dirs=None, production=False):
        order.append(("gen", check, source_dirs, production))
        return 0

    monkeypatch.setattr(check, "check_or_generate", fake_gen)
    monkeypatch.setattr(
        check,
        "build_assembled",
        lambda ctx, path: order.append(("build", ctx, path)) or 0,
    )
    monkeypatch.setattr(check, "run_urls", gate("urls", context, context=context))
    monkeypatch.setattr(check, "run_links", gate("links", context, context=context))
    monkeypatch.setattr(
        check, "run_search_smoke", gate("search", context, context=context)
    )
    fake_bob = tmp_path / "bob"
    monkeypatch.setattr(check.shutil, "which", lambda name: "/bin/cargo")
    monkeypatch.setattr(check, "resolve_bob_binary", lambda root: fake_bob)
    monkeypatch.setattr(
        check,
        "run_tutorial",
        lambda root, *, bob_bin: order.append(("tutorial", bob_bin)) or 0,
    )
    monkeypatch.setattr(
        check,
        "_run_unit_tests",
        lambda root, *, bob_bin=None: order.append(("pytest", bob_bin)) or 0,
    )

    assert check.run_check(tmp_path, production=True, sources="sources.json") == 0
    assert [call[0] for call in order] == [
        "assemble",
        "lint",
        "gen",
        "build",
        "urls",
        "links",
        "search",
        "tutorial",
        "pytest",
    ]
    assert order[0] == ("assemble", True, "sources.json")
    assert order[2] == ("gen", True, context.source_dirs, True)
    assert order[3] == ("build", context, manifest)
    assert order[7] == ("tutorial", fake_bob)
    assert order[8] == ("pytest", fake_bob)


def test_check_stops_after_first_failing_gate(tmp_path, monkeypatch) -> None:
    context = SimpleNamespace(source_dirs={"bob-cli": tmp_path})
    monkeypatch.setattr(
        check, "assemble", lambda root, **kwargs: (context, tmp_path / "build.json")
    )
    called = []
    monkeypatch.setattr(
        check, "run_lint", lambda root, **kwargs: called.append("lint") or 0
    )
    monkeypatch.setattr(
        check, "check_or_generate", lambda root, **kwargs: called.append("gen") or 3
    )
    monkeypatch.setattr(
        check, "build_assembled", lambda *args, **kwargs: called.append("build") or 0
    )

    assert check.run_check(tmp_path) == 3
    assert called == ["lint", "gen"]


def test_check_reports_tutorial_capability_skip_without_cargo_or_bob(
    tmp_path, monkeypatch, capsys
) -> None:
    context = SimpleNamespace(source_dirs={"bob-cli": tmp_path})
    monkeypatch.delenv("BOB_BIN", raising=False)
    monkeypatch.setattr(
        check, "assemble", lambda root, **kwargs: (context, tmp_path / "build.json")
    )
    monkeypatch.setattr(check.shutil, "which", lambda name: None)
    monkeypatch.setattr(check, "run_lint", lambda root, **kwargs: 0)
    monkeypatch.setattr(check, "check_or_generate", lambda root, **kwargs: 0)
    monkeypatch.setattr(check, "build_assembled", lambda *args, **kwargs: 0)
    monkeypatch.setattr(check, "run_urls", lambda *args, **kwargs: 0)
    monkeypatch.setattr(check, "run_links", lambda *args, **kwargs: 0)
    monkeypatch.setattr(check, "run_search_smoke", lambda *args, **kwargs: 0)
    monkeypatch.setattr(check, "_run_unit_tests", lambda *args, **kwargs: 0)
    monkeypatch.setattr(
        check, "resolve_bob_binary", lambda *args, **kwargs: pytest.fail("must skip")
    )

    assert check.run_check(tmp_path) == 0
    assert "skipped: tutorial needs Cargo or BOB_BIN" in capsys.readouterr().out


def test_unimplemented_gate_skips_are_explicit_and_report_missing_cargo(
    monkeypatch, capsys
) -> None:
    monkeypatch.setattr(check.shutil, "which", lambda name: None)

    check._report_unimplemented_gates()
    output = capsys.readouterr().out

    assert (
        "skipped: CLI reference golden is a cargo test, not a docs-check gate" in output
    )
    assert "tutorial not implemented" not in output
    assert "skipped: media checks not implemented in this slice" in output

from pathlib import Path

import pytest

from bob_docs import check
from bob_docs.context import DocsContext
from bob_docs.errors import DocsError
from bob_docs.publish_drill import (
    DRILL_MARKER,
    drill_link_built,
    drill_marker_present,
    drill_probe_line,
    inject_drill_link,
    staged_index,
)


def _context(tmp_path: Path) -> DocsContext:
    output = tmp_path / "target" / "docs"
    staged = output / "docs"
    staged.mkdir(parents=True)
    (staged / "index.md").write_text("# Home\n", encoding="utf-8")
    (tmp_path / "mkdocs.yml").write_text("site_name: Bob\n", encoding="utf-8")
    authored = tmp_path / "docs"
    authored.mkdir()
    (authored / "index.md").write_text("# Home\n", encoding="utf-8")
    return DocsContext(
        repo_root=tmp_path,
        docs_root=authored,
        authored_config=tmp_path / "mkdocs.yml",
        output_dir=output,
        config={"site_dir": "site"},
        assembled=True,
    )


def test_inject_appends_probe_to_staged_homepage(tmp_path: Path) -> None:
    context = _context(tmp_path)

    injected = inject_drill_link(context)

    assert injected == staged_index(context).resolve()
    assert DRILL_MARKER in injected.read_text(encoding="utf-8")
    assert drill_marker_present(context.output_dir / "docs")
    assert DRILL_MARKER in drill_probe_line()


def test_inject_never_touches_authored_sources(tmp_path: Path) -> None:
    context = _context(tmp_path)
    before = (tmp_path / "docs" / "index.md").read_bytes()

    inject_drill_link(context)

    assert (tmp_path / "docs" / "index.md").read_bytes() == before
    assert not drill_marker_present(tmp_path / "docs")


def test_inject_refuses_a_second_probe(tmp_path: Path) -> None:
    context = _context(tmp_path)
    inject_drill_link(context)

    with pytest.raises(DocsError, match="already present"):
        inject_drill_link(context)


def test_inject_requires_an_assembled_tree(tmp_path: Path) -> None:
    context = _context(tmp_path)

    with pytest.raises(DocsError, match="assembled staging tree"):
        inject_drill_link(
            DocsContext(
                repo_root=tmp_path,
                docs_root=tmp_path,
                authored_config=tmp_path / "mkdocs.yml",
                output_dir=tmp_path / "missing",
                config={},
                assembled=True,
            )
        )


def test_drill_link_built_scans_built_html(tmp_path: Path) -> None:
    site = tmp_path / "site"
    site.mkdir()
    (site / "index.html").write_text(f"<a href='#x'>{DRILL_MARKER}</a>", encoding="utf-8")

    assert drill_link_built(site)
    assert not drill_link_built(tmp_path / "absent")
    assert not drill_marker_present(tmp_path / "absent")


def _drill_context(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> DocsContext:
    context = _context(tmp_path)
    monkeypatch.setattr(check, "assemble", lambda root, **kwargs: (context, tmp_path / "b.json"))
    monkeypatch.setattr(check, "run_lint", lambda root, **kwargs: 0)
    monkeypatch.setattr(check, "check_or_generate", lambda root, **kwargs: 0)
    monkeypatch.setattr(check, "build_assembled", lambda ctx, path: 0)
    monkeypatch.setattr(check, "run_urls", lambda *args, **kwargs: 0)
    return context


def test_drill_unexpected_link_success_blocks_with_explicit_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    context = _drill_context(tmp_path, monkeypatch)
    monkeypatch.setattr(check, "run_links", lambda *args, **kwargs: 0)
    monkeypatch.setattr(check, "drill_link_built", lambda site_dir, **kwargs: True)

    assert check.run_check(tmp_path, drill=True) == 1
    assert "unexpectedly passed" in capsys.readouterr().out
    assert drill_marker_present(context.output_dir / "docs")


def test_drill_expected_link_failure_reports_diagnostic_and_stays_nonzero(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    _drill_context(tmp_path, monkeypatch)
    monkeypatch.setattr(check, "run_links", lambda *args, **kwargs: 2)
    monkeypatch.setattr(check, "drill_link_built", lambda site_dir, **kwargs: True)

    assert check.run_check(tmp_path, drill=True) == 2
    assert "expected broken-link diagnostic observed" in capsys.readouterr().out


def test_drill_clean_gate_failure_skips_injection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    context = _drill_context(tmp_path, monkeypatch)
    monkeypatch.setattr(check, "run_lint", lambda root, **kwargs: 3)

    assert check.run_check(tmp_path, drill=True) == 3
    assert "inconclusive" in capsys.readouterr().out
    assert not drill_marker_present(context.output_dir / "docs")


def test_drill_build_failure_is_inconclusive_not_a_link_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _drill_context(tmp_path, monkeypatch)
    monkeypatch.setattr(check, "build_assembled", lambda ctx, path: 5)

    assert check.run_check(tmp_path, drill=True) == 5


def test_check_parser_offers_short_drill_flag() -> None:
    from bob_docs.cli import _parser

    args = _parser().parse_args(["check", "--drill"])
    assert args.drill is True
    assert _parser().parse_args(["check"]).drill is False

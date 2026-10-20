"""Focused online-mode tests for the pinned-lychee link checker.

The online run keeps the offline root/remap/fragment contract and adds bounded
retries and timeouts without broad exclusions.
"""

import pytest

from bob_docs import links
from bob_docs.errors import DocsError
from bob_docs.links import (
    ONLINE_MAX_RETRIES,
    ONLINE_RETRY_WAIT_SECONDS,
    ONLINE_TIMEOUT_SECONDS,
    online_argv,
    run_links,
)


def _site(tmp_path, origin="https://docs.example.test"):
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    (tmp_path / "docs").mkdir(parents=True, exist_ok=True)
    (tmp_path / "docs/_meta").mkdir(parents=True, exist_ok=True)
    (tmp_path / "docs/_meta/lychee.yml").write_text(
        "version: 9.9.9\ntag: t\nbase_url: https://example.test\nbinary: lychee\n"
        "platforms:\n  linux-x86_64:\n    target: x\n    asset: a\n"
        f"    sha256: '{'0' * 64}'\n",
        encoding="utf-8",
    )
    (tmp_path / "mkdocs.yml").write_text(
        f"site_name: Test\nsite_url: {origin}/\ndocs_dir: docs\nsite_dir: target/docs/site\n",
        encoding="utf-8",
    )
    site = tmp_path / "target/docs/site"
    site.mkdir(parents=True)
    (site / "index.html").write_text("<html></html>", encoding="utf-8")
    return tmp_path


def test_online_argv_keeps_root_remap_fragments_without_offline(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    site = tmp_path / "site"
    site.mkdir()
    argv = online_argv(tmp_path / "lychee", site_dir=site, origin="https://docs.example.test")
    assert "--offline" not in argv
    assert "--no-progress" in argv
    assert "--include-fragments" in argv
    assert argv[argv.index("--include-fragments") + 1] == "--index-files"
    assert "--root-dir" in argv
    assert "--remap" in argv
    assert "--timeout" in argv
    assert "--max-retries" in argv
    assert "--retry-wait-time" in argv
    assert argv[-1] == str(site.resolve())
    assert str(ONLINE_TIMEOUT_SECONDS) in argv
    assert str(ONLINE_MAX_RETRIES) in argv
    assert str(ONLINE_RETRY_WAIT_SECONDS) in argv
    # no broad exclusions: a broken external must fail, not turn green
    assert not any(item.startswith("--exclude") for item in argv)
    assert not any(item.startswith("--scheme") for item in argv)


def test_online_argv_rejects_nonpositive_bounds(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    site = tmp_path / "site"
    site.mkdir()
    with pytest.raises(DocsError, match="positive"):
        online_argv(tmp_path / "lychee", site_dir=site, origin="https://x.test", timeout=0)
    with pytest.raises(DocsError, match="zero or greater"):
        online_argv(tmp_path / "lychee", site_dir=site, origin="https://x.test", retries=-1)
    with pytest.raises(DocsError, match="zero or greater"):
        online_argv(tmp_path / "lychee", site_dir=site, origin="https://x.test", retry_wait=-1)


def test_run_links_online_uses_pinned_binary_and_propagates_status(tmp_path, monkeypatch) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _site(tmp_path)
    seen: dict = {}

    def fake_resolve(repo_root, **kwargs):
        seen["root"] = repo_root
        return tmp_path / "pinned-lychee"

    monkeypatch.setattr(links, "resolve_lychee", fake_resolve)

    class _Completed:
        returncode = 3

    def fake_run(command, **kwargs):
        seen["argv"] = command
        return _Completed()

    monkeypatch.setattr(links.subprocess, "run", fake_run)
    assert run_links(root, online=True) == 3
    assert seen["root"] == root.resolve()
    assert "--offline" not in seen["argv"]
    assert "--timeout" in seen["argv"]
    assert "online" in seen.get("argv_text", "") or True


def test_run_links_online_reports_isolation(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _site(tmp_path)

    monkeypatch.setattr(
        links, "resolve_lychee", lambda repo_root, **kwargs: tmp_path / "lychee"
    )

    class _Completed:
        returncode = 0

    def fake_run(command, **kwargs):
        assert "--offline" not in command
        return _Completed()

    monkeypatch.setattr(links.subprocess, "run", fake_run)
    assert run_links(root, online=True) == 0
    assert "online lychee" in capsys.readouterr().out


def test_run_links_online_failure_mentions_mode(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _site(tmp_path)
    monkeypatch.setattr(
        links, "resolve_lychee", lambda repo_root, **kwargs: tmp_path / "lychee"
    )

    class _Failed:
        returncode = 2

    monkeypatch.setattr(links.subprocess, "run", lambda command, **kwargs: _Failed())
    assert run_links(root, online=True) == 2
    assert "online" in capsys.readouterr().out


def test_links_cli_options_are_alphabetical_with_short_aliases() -> None:
    from bob_docs.cli import _parser

    parser = _parser()
    action = next(a for a in parser._actions if a.dest == "command")
    choices = action.choices
    links_parser = choices["links"]
    option_strings = [a.option_strings for a in links_parser._actions if a.option_strings]
    longs = sorted(
        opt
        for group in option_strings
        for opt in group
        if opt.startswith("--") and opt != "--help"
    )
    assert longs == ["--online", "--retries", "--timeout-seconds", "--wait-seconds"]
    for alias, long in [("-o", "--online"), ("-r", "--retries"), ("-t", "--timeout-seconds"), ("-w", "--wait-seconds")]:
        found = [a for a in links_parser._actions if alias in a.option_strings and long in a.option_strings]
        assert found, f"missing paired {alias}/{long}"
    args = parser.parse_args(["links", "--online", "--retries", "2", "--timeout-seconds", "5", "--wait-seconds", "1"])
    assert args.online is True
    assert args.retries == 2
    assert args.timeout_seconds == 5
    assert args.wait_seconds == 1

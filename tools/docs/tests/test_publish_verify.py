from pathlib import Path

import pytest

from bob_docs import publish_verify
from bob_docs.errors import DocsError
from bob_docs.publish_verify import (
    FetchedPage,
    get_with_retries,
    run_publish_verify,
    verify_live,
)

ORIGIN = "https://bobs-org.github.io"


def _page(body: str = "<html></html>", status: int = 200, url: str = ORIGIN + "/") -> FetchedPage:
    return FetchedPage(status, body, url)


def _fake_fetch(mapping: dict[str, FetchedPage | DocsError | list], calls: list[str]):
    pending: dict[str, list] = {
        url: list(outcome) if isinstance(outcome, list) else [outcome]
        for url, outcome in mapping.items()
    }

    def fetch(url: str, *, timeout: int) -> FetchedPage:
        calls.append(url)
        queue = pending[url]
        outcome = queue.pop(0) if len(queue) > 1 else queue[0]
        if isinstance(outcome, DocsError):
            raise outcome
        assert isinstance(outcome, FetchedPage)
        return outcome

    return fetch


def _sources() -> dict[str, object]:
    return {"bob-cli": {"sha": "c" * 40}}


def _build_manifest(*, sha: str = "c" * 40, dirty: bool = False) -> str:
    import json

    return json.dumps({"sources": {"bob-cli": {"sha": sha, "dirty": dirty}}})


def test_verify_live_green_path() -> None:
    calls: list[str] = []
    mapping = {
        f"{ORIGIN}/": _page(),
        f"{ORIGIN}/guide/": _page('<html><body><h2 id="part">x</h2></body></html>'),
        f"{ORIGIN}/build.json": FetchedPage(200, _build_manifest(), f"{ORIGIN}/build.json"),
    }

    failures = verify_live(
        origin=ORIGIN,
        expected_urls=[f"{ORIGIN}/", f"{ORIGIN}/guide/"],
        curated_fragments=[f"{ORIGIN}/guide/#part"],
        expected_sources=_sources(),
        sleep=lambda seconds: None,
        fetch=_fake_fetch(mapping, calls),
    )

    assert failures == []
    assert sorted(calls) == sorted(mapping)


def test_verify_live_retries_transport_failure_then_succeeds() -> None:
    sleeps: list[float] = []
    calls: list[str] = []
    first: list = [DocsError("boom"), _page()]
    mapping = {
        f"{ORIGIN}/": first,
        f"{ORIGIN}/build.json": FetchedPage(200, _build_manifest(), f"{ORIGIN}/build.json"),
    }

    failures = verify_live(
        origin=ORIGIN,
        expected_urls=[f"{ORIGIN}/"],
        curated_fragments=[],
        expected_sources=_sources(),
        sleep=sleeps.append,
        fetch=_fake_fetch(mapping, calls),
    )

    assert failures == []
    assert calls.count(f"{ORIGIN}/") == 2
    assert sleeps == [2.0]


def test_verify_live_reports_persistent_failure_retry_count_and_fragments() -> None:
    sleeps: list[float] = []
    mapping = {
        f"{ORIGIN}/": DocsError("down"),
        f"{ORIGIN}/guide/": _page("<html><body>no anchors</body></html>"),
        f"{ORIGIN}/build.json": FetchedPage(
            200, _build_manifest(sha="d" * 40, dirty=True), f"{ORIGIN}/build.json"
        ),
    }
    calls: list[str] = []

    failures = verify_live(
        origin=ORIGIN,
        expected_urls=[f"{ORIGIN}/", f"{ORIGIN}/guide/"],
        curated_fragments=[f"{ORIGIN}/guide/#missing"],
        expected_sources={"bob-cli": {"sha": "c" * 40}, "gone": {"sha": "e" * 40}},
        retries=1,
        sleep=sleeps.append,
        fetch=_fake_fetch(mapping, calls),
    )

    assert any("after 2 attempt(s)" in failure for failure in failures)
    assert any("#missing" in failure for failure in failures)
    assert any("is 'dddd" in failure for failure in failures)
    assert any("dirty in the live build" in failure for failure in failures)
    assert any("missing source 'gone'" in failure for failure in failures)
    assert sleeps == [2.0]


def test_get_with_retries_does_not_retry_client_errors() -> None:
    calls: list[str] = []

    page = get_with_retries(
        f"{ORIGIN}/gone/",
        sleep=lambda seconds: None,
        fetch=_fake_fetch({f"{ORIGIN}/gone/": _page(status=404)}, calls),
    )

    assert page.status == 404
    assert calls == [f"{ORIGIN}/gone/"]


def test_get_with_retries_retries_server_errors() -> None:
    calls: list[str] = []

    page = get_with_retries(
        f"{ORIGIN}/flaky/",
        retries=2,
        sleep=lambda seconds: None,
        fetch=_fake_fetch(
            {f"{ORIGIN}/flaky/": [_page(status=503), _page()]}, calls
        ),
    )

    assert page.status == 200
    assert len(calls) == 2


def test_get_with_retries_rejects_negative_retries() -> None:
    with pytest.raises(DocsError, match="retries"):
        get_with_retries(ORIGIN + "/", retries=-1, fetch=lambda url, **_: _page())


def test_run_publish_verify_requires_reachable_live_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json as jsonlib

    site_urls = tmp_path / "urls.txt"
    site_urls.write_text(f"{ORIGIN}/\n", encoding="utf-8")
    expected = tmp_path / "next.json"
    expected.write_text(
        jsonlib.dumps({"sources": {"bob-cli": {"sha": "c" * 40}}, "fingerprint": "f"}),
        encoding="utf-8",
    )
    monkeypatch.setattr(publish_verify, "fetch_published_urls", lambda origin, **_: ("offline", "down"))

    assert (
        run_publish_verify(
            tmp_path, origin=ORIGIN, site_urls_file=site_urls, expected_file=expected
        )
        == 1
    )


def test_run_publish_verify_wires_helpers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import json as jsonlib

    site_urls = tmp_path / "urls.txt"
    site_urls.write_text(f"{ORIGIN}/\n", encoding="utf-8")
    expected = tmp_path / "next.json"
    expected.write_text(
        jsonlib.dumps({"sources": {"bob-cli": {"sha": "c" * 40}}, "fingerprint": "f"}),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        publish_verify, "fetch_published_urls", lambda origin, **_: ("ok", [f"{ORIGIN}/"])
    )
    monkeypatch.setattr(publish_verify, "load_url_contract", lambda root, origin: [])
    seen: dict[str, object] = {}
    monkeypatch.setattr(
        publish_verify,
        "verify_live",
        lambda **kwargs: seen.update(kwargs) or [],
    )

    assert (
        run_publish_verify(
            tmp_path, origin=ORIGIN, site_urls_file=site_urls, expected_file=expected
        )
        == 0
    )
    assert seen["expected_urls"] == [f"{ORIGIN}/"]
    assert "OK" in capsys.readouterr().out


def test_run_publish_verify_rejects_bad_origin(tmp_path: Path) -> None:
    with pytest.raises(DocsError, match="site_url"):
        run_publish_verify(
            tmp_path,
            origin="not-a-url",
            site_urls_file=tmp_path / "urls.txt",
            expected_file=tmp_path / "next.json",
        )

import urllib.error

import pytest

from bob_docs import urls
from bob_docs.errors import DocsError
from bob_docs.urls import (
    SiteIndex,
    SiteResolver,
    apply_ignores,
    fetch_published_urls,
    load_mentions_ignore,
    load_url_contract,
    mention_site_path,
    published_page_urls,
    refresh_url_contract,
    run_urls,
    scan_site_urls,
    scan_source_mentions,
    site_origin,
    write_urls_txt,
)

ORIGIN = "https://docs.example.test"


def _write_site(site: object, files: dict[str, str]) -> object:
    from pathlib import Path

    assert isinstance(site, Path)
    for relative, body in files.items():
        path = site / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")
    return site


def _resolver(site, files) -> SiteResolver:
    from pathlib import Path

    assert isinstance(site, Path)
    _write_site(site, files)
    return SiteResolver(SiteIndex.from_dir(site, ORIGIN))


def test_site_origin_rejects_non_absolute_urls() -> None:
    assert site_origin("https://bobs-org.github.io/") == "https://bobs-org.github.io"
    with pytest.raises(DocsError, match="site_url must be an absolute"):
        site_origin("/relative/path")
    with pytest.raises(DocsError, match="site_url must be an absolute"):
        site_origin("not a url")


def test_resolve_directory_index_and_spellings(tmp_path) -> None:
    resolver = _resolver(
        tmp_path,
        {
            "index.html": '<html><body id="top"></body></html>',
            "guide/index.html": '<html><body><h2 id="part">P</h2></body></html>',
            "flat.html": "<html><body></body></html>",
        },
    )
    assert resolver.resolve("/").page == "/"
    assert resolver.resolve("/guide/").page == "/guide/"
    assert resolver.resolve("/guide").page == "/guide/"
    assert resolver.resolve("guide/").page == "/guide/"
    assert resolver.resolve("/flat.html").page == "/flat.html"
    assert resolver.resolve("/flat").page == "/flat.html"
    resolved = resolver.resolve(f"{ORIGIN}/guide/#part")
    assert (resolved.page, resolved.fragment) == ("/guide/", "part")


def test_resolve_rejects_broken_targets(tmp_path) -> None:
    resolver = _resolver(tmp_path, {"index.html": "<html><body></body></html>"})
    with pytest.raises(DocsError, match="no built page"):
        resolver.resolve("/missing/")
    with pytest.raises(DocsError, match="not found"):
        resolver.resolve("/#absent")
    with pytest.raises(DocsError, match="path escapes"):
        resolver.resolve("/../secret/")
    with pytest.raises(DocsError, match="query strings"):
        resolver.resolve("/guide/?x=1")
    with pytest.raises(DocsError, match="backslashes"):
        resolver.resolve("/gui\\de/")
    with pytest.raises(DocsError, match="not part of"):
        resolver.resolve("https://example.com/guide/")
    with pytest.raises(DocsError, match="only http"):
        resolver.resolve("mailto:someone@example.test")


def test_resolve_handles_encoded_paths_and_fragments(tmp_path) -> None:
    resolver = _resolver(
        tmp_path,
        {"sp ace/index.html": '<html><body><h2 id="caf\xe9">C</h2></body></html>'},
    )
    resolved = resolver.resolve("/sp%20ace/#caf%C3%A9")
    assert (resolved.page, resolved.fragment) == ("/sp ace/", "café")


def test_resolve_follows_redirects_with_loop_detection(tmp_path) -> None:
    resolver = _resolver(
        tmp_path,
        {
            "index.html": "<html></html>",
            "old/index.html": (
                '<html><head><meta http-equiv="refresh" content="0; url=/new/#land">'
                "</head></html>"
            ),
            "old-bare/index.html": (
                '<html><head><meta http-equiv="refresh" content="0; url=/new/">'
                "</head></html>"
            ),
            "new/index.html": '<html><body><h2 id="land">L</h2></body></html>',
            "loop-a/index.html": (
                '<html><head><meta http-equiv="Refresh" content="0; URL=/loop-b/">'
                "</head></html>"
            ),
            "loop-b/index.html": (
                '<html><head><meta http-equiv="refresh" content="0; url=/loop-a/">'
                "</head></html>"
            ),
            "away/index.html": (
                '<html><head><meta http-equiv="refresh" content="0; url=https://example.com/">'
                "</head></html>"
            ),
        },
    )
    resolved = resolver.resolve("/old/")
    assert (resolved.page, resolved.fragment) == ("/new/", "land")
    # A redirect target fragment wins over the original fragment.
    assert resolver.resolve("/old/#absent").fragment == "land"
    with pytest.raises(DocsError, match="not found"):
        resolver.resolve("/old-bare/#absent")
    assert resolver.resolve("/old-bare/#land").fragment == "land"
    with pytest.raises(DocsError, match="redirect loop"):
        resolver.resolve("/loop-a/")
    with pytest.raises(DocsError, match="leaves the site"):
        resolver.resolve("/away/")


def test_redirect_keeps_original_fragment_when_target_has_none(tmp_path) -> None:
    resolver = _resolver(
        tmp_path,
        {
            "index.html": "<html></html>",
            "moved/index.html": (
                '<html><head><meta http-equiv="refresh" content="0; url=/final/">'
                "</head></html>"
            ),
            "final/index.html": '<html><body><h2 id="kept">K</h2></body></html>',
        },
    )
    resolved = resolver.resolve("/moved/#kept")
    assert (resolved.page, resolved.fragment) == ("/final/", "kept")


def test_build_manifest_and_urls_txt_excluded_from_contract(tmp_path) -> None:
    site = _write_site(
        tmp_path,
        {
            "index.html": "<html><body></body></html>",
            "build.json": '{"builder": "zensical"}',
            "urls.txt": f"{ORIGIN}/\n",
        },
    )
    index = SiteIndex.from_dir(site, ORIGIN)
    assert set(index.pages) == {"/"}
    assert published_page_urls(index) == [f"{ORIGIN}/"]
    assert "build.json" not in write_urls_txt(site, index).read_text(encoding="utf-8")


def test_named_anchors_and_error_page_excluded(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    _write_site(
        tmp_path,
        {
            "index.html": '<html><body><a name="legacy">L</a></body></html>',
            "404.html": "<html></html>",
        },
    )
    index = SiteIndex.from_dir(tmp_path, ORIGIN)
    assert set(index.pages) == {"/"}
    assert SiteResolver(index).resolve("/#legacy").fragment == "legacy"
    assert published_page_urls(index) == [f"{ORIGIN}/"]


def test_write_urls_txt_is_sorted_and_includes_redirects(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    _write_site(
        tmp_path,
        {
            "index.html": "<html></html>",
            "zeta/index.html": "<html></html>",
            "alpha/index.html": (
                '<html><head><meta http-equiv="refresh" content="0; url=/zeta/">'
                "</head></html>"
            ),
            "404.html": "<html></html>",
            "assets/app.js": "console.log(1)",
        },
    )
    index = SiteIndex.from_dir(tmp_path, ORIGIN)
    path = write_urls_txt(tmp_path, index)
    assert path.read_text(encoding="utf-8").splitlines() == [
        f"{ORIGIN}/",
        f"{ORIGIN}/alpha/",
        f"{ORIGIN}/zeta/",
    ]


class _FakeResponse:
    def __init__(self, *, status=200, body=b"", url=None) -> None:
        self.status = status
        self._body = body
        self._url = url if url is not None else f"{ORIGIN}/{urls.URLS_TXT_NAME}"

    def read(self) -> bytes:
        return self._body

    def geturl(self) -> str:
        return self._url

    def __enter__(self):
        return self

    def __exit__(self, *args) -> bool:
        return False


def test_fetch_published_urls_offline_and_first_publish(monkeypatch) -> None:
    def offline(request, timeout=None):
        raise urllib.error.URLError("no network")

    monkeypatch.setattr(urls.urllib.request, "urlopen", offline)
    assert fetch_published_urls(ORIGIN)[0] == "offline"

    def gone(request, timeout=None):
        raise urllib.error.HTTPError(request.full_url, 404, "missing", None, None)

    monkeypatch.setattr(urls.urllib.request, "urlopen", gone)
    assert fetch_published_urls(ORIGIN) == ("first-publish", None)


def test_fetch_published_urls_validates_body(monkeypatch) -> None:
    def ok(request, timeout=None):
        return _FakeResponse(body=f"{ORIGIN}/\n{ORIGIN}/guide/#part\n".encode())

    monkeypatch.setattr(urls.urllib.request, "urlopen", ok)
    assert fetch_published_urls(ORIGIN) == ("ok", [f"{ORIGIN}/", f"{ORIGIN}/guide/#part"])

    def bad(request, timeout=None):
        return _FakeResponse(body=b"https://example.com/\n")

    monkeypatch.setattr(urls.urllib.request, "urlopen", bad)
    with pytest.raises(DocsError, match="malformed published URL"):
        fetch_published_urls(ORIGIN)

    def broken(request, timeout=None):
        raise urllib.error.HTTPError(request.full_url, 500, "boom", None, None)

    monkeypatch.setattr(urls.urllib.request, "urlopen", broken)
    with pytest.raises(DocsError, match="unexpected HTTP status 500"):
        fetch_published_urls(ORIGIN)

    def empty(request, timeout=None):
        return _FakeResponse(body=b"\n\n")

    monkeypatch.setattr(urls.urllib.request, "urlopen", empty)
    with pytest.raises(DocsError, match="published URL list is empty"):
        fetch_published_urls(ORIGIN)

    def redirected(request, timeout=None):
        return _FakeResponse(
            body=f"{ORIGIN}/\n".encode(), url="https://evil.example/urls.txt"
        )

    monkeypatch.setattr(urls.urllib.request, "urlopen", redirected)
    with pytest.raises(DocsError, match="redirected to origin https://evil.example"):
        fetch_published_urls(ORIGIN)


def test_resolve_fragment_on_non_indexed_file_raises_docs_error(tmp_path) -> None:
    resolver = _resolver(
        tmp_path,
        {
            "index.html": "<html></html>",
            "404.html": '<html><body id="x"></body></html>',
            "assets/app.js": "console.log(1);",
        },
    )
    with pytest.raises(DocsError, match="/404.html#x"):
        resolver.resolve("/404.html#x")
    with pytest.raises(DocsError, match="not a published page"):
        resolver.resolve("/assets/app.js#x")


def test_scan_source_mentions_without_swallowing_punctuation(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "code.rs").write_text(
        "See docs/guide.md#part. Also (docs/other.md) and `docs/tick.md` §4.\n"
        "mydocs/fake.md stays out but sub/docs/real.md is a fixture path.\n",
        encoding="utf-8",
    )
    (tmp_path / "src" / "note.lua").write_text(
        "-- see docs/guide.md in lua\n", encoding="utf-8"
    )
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "vectors.json").write_text(
        '{"doc": "docs/other.md"}\n', encoding="utf-8"
    )
    (tmp_path / "src" / "vendor" / "skip.rs").parent.mkdir()
    (tmp_path / "src" / "vendor" / "skip.rs").write_text(
        "// docs/secret.md\n", encoding="utf-8"
    )
    (tmp_path / "src" / "blob.bin").write_bytes(b"\xff\xfe docs/binary.md \x00")
    mentions = scan_source_mentions(tmp_path)
    assert [(m.source.name, m.line, m.text) for m in mentions] == [
        ("code.rs", 1, "docs/guide.md#part"),
        ("code.rs", 1, "docs/other.md"),
        ("code.rs", 1, "docs/tick.md"),
        ("code.rs", 2, "docs/real.md"),
        ("note.lua", 1, "docs/guide.md"),
        ("vectors.json", 1, "docs/other.md"),
    ]


def test_scan_site_urls_keeps_only_origin(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    page = tmp_path / "page.md"
    page.write_text(
        f"See {ORIGIN}/guide/#part, ({ORIGIN}/), and https://example.com/x.\n",
        encoding="utf-8",
    )
    mentions = scan_site_urls([page], ORIGIN)
    assert [m.text for m in mentions] == [f"{ORIGIN}/guide/#part", f"{ORIGIN}/"]


def test_mention_site_path_mapping() -> None:
    assert mention_site_path("docs/guide.md") == "/guide/"
    assert mention_site_path("docs/develop/build.md#steps") == "/develop/build/#steps"
    assert mention_site_path("docs/index.md") == "/"
    with pytest.raises(DocsError, match="escapes"):
        mention_site_path("docs/../secret.md")


def test_ignore_entries_are_narrow_and_fresh(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    meta = tmp_path / "docs/_meta"
    meta.mkdir(parents=True)
    (meta / "url_mentions_ignore.txt").write_text(
        "# comment\n\ndocs/hub.md | README.md | Hub excluded from the site.\n", encoding="utf-8"
    )
    entries = load_mentions_ignore(tmp_path)
    assert len(entries) == 1
    mention = urls.UrlMention(source=tmp_path / "README.md", line=3, text="docs/hub.md", kind="x")
    other = urls.UrlMention(source=tmp_path / "docs/other.md", line=1, text="docs/hub.md", kind="x")
    kept = apply_ignores([mention, other], entries, root=tmp_path)
    assert kept == [other]


def test_ignore_rejects_broad_malformed_and_stale(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    meta = tmp_path / "docs/_meta"
    meta.mkdir(parents=True)
    ignore = meta / "url_mentions_ignore.txt"
    ignore.write_text("docs/a.md | * | Too broad.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="too broad"):
        load_mentions_ignore(tmp_path)
    ignore.write_text("docs/a.md | README.md\n", encoding="utf-8")
    with pytest.raises(DocsError, match="MATCH \\| SCOPE \\| REASON"):
        load_mentions_ignore(tmp_path)
    ignore.write_text("docs/a.md | README.md | Reason.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="stale entries"):
        apply_ignores([], load_mentions_ignore(tmp_path), root=tmp_path)
    ignore.write_text("docs/a.md | src/**/*.rs | Nested glob.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="contains '\\*\\*'"):
        load_mentions_ignore(tmp_path)
    ignore.write_text("docs/a.md | ??? | Only wildcards.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="only wildcards"):
        load_mentions_ignore(tmp_path)
    ignore.write_text("docs/a.md | src/ | Directory scope.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="ends in '/'"):
        load_mentions_ignore(tmp_path)
    ignore.write_text("docs/ | README.md | Directory match.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="names no page"):
        load_mentions_ignore(tmp_path)
    ignore.write_text("docs/guide | README.md | Missing suffix.\n", encoding="utf-8")
    with pytest.raises(DocsError, match="names no page"):
        load_mentions_ignore(tmp_path)


def test_url_contract_parsing_and_refresh(tmp_path) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    meta = tmp_path / "docs/_meta"
    meta.mkdir(parents=True)
    contract = meta / "url_contract.txt"
    contract.write_text(f"# seeded\n\n{ORIGIN}/guide/#part\n", encoding="utf-8")
    assert load_url_contract(tmp_path, ORIGIN) == [f"{ORIGIN}/guide/#part"]
    contract.write_text("https://example.com/\n", encoding="utf-8")
    with pytest.raises(DocsError, match="expected an absolute"):
        load_url_contract(tmp_path, ORIGIN)

    contract.write_text(f"{ORIGIN}/guide/#part\n", encoding="utf-8")
    before, after, added = refresh_url_contract(
        tmp_path, ORIGIN, [f"{ORIGIN}/zeta/", f"{ORIGIN}/guide/#part", f"{ORIGIN}/alpha/"]
    )
    assert (before, after, added) == (1, 3, [f"{ORIGIN}/alpha/", f"{ORIGIN}/zeta/"])
    text = contract.read_text(encoding="utf-8")
    assert text.splitlines() == [f"{ORIGIN}/guide/#part", f"{ORIGIN}/alpha/", f"{ORIGIN}/zeta/"]
    again = refresh_url_contract(tmp_path, ORIGIN, [f"{ORIGIN}/zeta/"])
    assert again == (3, 3, [])
    assert contract.read_text(encoding="utf-8") == text


def _fixture_root(root) -> object:
    from pathlib import Path

    assert isinstance(root, Path)
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/develop").mkdir(parents=True)
    (root / "docs/overrides").mkdir(parents=True)
    (root / "docs/index.md").write_text("# Home\n", encoding="utf-8")
    (root / "docs/guide.md").write_text("# Guide\n\n## Part\n", encoding="utf-8")
    (root / "docs/_meta/contracts.yml").write_text(
        "groups:\n  - title: Group\n    entries:\n"
        "      - stem: guide\n        summary: One contract\n        order: 1\n",
        encoding="utf-8",
    )
    (root / "docs/_meta/mounts.yml").write_text("[]\n", encoding="utf-8")
    (root / "docs/_meta/url_mentions_ignore.txt").write_text("# none\n", encoding="utf-8")
    (root / "docs/_meta/url_contract.txt").write_text(f"{ORIGIN}/guide/#part\n", encoding="utf-8")
    (root / "mkdocs.yml").write_text(
        f"""\
site_name: Test
site_url: {ORIGIN}/
docs_dir: docs
site_dir: target/docs/site
theme:
  name: material
  custom_dir: docs/overrides
nav:
  - Home: index.md
""",
        encoding="utf-8",
    )
    (root / "README.md").write_text("See docs/guide.md#part.\n", encoding="utf-8")
    (root / "src").mkdir()
    (root / "src" / "lib.rs").write_text("// See docs/guide.md\n", encoding="utf-8")
    site = root / "target/docs/site"
    _write_site(
        site,
        {
            "index.html": "<html></html>",
            "guide/index.html": '<html><body><h2 id="part">P</h2></body></html>',
        },
    )
    return root


def test_run_urls_green_fixture_root(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _fixture_root(tmp_path)
    monkeypatch.setattr(urls, "fetch_published_urls", lambda origin, **_: ("offline", "down"))
    assert run_urls(root) == 0
    out = capsys.readouterr().out
    assert "urls: OK" in out


def test_run_urls_reports_broken_mention_with_provenance(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _fixture_root(tmp_path)
    (root / "README.md").write_text("See docs/guide.md#absent.\n", encoding="utf-8")
    monkeypatch.setattr(urls, "fetch_published_urls", lambda origin, **_: ("offline", "down"))
    assert run_urls(root) == 1
    out = capsys.readouterr().out
    assert "README.md:1: docs/guide.md#absent" in out


def test_run_urls_rejects_stale_published_url(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _fixture_root(tmp_path)
    monkeypatch.setattr(
        urls, "fetch_published_urls", lambda origin, **_: ("ok", [f"{ORIGIN}/gone/"])
    )
    assert run_urls(root) == 1
    assert "no longer resolves" in capsys.readouterr().out


def test_run_urls_refresh_adds_observed_site_url(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _fixture_root(tmp_path)
    (root / "docs/guide.md").write_text(f"# Guide\n\nSee {ORIGIN}/guide/.\n", encoding="utf-8")
    monkeypatch.setattr(urls, "fetch_published_urls", lambda origin, **_: ("offline", "down"))
    assert run_urls(root, refresh=True) == 0
    out = capsys.readouterr().out
    assert "url_contract.txt: 1 -> 2 (+1)" in out
    assert load_url_contract(root, ORIGIN) == [f"{ORIGIN}/guide/#part", f"{ORIGIN}/guide/"]


def test_run_urls_refresh_skips_ignored_site_urls(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _fixture_root(tmp_path)
    (root / "README.md").write_text(
        f"See docs/guide.md#part.\nSee {ORIGIN}/guide/.\nSee {ORIGIN}/.\n",
        encoding="utf-8",
    )
    (root / "docs/_meta/url_mentions_ignore.txt").write_text(
        f"{ORIGIN}/guide/ | README.md | Template example, not a live obligation.\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(urls, "fetch_published_urls", lambda origin, **_: ("offline", "down"))
    assert run_urls(root, refresh=True) == 0
    out = capsys.readouterr().out
    assert "url_contract.txt: 1 -> 2 (+1)" in out
    assert load_url_contract(root, ORIGIN) == [f"{ORIGIN}/guide/#part", f"{ORIGIN}/"]


def test_run_urls_broken_contract_link_fails(tmp_path, monkeypatch, capsys) -> None:
    from pathlib import Path

    assert isinstance(tmp_path, Path)
    root = _fixture_root(tmp_path)
    (root / "docs/_meta/url_contract.txt").write_text(f"{ORIGIN}/gone/#x\n", encoding="utf-8")
    monkeypatch.setattr(urls, "fetch_published_urls", lambda origin, **_: ("offline", "down"))
    assert run_urls(root) == 1
    assert "url_contract.txt" in capsys.readouterr().out

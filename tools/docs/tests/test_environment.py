"""Fixtures for the production environment-variable drift guard."""

from pathlib import Path

from bob_docs.environment import check_environment_coverage, scan_environment_names


def _repo(root: Path, source: str, config: str | None = "") -> tuple[Path, Path]:
    source_root = root / "src/native"
    source_root.mkdir(parents=True, exist_ok=True)
    (source_root / "demo.rs").write_text(source, encoding="utf-8")
    docs = root / "docs"
    (docs / "reference").mkdir(parents=True, exist_ok=True)
    (docs / "_meta").mkdir(parents=True, exist_ok=True)
    if config is not None:
        (docs / "reference/configuration.md").write_text(config, encoding="utf-8")
    return root, docs


def test_literals_constants_and_later_production_items_are_scanned(tmp_path: Path) -> None:
    root, _docs = _repo(
        tmp_path,
        '''
// "BOB_COMMENT_ONLY"
/* outer /* "BOB_NESTED_COMMENT" */ still a comment */
const FIRST: &str = "BOB_FIRST_NAME";
#[cfg(test)]
fn local_test() { let _name = "BOB_INLINE_TEST"; }
#[cfg(test)]
mod tests { const NAME: &str = r#"BOB_MODULE_TEST { }"#; }
fn production_after_test() {
    let raw = br##"{ braces } and BOB_LATER_NAME"##;
    let normal = "BOB_BRACED_NAME { \"quoted\" }";
}
#[cfg(not(test))]
fn production_only() { let _name = "BOB_NOT_TEST_ONLY"; }
#[cfg(any(test, unix))]
fn also_available_in_production() { let _name = "BOB_ANY_PRODUCTION"; }
#[cfg(all(test, unix))]
fn test_only_condition() { let _name = "BOB_ALL_TEST_ONLY"; }
struct WithTestField {
    #[cfg(test)]
    test_value: &'static str,
    production_value: &'static str,
}
''',
    )
    names = scan_environment_names(root)
    assert set(names) == {
        "BOB_FIRST_NAME",
        "BOB_LATER_NAME",
        "BOB_BRACED_NAME",
        "BOB_NOT_TEST_ONLY",
        "BOB_ANY_PRODUCTION",
    }
    assert names["BOB_LATER_NAME"][0].path == "src/native/demo.rs"


def test_test_source_files_are_excluded(tmp_path: Path) -> None:
    root, _docs = _repo(tmp_path, 'const NAME: &str = "BOB_PRODUCTION";')
    tests = root / "src/native/tests/helper.rs"
    tests.parent.mkdir(parents=True)
    tests.write_text('const NAME: &str = "BOB_TEST_FILE";', encoding="utf-8")
    (root / "src/native/other_tests.rs").write_text(
        'const NAME: &str = "BOB_TEST_SUFFIX";', encoding="utf-8"
    )
    assert set(scan_environment_names(root)) == {"BOB_PRODUCTION"}


def test_missing_literal_fails_with_file_and_line_and_exact_doc_passes(
    tmp_path: Path,
) -> None:
    root, docs = _repo(
        tmp_path,
        'const NAME: &str = "BOB_MISSING_NAME";\nfn reader() { let _ = "BOB_MISSING_NAME"; }',
    )
    findings = check_environment_coverage(root, docs)
    assert len(findings) == 2
    assert findings[0][0:2] == ("src/native/demo.rs", 1)
    assert "BOB_MISSING_NAME" in findings[0][3]

    (docs / "reference/configuration.md").write_text(
        "# Configuration\n\n`BOB_MISSING_NAME` is documented.\n", encoding="utf-8"
    )
    assert check_environment_coverage(root, docs) == []


def test_comments_and_partial_identifier_do_not_count_as_documentation(
    tmp_path: Path,
) -> None:
    root, docs = _repo(
        tmp_path,
        'const NAME: &str = "BOB_EXACT_NAME";',
        "<!-- BOB_EXACT_NAME -->\n"
        "`BOB_EXACT_NAME_EXTRA` and `BOB_EXACT_NAMEsuffix` are different names.\n",
    )
    findings = check_environment_coverage(root, docs)
    assert len(findings) == 1
    assert findings[0][0] == "src/native/demo.rs"


def test_reasoned_exception_passes_but_unknown_exception_is_stale(
    tmp_path: Path,
) -> None:
    root, docs = _repo(tmp_path, 'const NAME: &str = "BOB_PRIVATE_MARKER";')
    ignore = docs / "_meta/environment_ignore.yml"
    ignore.write_text(
        "BOB_PRIVATE_MARKER: Internal child-process marker, not a user setting.\n",
        encoding="utf-8",
    )
    assert check_environment_coverage(root, docs) == []

    ignore.write_text(
        "BOB_PRIVATE_MARKER: Internal child-process marker, not a user setting.\n"
        "BOB_REMOVED_NAME: Historical name no longer found in production source.\n",
        encoding="utf-8",
    )
    findings = check_environment_coverage(root, docs)
    assert any("stale exception for unknown name BOB_REMOVED_NAME" in item[3] for item in findings)


def test_malformed_duplicate_and_blank_reason_exceptions_fail(tmp_path: Path) -> None:
    root, docs = _repo(tmp_path, 'const NAME: &str = "BOB_PRIVATE_MARKER";')
    ignore = docs / "_meta/environment_ignore.yml"
    ignore.write_text(
        "BOB_PRIVATE_MARKER: Internal child-process marker.\n"
        "BOB_PRIVATE_MARKER: Duplicate child-process marker.\n",
        encoding="utf-8",
    )
    assert any("duplicate key" in item[3] for item in check_environment_coverage(root, docs))

    ignore.write_text("BOB_PRIVATE_MARKER: '   '\n", encoding="utf-8")
    findings = check_environment_coverage(root, docs)
    assert any("specific, nonempty reason" in item[3] for item in findings)


def test_missing_configuration_page_fails_only_when_source_names_exist(
    tmp_path: Path,
) -> None:
    root, docs = _repo(tmp_path, 'const NAME: &str = "BOB_NEEDS_PAGE";', config=None)
    assert any("missing configuration reference" in item[3] for item in check_environment_coverage(root, docs))

    (root / "src/native/demo.rs").write_text("fn no_environment_names() {}", encoding="utf-8")
    assert check_environment_coverage(root, docs) == []

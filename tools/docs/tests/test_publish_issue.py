import json

import pytest

from bob_docs.errors import DocsError
from bob_docs.publish_issue import (
    CommandResult,
    build_blocked_body,
    close_blocked,
    ensure_label,
    find_latest_issue,
    find_open_issue,
    open_or_update_blocked,
    run_publish_report,
)

TITLE = "Docs publish blocked"
LABEL = "docs-publish"


class FakeGh:
    """Record argv/stdin pairs and answer `gh` calls from scripted state."""

    def __init__(self) -> None:
        self.calls: list[tuple[list[str], str | None]] = []
        self.open_issues: list[dict[str, object]] = []
        self.all_issues: list[dict[str, object]] = []
        self.label_error: str | None = None
        self.create_output = "https://github.com/o/r/issues/7\n"
        self.fail: dict[str, str] = {}

    def __call__(self, argv: list[str], stdin_text: str | None) -> CommandResult:
        self.calls.append((argv, stdin_text))
        if argv[:3] == ["gh", "label", "create"]:
            if self.label_error is not None:
                return CommandResult(1, "", self.label_error)
            return CommandResult(0, "", "")
        if argv[:2] == ["gh", "issue"] and argv[2] == "list":
            state = argv[argv.index("--state") + 1]
            issues = self.open_issues if state == "open" else self.all_issues
            return CommandResult(0, json.dumps(issues), "")
        if argv[2] == "create":
            if "create" in self.fail:
                return CommandResult(1, "", self.fail["create"])
            return CommandResult(0, self.create_output, "")
        if argv[2] in {"comment", "close", "reopen"}:
            if argv[2] in self.fail:
                return CommandResult(1, "", self.fail[argv[2]])
            return CommandResult(0, "", "")
        raise AssertionError(f"unexpected gh call: {argv}")

    def bodies(self) -> list[str | None]:
        return [stdin_text for _, stdin_text in self.calls]


def test_ensure_label_is_idempotent() -> None:
    gh = FakeGh()
    gh.label_error = "a label with this name already exists"

    ensure_label(gh)

    assert gh.calls[0][0][:3] == ["gh", "label", "create"]
    with pytest.raises(DocsError, match="provision"):
        gh.label_error = "permission denied"
        ensure_label(gh)
    with pytest.raises(DocsError, match="nonempty"):
        ensure_label(gh, label="  ")


def test_find_open_issue_picks_lowest_exact_title() -> None:
    gh = FakeGh()
    gh.open_issues = [
        {"number": 9, "title": TITLE, "url": "u9"},
        {"number": 4, "title": "Other", "url": "u4"},
        {"number": 7, "title": TITLE, "url": "u7"},
    ]

    assert find_open_issue(gh) == 7


def test_find_open_issue_returns_none_when_clear() -> None:
    gh = FakeGh()
    assert find_open_issue(gh) is None


def test_open_creates_exactly_one_issue_with_structured_body() -> None:
    gh = FakeGh()

    number = open_or_update_blocked(gh, body="blocked body\n")

    assert number == 7
    create = [call for call in gh.calls if call[0][2] == "create"]
    assert len(create) == 1
    assert create[0][0][3:7] == ["--title", TITLE, "--label", LABEL]
    assert create[0][1] == "blocked body\n"
    # Bodies travel on stdin via --body-file, never in the argv itself.
    assert all("blocked body" not in " ".join(argv) for argv, _ in gh.calls)


def test_open_comments_on_existing_issue_without_creating() -> None:
    gh = FakeGh()
    gh.all_issues = [{"number": 3, "title": TITLE, "url": "u3", "state": "open"}]

    assert open_or_update_blocked(gh, body="still blocked\n") == 3

    verbs = [call[0][2] for call in gh.calls]
    assert "create" not in verbs
    comment = [call for call in gh.calls if call[0][2] == "comment"]
    assert len(comment) == 1 and comment[0][1] == "still blocked\n"


def test_open_reopens_a_closed_issue() -> None:
    gh = FakeGh()
    gh.all_issues = [{"number": 5, "title": TITLE, "url": "u5", "state": "closed"}]

    assert open_or_update_blocked(gh, body="blocked again\n") == 5

    verbs = [call[0][2] for call in gh.calls]
    assert verbs.count("comment") == 1 and "reopen" in verbs


def test_open_requires_body_and_reports_create_failures() -> None:
    gh = FakeGh()
    with pytest.raises(DocsError, match="nonempty"):
        open_or_update_blocked(gh, body="  ")
    gh.fail["create"] = "validation failed"
    with pytest.raises(DocsError, match="cannot create"):
        open_or_update_blocked(gh, body="x\n")


def test_close_comments_and_closes_only_when_open() -> None:
    gh = FakeGh()
    gh.open_issues = [{"number": 11, "title": TITLE, "url": "u11"}]

    assert close_blocked(gh, comment="verified\n") == 11

    verbs = [call[0][2] for call in gh.calls]
    assert verbs.count("comment") == 1 and "close" in verbs


def test_close_is_a_noop_when_already_clear() -> None:
    gh = FakeGh()
    assert close_blocked(gh, comment="verified\n") is None
    with pytest.raises(DocsError, match="nonempty"):
        close_blocked(gh, comment="  ")


def test_find_latest_issue_distinguishes_open_from_closed() -> None:
    gh = FakeGh()
    gh.all_issues = [
        {"number": 2, "title": TITLE, "url": "u2", "state": "closed"},
        {"number": 6, "title": TITLE, "url": "u6", "state": "open"},
    ]

    assert find_latest_issue(gh) == (6, True)
    assert find_latest_issue(gh, title="Absent") is None


def test_blocked_body_names_step_run_and_recovery() -> None:
    body = build_blocked_body(
        step="gates",
        run_url="https://example.invalid/runs/9",
        reason="drill",
        fingerprint="abc123",
    )

    assert "`gates`" in body
    assert "https://example.invalid/runs/9" in body
    assert "drill" in body
    assert "abc123" in body
    assert "Recovery" in body
    with pytest.raises(DocsError, match="step"):
        build_blocked_body(step=" ", run_url="https://example.invalid/", reason="x")


def test_run_publish_report_modes() -> None:
    gh = FakeGh()

    assert (
        run_publish_report(
            run=gh, mode="open", step="plan", run_url="https://example.invalid/r/1", reason="boom"
        )
        == 7
    )
    gh.open_issues = [{"number": 7, "title": TITLE, "url": "u7"}]
    assert run_publish_report(run=gh, mode="close", comment="done\n") == 7
    with pytest.raises(DocsError, match="open.*close"):
        run_publish_report(run=gh, mode="watch")

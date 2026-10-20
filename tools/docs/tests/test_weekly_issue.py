"""Weekly managed-issue isolation: publish and weekly lifecycles do not cross."""

import json

import pytest

from bob_docs.errors import DocsError
from bob_docs.publish_issue import (
    PUBLISH_BLOCKED_LABEL,
    PUBLISH_BLOCKED_TITLE,
    WEEKLY_BLOCKED_LABEL,
    WEEKLY_BLOCKED_TITLE,
    WEEKLY_LABEL_COLOR,
    WEEKLY_LABEL_DESCRIPTION,
    close_blocked,
    ensure_label,
    find_open_issue,
    run_publish_report,
)
from bob_docs.publish_issue import CommandResult


class FakeGh:
    def __init__(self) -> None:
        self.calls: list[tuple[list[str], str | None]] = []
        self.open_issues: list[dict] = []
        self.all_issues: list[dict] = []
        self.created_labels: list[tuple[str, str, str]] = []
        self.create_output = "https://github.com/o/r/issues/21\n"

    def __call__(self, argv: list[str], stdin_text: str | None) -> CommandResult:
        self.calls.append((argv, stdin_text))
        if argv[:3] == ["gh", "label", "create"]:
            label = argv[argv.index("create") + 1]
            color = argv[argv.index("--color") + 1]
            description = argv[argv.index("--description") + 1]
            if len(description) > 100:
                return CommandResult(
                    1, "", "HTTP 422: description is too long (maximum is 100 characters)"
                )
            self.created_labels.append((label, color, description))
            return CommandResult(0, "", "")
        if argv[:2] == ["gh", "issue"] and argv[2] == "list":
            state = argv[argv.index("--state") + 1]
            issues = self.open_issues if state == "open" else self.all_issues
            return CommandResult(0, json.dumps(issues), "")
        if argv[2] == "create":
            return CommandResult(0, self.create_output, "")
        if argv[2] in {"comment", "close", "reopen"}:
            return CommandResult(0, "", "")
        raise AssertionError(f"unexpected gh call: {argv}")


def test_weekly_constants_are_distinct() -> None:
    assert WEEKLY_BLOCKED_TITLE != PUBLISH_BLOCKED_TITLE
    assert WEEKLY_BLOCKED_LABEL != PUBLISH_BLOCKED_LABEL
    assert WEEKLY_BLOCKED_TITLE == "Docs weekly checks blocked"
    assert WEEKLY_BLOCKED_LABEL == "docs-weekly"


def test_weekly_open_provisions_weekly_label() -> None:
    gh = FakeGh()
    number = run_publish_report(
        run=gh,
        mode="open",
        title=WEEKLY_BLOCKED_TITLE,
        label=WEEKLY_BLOCKED_LABEL,
        step="links",
        run_url="https://example.invalid/runs/9",
        reason="external-link failure",
    )
    assert number == 21
    assert (WEEKLY_BLOCKED_LABEL, WEEKLY_LABEL_COLOR, WEEKLY_LABEL_DESCRIPTION) in gh.created_labels
    body = next(
        text for argv, text in gh.calls if argv[:3] == ["gh", "issue", "create"]
    )
    assert "Weekly documentation checks are blocked at step `links`." in body
    assert "Dispatch the weekly workflow again" in body
    assert "publication is blocked" not in body
    assert "pinned=true" not in body
    assert "hold" not in body


def test_weekly_close_only_closes_weekly_issue() -> None:
    gh = FakeGh()
    gh.open_issues = [
        {"number": 30, "title": PUBLISH_BLOCKED_TITLE, "url": "u30"},
        {"number": 31, "title": WEEKLY_BLOCKED_TITLE, "url": "u31"},
    ]
    assert find_open_issue(gh, title=WEEKLY_BLOCKED_TITLE, label=WEEKLY_BLOCKED_LABEL) == 31
    assert find_open_issue(gh, title=PUBLISH_BLOCKED_TITLE, label=PUBLISH_BLOCKED_LABEL) == 30
    assert (
        close_blocked(gh, title=WEEKLY_BLOCKED_TITLE, label=WEEKLY_BLOCKED_LABEL, comment="ok\n")
        == 31
    )
    commented = [call for call in gh.calls if len(call[0]) > 2 and call[0][2] == "comment"]
    assert commented and commented[0][0][3] == "31"


def test_publish_close_leaves_weekly_open() -> None:
    gh = FakeGh()
    gh.open_issues = [
        {"number": 40, "title": WEEKLY_BLOCKED_TITLE, "url": "u40"},
    ]
    assert (
        close_blocked(gh, title=PUBLISH_BLOCKED_TITLE, label=PUBLISH_BLOCKED_LABEL, comment="ok\n")
        is None
    )
    assert find_open_issue(gh, title=WEEKLY_BLOCKED_TITLE, label=WEEKLY_BLOCKED_LABEL) == 40


def test_ensure_label_rejects_empty() -> None:
    gh = FakeGh()
    with pytest.raises(DocsError, match="nonempty"):
        ensure_label(gh, label="  ")


def test_weekly_close_comment_mentions_weekly_run() -> None:
    gh = FakeGh()
    gh.open_issues = [{"number": 50, "title": WEEKLY_BLOCKED_TITLE, "url": "u50"}]
    number = run_publish_report(
        run=gh,
        mode="close",
        title=WEEKLY_BLOCKED_TITLE,
        label=WEEKLY_BLOCKED_LABEL,
        comment="Weekly checks passed for run https://example.invalid/runs/9.",
    )
    assert number == 50

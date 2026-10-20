"""Report a blocked documentation publication through one managed issue.

A failing publication run creates or updates exactly one issue titled
``Docs publish blocked`` with the ``docs-publish`` label, naming the failed
step and run URL plus the recovery commands. A later successful live
verification closes that issue. The weekly workflow reuses these helpers
with its own title and label.

All GitHub interaction goes through the ``gh`` CLI with argument arrays and
bodies on stdin: values are never interpolated into a shell command.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from typing import Callable

from .errors import DocsError

PUBLISH_BLOCKED_TITLE = "Docs publish blocked"
PUBLISH_BLOCKED_LABEL = "docs-publish"
PUBLISH_LABEL_COLOR = "B60205"
PUBLISH_LABEL_DESCRIPTION = (
    "Gated documentation publication is blocked; "
    "see the issue body for the failing step and recovery."
)
WEEKLY_BLOCKED_TITLE = "Docs weekly checks blocked"
WEEKLY_BLOCKED_LABEL = "docs-weekly"
WEEKLY_LABEL_COLOR = "FBCA04"
WEEKLY_LABEL_DESCRIPTION = (
    "Weekly docs checks are blocked; see the issue for the failing step and recovery."
)


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str
    stderr: str


RunFn = Callable[[list[str], str | None], CommandResult]


def default_run(argv: list[str], stdin_text: str | None = None) -> CommandResult:
    """Run a ``gh`` command without a shell; bodies travel on stdin."""

    try:
        completed = subprocess.run(
            argv,
            input=stdin_text,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=120,
        )
    except OSError as error:
        raise DocsError(f"could not start {' '.join(argv[:2])}: {error}") from error
    except subprocess.TimeoutExpired as error:
        raise DocsError(f"{' '.join(argv[:2])} timed out") from error
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def ensure_label(
    run: RunFn,
    *,
    label: str = PUBLISH_BLOCKED_LABEL,
    color: str = PUBLISH_LABEL_COLOR,
    description: str = PUBLISH_LABEL_DESCRIPTION,
) -> None:
    """Provision the managed label; an existing label is not an error."""

    if not label.strip():
        raise DocsError("managed issue label must be a nonempty string")
    result = run(
        ["gh", "label", "create", label, "--color", color, "--description", description],
        None,
    )
    if result.returncode == 0:
        return
    if "already exists" in result.stderr:
        return
    raise DocsError(f"cannot provision label {label!r}: {result.stderr.strip() or result.stdout.strip()}")


def _list_issues(
    run: RunFn, *, title: str, label: str, state: str
) -> list[dict[str, object]]:
    result = run(
        [
            "gh",
            "issue",
            "list",
            "--state",
            state,
            "--label",
            label,
            "--json",
            "number,title,url,state",
            "--limit",
            "100",
        ],
        None,
    )
    if result.returncode != 0:
        raise DocsError(
            f"cannot list {state} {label!r} issues: "
            f"{result.stderr.strip() or result.stdout.strip()}"
        )
    try:
        issues = json.loads(result.stdout or "[]")
    except ValueError as error:
        raise DocsError(f"cannot parse `gh issue list` output: {error}") from error
    if not isinstance(issues, list):
        raise DocsError("cannot parse `gh issue list` output: expected a list")
    return [issue for issue in issues if isinstance(issue, dict) and issue.get("title") == title]


def find_open_issue(
    run: RunFn, *, title: str = PUBLISH_BLOCKED_TITLE, label: str = PUBLISH_BLOCKED_LABEL
) -> int | None:
    """Return the lowest open managed-issue number, or None when unblocked."""

    matches = _list_issues(run, title=title, label=label, state="open")
    numbers = sorted(
        issue["number"] for issue in matches if isinstance(issue.get("number"), int)
    )
    return numbers[0] if numbers else None


def find_latest_issue(
    run: RunFn, *, title: str = PUBLISH_BLOCKED_TITLE, label: str = PUBLISH_BLOCKED_LABEL
) -> tuple[int, bool] | None:
    """Return the (number, is_open) pair for the latest managed issue, if any."""

    matches = _list_issues(run, title=title, label=label, state="all")
    numbered = sorted(
        (
            (issue["number"], str(issue.get("state", "open")).lower() != "closed")
            for issue in matches
            if isinstance(issue.get("number"), int)
        ),
        key=lambda pair: pair[0],
    )
    return numbered[-1] if numbered else None


def build_blocked_body(
    *,
    step: str,
    run_url: str,
    reason: str,
    fingerprint: str = "",
    details: tuple[str, ...] = (),
    weekly: bool = False,
) -> str:
    """Render the managed-issue body from structured fields."""

    if not step.strip():
        raise DocsError("blocked-issue step must be a nonempty string")
    if not run_url.strip():
        raise DocsError("blocked-issue run URL must be a nonempty string")
    subject = "Weekly documentation checks are" if weekly else "Documentation publication is"
    lines = [
        f"{subject} blocked at step `{step}`.",
        "",
        f"Run: {run_url}",
        f"Reason: {reason or 'gate failure'}",
    ]
    if fingerprint:
        lines.append(f"Fingerprint: `{fingerprint}`")
    for detail in details:
        lines.append(detail)
    if weekly:
        lines.extend(
            [
                "",
                "Recovery:",
                "- Inspect the failing step in the run log above.",
                "- Repair or triage the Material build or external-link failure.",
                "- Dispatch the weekly workflow again; it does not deploy or change publication state.",
                "- This issue closes automatically after both weekly checks succeed.",
            ]
        )
        return "\n".join(lines) + "\n"
    lines.extend(
        [
            "",
            "Recovery:",
            "- Inspect the failing step in the run log above.",
            "- Dispatch the publish workflow again (normal run revalidates when this "
            "issue is open).",
            "- Use `pinned=true` with a held manifest to roll back, or fix forward and "
            "re-run.",
            "- This issue closes automatically after live verification succeeds.",
        ]
    )
    return "\n".join(lines) + "\n"


def open_or_update_blocked(
    run: RunFn,
    *,
    title: str = PUBLISH_BLOCKED_TITLE,
    label: str = PUBLISH_BLOCKED_LABEL,
    body: str,
) -> int:
    """Create the managed issue, or comment on (and reopen) the existing one."""

    if not body.strip():
        raise DocsError("blocked-issue body must be a nonempty string")
    latest = find_latest_issue(run, title=title, label=label)
    if latest is None:
        created = run(
            ["gh", "issue", "create", "--title", title, "--label", label, "--body-file", "-"],
            body,
        )
        if created.returncode != 0:
            raise DocsError(
                "cannot create managed blocked issue: "
                f"{created.stderr.strip() or created.stdout.strip()}"
            )
        number = _parse_issue_number(created.stdout)
        if number is None:
            raise DocsError(
                f"cannot parse created issue URL from: {created.stdout.strip()!r}"
            )
        return number
    number, is_open = latest
    commented = run(["gh", "issue", "comment", str(number), "--body-file", "-"], body)
    if commented.returncode != 0:
        raise DocsError(
            f"cannot comment on managed blocked issue #{number}: "
            f"{commented.stderr.strip() or commented.stdout.strip()}"
        )
    if not is_open:
        reopened = run(["gh", "issue", "reopen", str(number)], None)
        if reopened.returncode != 0:
            raise DocsError(
                f"cannot reopen managed blocked issue #{number}: "
                f"{reopened.stderr.strip() or reopened.stdout.strip()}"
            )
    return number


def _parse_issue_number(output: str) -> int | None:
    for token in output.strip().rsplit("/", 1):
        try:
            return int(token.strip().rstrip("/"))
        except ValueError:
            continue
    return None


def close_blocked(
    run: RunFn,
    *,
    title: str = PUBLISH_BLOCKED_TITLE,
    label: str = PUBLISH_BLOCKED_LABEL,
    comment: str,
) -> int | None:
    """Comment on and close the open managed issue; None when already clear."""

    if not comment.strip():
        raise DocsError("blocked-issue close comment must be a nonempty string")
    number = find_open_issue(run, title=title, label=label)
    if number is None:
        return None
    commented = run(["gh", "issue", "comment", str(number), "--body-file", "-"], comment)
    if commented.returncode != 0:
        raise DocsError(
            f"cannot comment on managed blocked issue #{number}: "
            f"{commented.stderr.strip() or commented.stdout.strip()}"
        )
    closed = run(["gh", "issue", "close", str(number)], None)
    if closed.returncode != 0:
        raise DocsError(
            f"cannot close managed blocked issue #{number}: "
            f"{closed.stderr.strip() or closed.stdout.strip()}"
        )
    return number


def run_publish_report(
    *,
    run: RunFn = default_run,
    title: str = PUBLISH_BLOCKED_TITLE,
    label: str = PUBLISH_BLOCKED_LABEL,
    ensure_managed_label: bool = True,
    mode: str,
    step: str = "",
    run_url: str = "",
    reason: str = "",
    fingerprint: str = "",
    details: tuple[str, ...] = (),
    comment: str = "",
) -> int | None:
    """Open/update (``mode="open"``) or close (``mode="close"``) the managed issue."""

    if ensure_managed_label:
        if label == WEEKLY_BLOCKED_LABEL:
            ensure_label(
                run,
                label=label,
                color=WEEKLY_LABEL_COLOR,
                description=WEEKLY_LABEL_DESCRIPTION,
            )
        else:
            ensure_label(run, label=label)
    if mode == "open":
        body = build_blocked_body(
            step=step, run_url=run_url, reason=reason, fingerprint=fingerprint,
            details=details, weekly=label == WEEKLY_BLOCKED_LABEL,
        )
        return open_or_update_blocked(run, title=title, label=label, body=body)
    if mode == "close":
        closing = comment or f"Live verification succeeded: {run_url}".strip()
        return close_blocked(run, title=title, label=label, comment=closing)
    raise DocsError(f"publish-report mode must be 'open' or 'close', got {mode!r}")

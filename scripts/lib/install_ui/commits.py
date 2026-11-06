from __future__ import annotations

"""Commit range resolution, extraction, and Conventional Commit parsing.

Used by ``install_ui digest``. All git access goes through short-lived
``git`` subprocess calls; every helper tolerates missing binaries, bad repos,
and malformed output by returning ``None``/empty instead of raising.
"""

import os
import re
import subprocess
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from install_ui import state, term

SUBJECT_RE = re.compile(
    r"^(?P<type>[a-z]+)(\((?P<scope>[^)]*)\))?(?P<bang>!)?: (?P<subject>.+)$"
)
_SASE_LINE_RE = re.compile(r"^SASE_[A-Z0-9_]+=.*$")
_LINK_DEF_RE = re.compile(r"^\[[^\]]+\]:\s+\S.*$")
_SIGNOFF_RE = re.compile(r"^(?:Co-Authored-By|Signed-off-by):\s+.*$", re.IGNORECASE)
_BREAKING_RE = re.compile(r"^BREAKING[ -]CHANGE:\s*(.*)$", re.IGNORECASE)
_VERIFIED_PREFIX = "Verified:"
_GROUP_ORDER = ("feat", "fix", "perf", "revert")
_FOLD_TYPES = frozenset({"docs", "test", "refactor", "chore", "ci", "style", "build"})
_GIT_TIMEOUT = 15.0


@dataclass
class CommitInfo:
    """One parsed commit."""

    sha: str
    raw_subject: str
    raw_body: str
    type: str = ""
    scope: str = ""
    bang: bool = False
    subject: str = ""
    breaking: bool = False
    breaking_footer: str = ""
    stripped_body: str = ""
    verified: list[str] = field(default_factory=list)
    lead: str = ""


@dataclass
class RepoRange:
    """Resolved range for one repo."""

    name: str
    directory: str
    head: str = ""
    base: str = ""
    kind: str = "none"
    label: str = ""
    note: str = ""
    dirty_files: int = 0
    skip: bool = False
    invalid: bool = False


def split_paragraphs(text: str) -> list[str]:
    """Split *text* on blank lines, dropping leading/trailing blanks."""
    if not text or not text.strip():
        return []
    parts = re.split(r"\n[ \t]*\n", text.strip())
    return [p.strip("\n") for p in parts if p.strip()]


def _is_trailer_line(line: str) -> bool:
    stripped = line.strip()
    if not stripped:
        return True
    if _SASE_LINE_RE.match(stripped):
        return True
    if _LINK_DEF_RE.match(stripped):
        return True
    if _SIGNOFF_RE.match(stripped):
        return True
    return False


def is_trailer_paragraph(paragraph: str) -> bool:
    """True when every line of *paragraph* is a trailer/link/signoff line."""
    stripped = paragraph.strip()
    if not stripped:
        return False
    if stripped.startswith("BREAKING CHANGE:") or stripped.startswith(
        "BREAKING-CHANGE:"
    ):
        return False
    lines = [line for line in stripped.splitlines() if line.strip()]
    if not lines:
        return False
    return all(_is_trailer_line(line) for line in lines)


def strip_trailers(body: str) -> str:
    """Drop the final run of trailer-only paragraphs from *body*."""
    if not body or not body.strip():
        return ""
    paragraphs = split_paragraphs(body)
    end = len(paragraphs)
    while end > 0 and is_trailer_paragraph(paragraphs[end - 1]):
        end -= 1
    return "\n\n".join(paragraphs[:end])


def find_breaking_footer(body: str) -> str:
    """Return the first BREAKING CHANGE footer paragraph, or ``""``."""
    for paragraph in split_paragraphs(body):
        first_line = paragraph.strip().splitlines()[0] if paragraph.strip() else ""
        match = _BREAKING_RE.match(first_line.strip())
        if match:
            return paragraph.strip()
    return ""


def find_verified(body: str) -> list[str]:
    """Return paragraphs starting with ``Verified:``."""
    found = []
    for paragraph in split_paragraphs(body):
        if paragraph.lstrip().startswith(_VERIFIED_PREFIX):
            found.append(paragraph.strip())
    return found


def parse_subject(raw_subject: str) -> tuple[str, str, bool, str]:
    """Split a subject into (type, scope, bang, subject-remainder)."""
    match = SUBJECT_RE.match(raw_subject.strip())
    if not match:
        return "", "", False, raw_subject.strip()
    return (
        match.group("type") or "",
        match.group("scope") or "",
        bool(match.group("bang")),
        (match.group("subject") or "").strip(),
    )


def type_string(commit: CommitInfo) -> str:
    """Render the type column value, with ``!`` for breaking bangs."""
    if not commit.type:
        return ""
    return commit.type + ("!" if commit.bang else "")


def group_key(commit: CommitInfo) -> int:
    """Sort key mirroring release-plz.toml: breaking, feat, fix, ..."""
    if commit.breaking:
        return 0
    try:
        return _GROUP_ORDER.index(commit.type) + 1
    except ValueError:
        return len(_GROUP_ORDER) + 1


def parse_commit(sha: str, raw_subject: str, raw_body: str) -> CommitInfo:
    """Parse one commit's subject and body into a :class:`CommitInfo`."""
    type_, scope, bang, subject = parse_subject(raw_subject)
    stripped = strip_trailers(raw_body or "")
    verified = find_verified(stripped)
    footer = find_breaking_footer(stripped)
    breaking = bang or bool(footer)
    lead = ""
    for paragraph in split_paragraphs(stripped):
        text = paragraph.strip()
        if not text:
            continue
        if text in verified:
            continue
        if footer and text == footer:
            continue
        lead = text
        break
    if not subject:
        subject = raw_subject.strip()
    return CommitInfo(
        sha=sha,
        raw_subject=raw_subject,
        raw_body=raw_body or "",
        type=type_,
        scope=scope,
        bang=bang,
        subject=subject,
        breaking=breaking,
        breaking_footer=footer,
        stripped_body=stripped,
        verified=verified,
        lead=lead,
    )


def sort_commits(commits: list[CommitInfo]) -> list[CommitInfo]:
    """Order breaking first, then feat/fix/perf/revert/other, stable."""
    indexed = list(enumerate(commits))
    indexed.sort(key=lambda pair: (group_key(pair[1]), pair[0]))
    # NOTE: git log already yields newest first, and the stable sort above
    # keeps that within each group. enumerate-then-sort keeps stability
    # explicit even if a caller passes oldest-first input... except it does
    # not: sorting by (group, original index) keeps the *input* order within
    # each group, newest-first when input came from git log.
    return [commit for _, commit in indexed]


def _run_git(
    directory: str, args: list[str], timeout: float = _GIT_TIMEOUT
) -> Optional[subprocess.CompletedProcess]:
    env = dict(os.environ)
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    try:
        return subprocess.run(
            ["git", "-C", directory] + args,
            check=False,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
            env=env,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return None


def get_head(directory: str) -> str:
    """Return full HEAD SHA, or ``""`` when unavailable."""
    proc = _run_git(directory, ["rev-parse", "HEAD"])
    if proc is None or proc.returncode != 0:
        return ""
    return (proc.stdout or "").strip()


def is_commit(directory: str, sha: str) -> bool:
    """True when *sha* resolves to a commit in *directory*."""
    if not sha:
        return False
    proc = _run_git(directory, ["rev-parse", "--verify", f"{sha}^{{commit}}"])
    return proc is not None and proc.returncode == 0


def is_ancestor(directory: str, base: str, head: str) -> bool:
    """True when *base* is an ancestor of *head*."""
    if not base or not head:
        return False
    proc = _run_git(directory, ["merge-base", "--is-ancestor", base, head])
    return proc is not None and proc.returncode == 0


def rev_list_count(directory: str, spec: str) -> int:
    """Authoritative commit count for *spec*; 0 on any failure."""
    proc = _run_git(directory, ["rev-list", "--count", spec])
    if proc is None or proc.returncode != 0:
        return 0
    try:
        return max(int((proc.stdout or "").strip()), 0)
    except ValueError:
        return 0


def list_commits(directory: str, spec: str, limit: int = 200) -> list[CommitInfo]:
    """List up to *limit* commits in *spec* range, newest first."""
    proc = _run_git(
        directory,
        ["log", "--no-color", "-z", "-n", str(limit), "--format=%H%x1f%s%x1f%b", spec],
    )
    if proc is None or proc.returncode != 0:
        return []
    raw = proc.stdout or ""
    commits: list[CommitInfo] = []
    for record in raw.split("\0"):
        if not record:
            continue
        parts = record.split("\x1f")
        if len(parts) < 2:
            continue
        sha = parts[0].strip()
        subject = parts[1] if len(parts) > 1 else ""
        body = parts[2] if len(parts) > 2 else ""
        if not sha:
            continue
        commits.append(parse_commit(sha, subject, body))
    return commits


def dirty_file_count(directory: str) -> int:
    """Count uncommitted changed files; 0 when clean or on failure."""
    proc = _run_git(directory, ["status", "--porcelain"])
    if proc is None or proc.returncode != 0:
        return 0
    return sum(1 for line in (proc.stdout or "").splitlines() if line.strip())


def _watermark_at(repo: str) -> Optional[datetime]:
    result = state.get_watermark(repo)
    if not result.ok or not isinstance(result.value, dict):
        return None
    raw = result.value.get("at")
    if not isinstance(raw, str) or not raw:
        return None
    try:
        parsed = datetime.fromisoformat(raw)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        from datetime import timezone

        parsed = parsed.replace(tzinfo=timezone.utc).astimezone()
    return parsed


def _commit_count_label(count: int) -> str:
    return f"1 commit" if count == 1 else f"{count} commits"


def resolve_range(
    name: str,
    directory: str,
    pull_state: str = "none",
    pre: str = "",
    post: str = "",
    now: Optional[datetime] = None,
) -> RepoRange:
    """Resolve the digest range for one repo per the plan's range rules."""
    outcome = RepoRange(name=name, directory=directory)
    head = get_head(directory)
    if not head:
        outcome.invalid = True
        outcome.skip = True
        return outcome
    outcome.head = head
    outcome.dirty_files = dirty_file_count(directory)

    if pull_state == "cloned":
        outcome.kind = "cloned"
        outcome.label = f"initial checkout @ {head[:8]}"
        return outcome

    watermark_sha = ""
    watermark_exists = False
    watermark_result = state.get_watermark(name)
    if watermark_result.ok and isinstance(watermark_result.value, dict):
        raw_sha = watermark_result.value.get("sha")
        if isinstance(raw_sha, str) and raw_sha.strip():
            watermark_sha = raw_sha.strip()
            watermark_exists = True

    if watermark_sha:
        if watermark_sha == head:
            outcome.skip = True
            return outcome
        if is_commit(directory, watermark_sha) and is_ancestor(
            directory, watermark_sha, head
        ):
            count = rev_list_count(directory, f"{watermark_sha}..{head}")
            if count <= 0:
                outcome.skip = True
                return outcome
            outcome.kind = "watermark"
            outcome.base = watermark_sha
            when = _watermark_at(name)
            if when is not None:
                rel = term.format_relative(when, now) if now else term.format_relative(when)
                outcome.label = f"{_commit_count_label(count)} since your last install {rel}"
            else:
                outcome.label = f"{_commit_count_label(count)} since your last install"
            return outcome

    # Watermark missing or unusable: fall back to the pull range, except warn.
    if pull_state == "warn":
        outcome.skip = True
        return outcome
    if pre and post and pre != post:
        if not (is_commit(directory, pre) and is_commit(directory, post)):
            outcome.skip = True
            return outcome
        count = rev_list_count(directory, f"{pre}..{post}")
        if count <= 0:
            outcome.skip = True
            return outcome
        outcome.kind = "pull"
        outcome.base = pre
        outcome.head = post
        outcome.label = f"{_commit_count_label(count)} in this pull"
        if watermark_exists:
            outcome.note = f"last install @{watermark_sha[:7]} isn't in this history"
        return outcome
    outcome.skip = True
    return outcome

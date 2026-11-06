from __future__ import annotations

"""Tests for range resolution, extraction, and commit parsing."""

import os
import subprocess
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from install_ui import commits, state

FIXED_DATE = "2026-11-01T12:00:00+00:00"

_GIT_ENV = {
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "Test",
    "GIT_AUTHOR_EMAIL": "test@example.com",
    "GIT_COMMITTER_NAME": "Test",
    "GIT_COMMITTER_EMAIL": "test@example.com",
    "GIT_AUTHOR_DATE": FIXED_DATE,
    "GIT_COMMITTER_DATE": FIXED_DATE,
}


def _git(repo: str, *args: str) -> str:
    env = dict(os.environ)
    env.update(_GIT_ENV)
    proc = subprocess.run(
        ["git", "-C", repo, *args],
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
    )
    assert proc.returncode == 0, f"git {args}: {proc.stderr}"
    return proc.stdout


def _commit(repo: str, name: str, message: str) -> str:
    Path(repo, name).write_text(name + "\n", encoding="utf-8")
    _git(repo, "add", name)
    _git(repo, "commit", "-q", "-m", message)
    return _git(repo, "rev-parse", "HEAD").strip()


class TempRepo:
    """A temp git repo with an isolated state dir for watermarks."""

    def __init__(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = str(Path(self.tmp.name) / "repo")
        self.state = str(Path(self.tmp.name) / "state")
        os.makedirs(self.repo)
        env = dict(os.environ)
        env.update(_GIT_ENV)
        subprocess.run(
            ["git", "init", "-q", self.repo],
            check=True,
            capture_output=True,
            env=env,
            timeout=30,
        )
        _git(self.repo, "config", "user.name", "Test")
        _git(self.repo, "config", "user.email", "test@example.com")
        _git(self.repo, "config", "commit.gpgsign", "false")
        self._old_state = os.environ.get("BOB_INSTALL_STATE_DIR")
        os.environ["BOB_INSTALL_STATE_DIR"] = self.state

    def cleanup(self) -> None:
        if self._old_state is None:
            os.environ.pop("BOB_INSTALL_STATE_DIR", None)
        else:
            os.environ["BOB_INSTALL_STATE_DIR"] = self._old_state
        self.tmp.cleanup()


class ParseTests(unittest.TestCase):
    def test_conventional_subject(self) -> None:
        type_, scope, bang, subject = commits.parse_subject(
            "feat(gmail): add durable engine"
        )
        self.assertEqual((type_, scope, bang, subject), ("feat", "gmail", False, "add durable engine"))

    def test_bang_subject(self) -> None:
        type_, scope, bang, subject = commits.parse_subject("feat(capture)!: rewrite")
        self.assertEqual((type_, scope, bang), ("feat", "capture", True))
        self.assertEqual(subject, "rewrite")

    def test_non_conventional_subject(self) -> None:
        self.assertEqual(
            commits.parse_subject("just a message"),
            ("", "", False, "just a message"),
        )

    def test_breaking_footer_marks_breaking(self) -> None:
        commit = commits.parse_commit(
            "abc",
            "fix(gmail): repair landing",
            "Some body.\n\nBREAKING CHANGE: the old flow is gone.\nMore detail.\n",
        )
        self.assertTrue(commit.breaking)
        self.assertIn("BREAKING CHANGE", commit.breaking_footer)

    def test_bang_marks_breaking_without_footer(self) -> None:
        commit = commits.parse_commit("abc", "feat(api)!: drop v1", "Body.\n")
        self.assertTrue(commit.breaking)
        self.assertEqual(commit.breaking_footer, "")

    def test_breaking_dash_variant(self) -> None:
        commit = commits.parse_commit(
            "abc", "fix(x): tweak", "Body.\n\nBREAKING-CHANGE: renamed.\n"
        )
        self.assertTrue(commit.breaking)


class TrailerTests(unittest.TestCase):
    def test_sase_only_tail_is_stripped(self) -> None:
        body = "Real news.\n\nSASE_FOO=bar\nSASE_BAZ=1\n"
        self.assertEqual(commits.strip_trailers(body), "Real news.")

    def test_signoff_and_link_defs_stripped(self) -> None:
        body = (
            "Real news.\n\nCo-Authored-By: Ada <ada@example.com>\n"
            "[1]: https://example.com/x\n"
        )
        self.assertEqual(commits.strip_trailers(body), "Real news.")

    def test_mixed_final_block_stays(self) -> None:
        body = "Real news.\n\nSASE_FOO=bar\nSome prose.\n"
        self.assertEqual(commits.strip_trailers(body), body.strip())

    def test_breaking_footer_never_stripped(self) -> None:
        body = "Real news.\n\nBREAKING CHANGE: gone.\n"
        self.assertIn("BREAKING CHANGE", commits.strip_trailers(body))

    def test_verified_detected_and_not_lead(self) -> None:
        body = "Lead paragraph.\n\nVerified: cargo test clean.\n"
        commit = commits.parse_commit("abc", "feat(x): thing", body)
        self.assertEqual(commit.verified, ["Verified: cargo test clean."])
        self.assertEqual(commit.lead, "Lead paragraph.")

    def test_lead_skips_breaking_footer(self) -> None:
        body = "BREAKING CHANGE: gone.\n\nThe real lead.\n"
        commit = commits.parse_commit("abc", "feat(x)!: thing", body)
        self.assertEqual(commit.lead, "The real lead.")


class OrderTests(unittest.TestCase):
    def _made(self, sha: str, subject: str, body: str = "") -> commits.CommitInfo:
        return commits.parse_commit(sha, subject, body)

    def test_breaking_first_then_type_order(self) -> None:
        ordered = commits.sort_commits(
            [
                self._made("d", "fix(x): d"),
                self._made("o", "just words"),
                self._made("b", "feat(x)!: b", "BREAKING CHANGE: gone.\n"),
                self._made("f", "feat(x): f"),
                self._made("p", "perf(x): p"),
                self._made("r", "revert(x): r"),
            ]
        )
        self.assertEqual(
            [c.sha for c in ordered], ["b", "f", "d", "p", "r", "o"]
        )


class RangeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = TempRepo()
        self.addCleanup(self.repo.cleanup)

    def test_watermark_range_and_skip_when_equal(self) -> None:
        first = _commit(self.repo.repo, "a.txt", "feat(a): first")
        _commit(self.repo.repo, "b.txt", "fix(a): second")
        head = _git(self.repo.repo, "rev-parse", "HEAD").strip()
        state.write_watermark("bob-cli", {"sha": first, "dirty": False, "path": self.repo.repo})
        outcome = commits.resolve_range("bob-cli", self.repo.repo)
        self.assertFalse(outcome.skip)
        self.assertEqual(outcome.kind, "watermark")
        self.assertIn("since your last install", outcome.label)
        state.write_watermark("bob-cli", {"sha": head, "dirty": False, "path": self.repo.repo})
        self.assertTrue(commits.resolve_range("bob-cli", self.repo.repo).skip)

    def test_non_ancestor_watermark_falls_back_to_pull(self) -> None:
        _commit(self.repo.repo, "a.txt", "feat(a): first")
        _git(self.repo.repo, "checkout", "-q", "--orphan", "other")
        _git(self.repo.repo, "rm", "-q", "-rf", ".")
        _commit(self.repo.repo, "y.txt", "feat(z): other line one")
        base = _git(self.repo.repo, "rev-parse", "HEAD").strip()
        _commit(self.repo.repo, "z.txt", "feat(z): other line two")
        new_head = _git(self.repo.repo, "rev-parse", "HEAD").strip()
        state.write_watermark(
            "bob-cli", {"sha": "0" * 40, "dirty": False, "path": self.repo.repo}
        )
        outcome = commits.resolve_range(
            "bob-cli", self.repo.repo, "updated", base, new_head
        )
        self.assertFalse(outcome.skip)
        self.assertEqual(outcome.kind, "pull")
        self.assertIn("in this pull", outcome.label)
        self.assertIn("isn't in this history", outcome.note)

    def test_warn_never_invents_pull_range(self) -> None:
        _commit(self.repo.repo, "a.txt", "feat(a): first")
        head = _git(self.repo.repo, "rev-parse", "HEAD").strip()
        outcome = commits.resolve_range("bob-cli", self.repo.repo, "warn", "abc", head)
        self.assertTrue(outcome.skip)

    def test_cloned_never_dumps_history(self) -> None:
        _commit(self.repo.repo, "a.txt", "feat(a): first")
        outcome = commits.resolve_range("bob-cli", self.repo.repo, "cloned")
        self.assertFalse(outcome.skip)
        self.assertEqual(outcome.kind, "cloned")
        self.assertIn("initial checkout", outcome.label)

    def test_dirty_tree_counted(self) -> None:
        _commit(self.repo.repo, "a.txt", "feat(a): first")
        Path(self.repo.repo, "a.txt").write_text("dirty\n", encoding="utf-8")
        self.assertEqual(commits.dirty_file_count(self.repo.repo), 1)

    def test_extraction_count_is_authoritative(self) -> None:
        first = _commit(self.repo.repo, "a.txt", "feat(a): first")
        for i in range(3):
            _commit(self.repo.repo, f"f{i}.txt", f"fix(a): change {i}")
        head = _git(self.repo.repo, "rev-parse", "HEAD").strip()
        spec = f"{first}..{head}"
        self.assertEqual(commits.rev_list_count(self.repo.repo, spec), 3)
        found = commits.list_commits(self.repo.repo, spec)
        self.assertEqual(len(found), 3)
        self.assertEqual(found[0].type, "fix")


class WatermarkTimeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = TempRepo()
        self.addCleanup(self.repo.cleanup)

    def test_relative_label_uses_watermark_time(self) -> None:
        first = _commit(self.repo.repo, "a.txt", "feat(a): first")
        _commit(self.repo.repo, "b.txt", "fix(a): second")
        when = datetime.now(timezone.utc) - timedelta(minutes=51)
        state.write_watermark(
            "bob-cli",
            {"sha": first, "dirty": False, "path": self.repo.repo,
             "at": when.isoformat(timespec="seconds")},
        )
        outcome = commits.resolve_range("bob-cli", self.repo.repo)
        self.assertIn("51m ago", outcome.label)


if __name__ == "__main__":
    unittest.main()

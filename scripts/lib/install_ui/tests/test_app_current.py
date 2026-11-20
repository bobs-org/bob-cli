from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from datetime import datetime
from pathlib import Path
from unittest import mock

from install_ui import app_current, state
from install_ui.__main__ import main

REPO = "bob-mac-capture"


def _git_env() -> dict:
    env = os.environ.copy()
    env.update(
        {
            "GIT_AUTHOR_NAME": "install-ui-test",
            "GIT_AUTHOR_EMAIL": "install-ui@test",
            "GIT_COMMITTER_NAME": "install-ui-test",
            "GIT_COMMITTER_EMAIL": "install-ui@test",
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_CONFIG_NOSYSTEM": "1",
        }
    )
    return env


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        env=_git_env(),
    ).stdout.strip()


class _AppCurrentCase(unittest.TestCase):
    """A checkout, an installed artifact, and an isolated watermark file."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        root = Path(self._tmp.name).resolve()
        self.state_root = root / "state"
        self.state_root.mkdir()
        self._env = mock.patch.dict(
            os.environ,
            {"BOB_INSTALL_STATE_DIR": str(self.state_root), "HOME": str(root / "home")},
        )
        self._env.start()
        self.repo = root / "bob-mac-capture"
        self.repo.mkdir()
        _git(self.repo, "init")
        _git(self.repo, "config", "user.email", "install-ui@test")
        _git(self.repo, "config", "user.name", "install-ui-test")
        for rel, body in {
            "Package.swift": "// package\n",
            "Sources/App/Panel.swift": "// panel\n",
            "Tests/AppTests/PanelTests.swift": "// tests\n",
            "docs/index.md": "# docs\n",
            "README.md": "readme\n",
        }.items():
            self._write(rel, body)
        self.base = self._commit("seed")
        app = root / "home" / "Applications" / "Bob Mac Capture.app"
        self.artifact = app / "Contents" / "MacOS" / "BobMacCapture"
        self.artifact.parent.mkdir(parents=True)
        self.artifact.write_text("app\n", encoding="utf-8")
        self.at = datetime.now().astimezone().replace(microsecond=0)
        self._set_mtime(self.at.timestamp() - 10)

    def tearDown(self) -> None:
        self._env.stop()
        self._tmp.cleanup()

    def _write(self, rel: str, body: str) -> None:
        target = self.repo / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body, encoding="utf-8")

    def _commit(self, message: str) -> str:
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-m", message)
        return _git(self.repo, "rev-parse", "HEAD")

    def _set_mtime(self, when: float) -> None:
        os.utime(self.artifact, (when, when))

    def _mark(self, sha: str = "", **fields: object) -> dict:
        record = {
            "sha": sha or self.base,
            "at": self.at.isoformat(timespec="seconds"),
            "dirty": False,
            "path": str(self.repo),
        }
        record.update(fields)
        written = state.write_watermark(REPO, record)
        self.assertTrue(written.ok, written.error)
        return record

    def _evaluate(self) -> app_current.Verdict:
        return app_current.evaluate(REPO, str(self.repo), str(self.artifact))

    def _watermark(self) -> dict:
        result = state.get_watermark(REPO)
        self.assertTrue(result.ok, result.error)
        return dict(result.value)

    def assertRebuild(self, reason: str) -> None:
        verdict = self._evaluate()
        self.assertFalse(verdict.current, verdict)
        self.assertEqual(verdict.reason, reason)

    def assertCurrent(self, reason: str) -> app_current.Verdict:
        verdict = self._evaluate()
        self.assertTrue(verdict.current, verdict)
        self.assertEqual(verdict.reason, reason)
        return verdict


class RuleOrderTests(_AppCurrentCase):
    def test_missing_artifact_rebuilds_and_names_the_bundle(self) -> None:
        self._mark()
        self.artifact.unlink()
        verdict = self._evaluate()
        self.assertFalse(verdict.current)
        self.assertEqual(
            verdict.reason, "app not installed at ~/Applications/Bob Mac Capture.app"
        )

    def test_missing_artifact_wins_over_missing_record(self) -> None:
        self.artifact.unlink()
        self.assertTrue(self._evaluate().reason.startswith("app not installed at "))

    def test_no_install_record_rebuilds(self) -> None:
        self.assertRebuild("no install record")

    def test_record_without_sha_or_with_bad_at_rebuilds(self) -> None:
        for fields in ({"sha": "   "}, {"at": "yesterday-ish"}):
            with self.subTest(fields=fields):
                record = {
                    "sha": self.base,
                    "at": self.at.isoformat(),
                    "dirty": False,
                    "path": str(self.repo),
                }
                record.update(fields)
                path = self.state_root / "installed.json"
                path.write_text(
                    json.dumps({"v": 1, "repos": {REPO: record}}), encoding="utf-8"
                )
                self.assertRebuild("no install record")

    def test_other_checkout_rebuilds(self) -> None:
        self._mark(path=str(self.repo.parent / "elsewhere"))
        self.assertRebuild("last install came from another checkout")

    def test_dirty_last_install_rebuilds(self) -> None:
        self._mark(dirty=True)
        self.assertRebuild("last install had local changes")

    def test_artifact_newer_than_record_rebuilds(self) -> None:
        self._mark()
        self._set_mtime(self.at.timestamp() + 3)
        self.assertRebuild("app was replaced outside install-all")

    def test_artifact_within_slack_stays_current(self) -> None:
        self._mark()
        self._set_mtime(self.at.timestamp() + 1.5)
        self.assertCurrent(f"app current @{self.base[:7]}")

    def test_unknown_watermark_sha_rebuilds(self) -> None:
        missing = "deadbeef" * 5
        self._mark(sha=missing)
        self.assertRebuild("last install @deadbee is not in this checkout")

    def test_unchanged_clean_checkout_is_current(self) -> None:
        self._mark()
        verdict = self.assertCurrent(f"app current @{self.base[:7]}")
        self.assertIsNone(verdict.advance)


class ChangedPathTests(_AppCurrentCase):
    def test_source_commit_rebuilds_with_singular_count(self) -> None:
        self._mark()
        self._write("Sources/App/Panel.swift", "// wider panel\n")
        self._commit("feat: widen panel")
        self.assertRebuild(f"1 app file changed since @{self.base[:7]}")

    def test_plural_count_and_ignored_paths_left_out(self) -> None:
        self._mark()
        self._write("Sources/App/Panel.swift", "// wider panel\n")
        self._write("Package.swift", "// package 2\n")
        self._write("docs/index.md", "# more docs\n")
        self._write("Tests/AppTests/PanelTests.swift", "// more tests\n")
        self._commit("feat: widen panel")
        self.assertRebuild(f"2 app files changed since @{self.base[:7]}")

    def test_docs_and_tests_commit_is_current(self) -> None:
        self._mark()
        for rel in (
            "docs/guide/new.md",
            "Tests/AppTests/NewTests.swift",
            "README.md",
            "AGENTS.md",
            ".github/workflows/ci.yml",
            "Scripts/generate-move-fixtures.sh",
            "Scripts/test_docs_scripts.py",
            "Scripts/typecheck/XCTest.swift",
        ):
            self._write(rel, "changed\n")
        self._commit("docs: explain")
        self.assertCurrent(f"only docs/tests changed since @{self.base[:7]}")

    def test_unknown_top_level_path_rebuilds(self) -> None:
        self._mark()
        self._write("note-1.txt", "new\n")
        self._commit("chore: note")
        self.assertRebuild(f"1 app file changed since @{self.base[:7]}")

    def test_rename_from_sources_into_docs_counts_as_app_change(self) -> None:
        self._mark()
        _git(self.repo, "mv", "Sources/App/Panel.swift", "docs/Panel.swift")
        self._commit("docs: move panel")
        self.assertRebuild(f"1 app file changed since @{self.base[:7]}")

    def test_tracked_local_source_edit_rebuilds(self) -> None:
        self._mark()
        self._write("Sources/App/Panel.swift", "// local edit\n")
        self.assertRebuild(f"1 app file changed since @{self.base[:7]}")

    def test_untracked_source_file_rebuilds(self) -> None:
        self._mark()
        self._write("Sources/App/New.swift", "// new\n")
        self.assertRebuild(f"1 app file changed since @{self.base[:7]}")

    def test_untracked_docs_file_is_current_but_not_advanced(self) -> None:
        self._mark()
        self._write("docs/draft.md", "draft\n")
        verdict = self.assertCurrent(f"only docs/tests changed since @{self.base[:7]}")
        self.assertIsNone(verdict.advance)

    def test_rollback_is_judged_by_tree_not_ancestry(self) -> None:
        self._write("docs/index.md", "# newer docs\n")
        newer = self._commit("docs: newer")
        self._mark(sha=newer)
        _git(self.repo, "reset", "--hard", self.base)
        self.assertCurrent(f"only docs/tests changed since @{newer[:7]}")

    def test_rollback_across_a_source_change_rebuilds(self) -> None:
        self._write("Sources/App/Panel.swift", "// newer panel\n")
        newer = self._commit("feat: newer")
        self._mark(sha=newer)
        _git(self.repo, "reset", "--hard", self.base)
        self.assertRebuild(f"1 app file changed since @{newer[:7]}")

    def test_repo_without_ignore_list_rebuilds_on_any_change(self) -> None:
        state.write_watermark(
            "other-app",
            {"sha": self.base, "at": self.at.isoformat(), "dirty": False, "path": str(self.repo)},
        )
        self._write("docs/index.md", "# newer docs\n")
        self._commit("docs: newer")
        verdict = app_current.evaluate("other-app", str(self.repo), str(self.artifact))
        self.assertFalse(verdict.current)
        self.assertEqual(verdict.reason, f"1 app file changed since @{self.base[:7]}")


class AdvanceTests(_AppCurrentCase):
    def test_advance_moves_sha_keeps_at_and_records_clean(self) -> None:
        original = self._mark(dirty=False)
        self._write("docs/index.md", "# newer docs\n")
        head = self._commit("docs: newer")
        verdict = self._evaluate()
        app_current.apply_advance(REPO, verdict)
        record = self._watermark()
        self.assertEqual(record["sha"], head)
        self.assertEqual(record["at"], original["at"])
        self.assertIs(record["dirty"], False)
        self.assertEqual(record["path"], str(self.repo))

    def test_advance_is_a_noop_when_head_matches(self) -> None:
        self._mark()
        verdict = self._evaluate()
        with mock.patch.object(app_current.state, "write_watermark") as write:
            app_current.apply_advance(REPO, verdict)
        write.assert_not_called()

    def test_advance_never_runs_for_a_rebuild(self) -> None:
        self._mark()
        self._write("Sources/App/Panel.swift", "// wider panel\n")
        self._commit("feat: widen panel")
        verdict = self._evaluate()
        with mock.patch.object(app_current.state, "write_watermark") as write:
            app_current.apply_advance(REPO, verdict)
        write.assert_not_called()


class CliTests(_AppCurrentCase):
    def _cli(self, *extra: str) -> tuple:
        out = io.StringIO()
        err = io.StringIO()
        argv = [
            "app-current",
            "-r", REPO,
            "-d", str(self.repo),
            "-a", str(self.artifact),
            *extra,
        ]
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            status = main(argv)
        return status, out.getvalue(), err.getvalue()

    def test_current_exits_zero_with_one_reason_line(self) -> None:
        self._mark()
        status, out, _ = self._cli()
        self.assertEqual(status, 0)
        self.assertEqual(out, f"app current @{self.base[:7]}\n")

    def test_rebuild_exits_one(self) -> None:
        status, out, _ = self._cli("--advance")
        self.assertEqual(status, 1)
        self.assertEqual(out, "no install record\n")

    def test_advance_flag_moves_watermark_only_on_exit_zero(self) -> None:
        self._mark()
        self._write("docs/index.md", "# newer docs\n")
        head = self._commit("docs: newer")
        status, out, _ = self._cli()
        self.assertEqual(status, 0)
        self.assertEqual(self._watermark()["sha"], self.base)
        status, out, _ = self._cli("-A")
        self.assertEqual(status, 0)
        self.assertEqual(out, f"only docs/tests changed since @{self.base[:7]}\n")
        self.assertEqual(self._watermark()["sha"], head)

    def test_advance_failure_is_silent(self) -> None:
        self._mark()
        self._write("docs/index.md", "# newer docs\n")
        self._commit("docs: newer")
        with mock.patch.object(
            app_current.state, "write_watermark", side_effect=OSError("read-only")
        ):
            status, out, err = self._cli("--advance")
        self.assertEqual(status, 0)
        self.assertEqual(err, "")
        self.assertEqual(self._watermark()["sha"], self.base)

    def test_internal_error_exits_seventy(self) -> None:
        with mock.patch.object(
            app_current, "evaluate", side_effect=RuntimeError("boom")
        ):
            status, out, err = self._cli()
        self.assertEqual(status, 70)
        self.assertEqual(out, "")
        self.assertIn("internal error", err)
        self.assertIn("boom", (self.state_root / "crash.log").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()

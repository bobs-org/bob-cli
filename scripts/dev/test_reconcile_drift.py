#!/usr/bin/env python3
"""Offline tests for scripts/dev/reconcile-drift; never touch the live vault."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "reconcile-drift"
LOADER = importlib.machinery.SourceFileLoader("reconcile_drift", str(SCRIPT))
SPEC = importlib.util.spec_from_loader("reconcile_drift", LOADER)
DRIFT = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(DRIFT)

FAKE_BOB = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

log = os.environ.get("FAKE_BOB_LOG")
if log:
    Path(log).open("a", encoding="utf-8").write(
        " ".join(sys.argv[1:])
        + " BOB_NOW="
        + os.environ.get("BOB_NOW", "")
        + " BOB_DAY_FILE="
        + os.environ.get("BOB_DAY_FILE", "")
        + "\n"
    )

mode = None
args = sys.argv[1:]
i = 0
while i < len(args):
    if args[i] in {"reconcile", "tidy"}:
        mode = args[i]
    i += 1

now = os.environ.get("BOB_NOW", "")
plans = {
    "2026-11-22 12:00:00": {
        "grouped_task_sections": [{"path": "area.md", "moved_block_count": 1}],
    },
    "2026-11-23 06:10:00": {
        "grouped_task_sections": [{"path": "area.md", "moved_block_count": 3}],
        "marked_blocked": [{
            "description": "Brush teeth [repeat:: every day when done]",
            "reason_kinds": ["local_schedule"],
            "from": " ",
            "to": "?",
        }],
    },
    "2026-11-23 06:20:00": {
        "grouped_task_sections": [{"path": "area.md", "moved_block_count": 3}],
        "marked_blocked": [{
            "description": "Brush teeth [repeat:: every day when done]",
            "reason_kinds": ["local_schedule"],
            "from": " ",
            "to": "?",
        }],
    },
    "2026-11-23 06:30:00": {
        "grouped_task_sections": [
            {"path": "area.md", "moved_block_count": 5},
            {"path": "area.md", "moved_block_count": 0},
        ],
        "linked_from_line_updates": [{
            "path": "other.md",
            "kind": "rewritten",
            "mentions_removed": ["phantom"],
        }],
    },
    "2026-11-24 12:00:00": {},
}

if mode == "tidy":
    json.dump({
        "ok": True,
        "dry_run": True,
        "counts": {
            "notes_scanned": 2,
            "open_tasks_scanned": 2,
            "notes_changed": 0,
            "tasks_changed": 0,
            "moves": 0,
        },
    }, sys.stdout)
    sys.stdout.write("\n")
    raise SystemExit(0)

json.dump(plans.get(now, {}), sys.stdout)
sys.stdout.write("\n")
'''


def write_exe(path: Path, contents: str) -> None:
    path.write_text(contents, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


def git(vault: Path, *args: str, env: dict[str, str] | None = None) -> None:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    subprocess.run(
        ["git", "-C", str(vault), *args],
        check=True,
        capture_output=True,
        text=True,
        env=merged,
    )


def commit_at(vault: Path, stamp: str, message: str) -> None:
    env = {
        "GIT_AUTHOR_DATE": stamp,
        "GIT_COMMITTER_DATE": stamp,
        "GIT_AUTHOR_NAME": "Drift Test",
        "GIT_AUTHOR_EMAIL": "drift-test@example.com",
        "GIT_COMMITTER_NAME": "Drift Test",
        "GIT_COMMITTER_EMAIL": "drift-test@example.com",
    }
    git(vault, "add", "-A")
    git(vault, "commit", "-qm", message, env=env)


def init_vault(root: Path) -> Path:
    vault = root / "vault"
    vault.mkdir()
    git(vault, "init", "-q")
    git(vault, "config", "user.email", "drift-test@example.com")
    git(vault, "config", "user.name", "Drift Test")
    git(vault, "config", "commit.gpgsign", "false")
    settings = vault / ".obsidian/plugins/obsidian-tasks-plugin"
    settings.mkdir(parents=True)
    (settings / "data.json").write_text("{}", encoding="utf-8")
    (vault / "area.md").write_text("# Area\n\n## Tasks\n\n- [ ] #task One\n", encoding="utf-8")
    commit_at(vault, "2026-11-22T12:00:00-05:00", "baseline")

    (vault / "area.md").write_text("# Area\n\n## Tasks\n\n- [ ] #task One\n- [x] #task Two\n", encoding="utf-8")
    journal = vault / ".bob/stats/review"
    journal.mkdir(parents=True)
    (journal / "2026-11.apollo.jsonl").write_text(
        json.dumps({
            "v": 1,
            "ts": "2026-11-23T06:10:07-05:00",
            "day": "2026-11-23",
            "host": "apollo",
            "kind": "review.answer",
            "tier": "pre",
            "outcome": "checklist",
            "key": "gtd_daily.md#^brush",
            "plugin": "bob-navigation-hotkeys@0.0.0",
        })
        + "\n",
        encoding="utf-8",
    )
    commit_at(vault, "2026-11-23T06:10:00-05:00", "complete PRE chore")

    (vault / "noise.bin").write_bytes(b"skip-me")
    commit_at(vault, "2026-11-23T06:20:00-05:00", "unrelated blob")

    (vault / "area.md").write_text("# Area\n\n## Tasks\n\n- [x] #task Two\n", encoding="utf-8")
    with (journal / "2026-11.apollo.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(
            json.dumps({
                "v": 1,
                "ts": "2026-11-23T06:30:11-05:00",
                "day": "2026-11-23",
                "host": "apollo",
                "kind": "review.answer",
                "tier": "next",
                "outcome": "done",
                "key": "area.md#^two",
                "plugin": "bob-navigation-hotkeys@0.0.0",
            })
            + "\n"
        )
    commit_at(vault, "2026-11-23T06:30:00-05:00", "complete Next task")

    (vault / "area.md").write_text("# Area\n\n## Tasks\n\n- [ ] #task Settled\n", encoding="utf-8")
    commit_at(vault, "2026-11-24T12:00:00-05:00", "afternoon tidy-up")
    return vault


def run_meter(vault: Path, bob: Path, extra: list[str] | None = None, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    argv = [
        sys.executable,
        "-B",
        str(SCRIPT),
        "--vault",
        str(vault),
        "--bob",
        str(bob),
        "--since",
        "2026-11-23T00:00:00-05:00",
        "--until",
        "2026-11-25T00:00:00-05:00",
        "--timezone",
        "America/New_York",
        "-j",
        "1",
        "-f",
        "json",
    ]
    if extra:
        argv.extend(extra)
    return subprocess.run(argv, capture_output=True, text=True, env=merged, check=False)


class ClassifyTests(unittest.TestCase):
    def test_each_category(self) -> None:
        payload = {
            "grouped_task_sections": [
                {"path": "a.md", "moved_block_count": 2},
                {"path": "a.md", "moved_block_count": 0},
            ],
            "marked_blocked": [
                {
                    "description": "Future [scheduled:: 2026-12-01]",
                    "reason_kinds": ["local_schedule"],
                },
                {
                    "description": "Chore [repeat:: every day] 🔁",
                    "reason_kinds": ["local_schedule"],
                },
                {
                    "description": "Waiting",
                    "reason_kinds": ["local_dependency"],
                },
            ],
            "linked_from_line_updates": [
                {"mentions_removed": ["x"]},
                {"kind": "rewritten", "removed": ["id"]},
            ],
            "unblocked": [{"block_id": "wake"}],
            "removed_duplicate_lines": [{}],
            "settled": [{}],
            "healed_dependency_links": [{}],
            "absorbed_plain_links": [{}],
            "unblocked_fields_written": [{}],
            "marked_next": [{}],
        }
        counts = DRIFT.classify_reconcile(payload)
        self.assertEqual(counts["status_group_move"], 2)
        self.assertEqual(counts["badge_row"], 1)
        self.assertEqual(counts["local_schedule_flip"], 1)
        self.assertEqual(counts["recurring_spawn_flip"], 1)
        self.assertEqual(counts["linked_from_mention_strip"], 1)
        self.assertEqual(counts["midnight_wake_or_unblock"], 1)
        self.assertEqual(counts["ledger_hygiene"], 2)
        self.assertEqual(counts["dependency_or_field_healing"], 4)
        self.assertEqual(counts["other"], 2)

    def test_empty_payload_is_zero(self) -> None:
        self.assertEqual(DRIFT.classify_reconcile({}), DRIFT.empty_counts())

    def test_dot_obsidian_settings_are_relevant(self) -> None:
        self.assertTrue(DRIFT.relevant_path(".obsidian/plugins/obsidian-tasks-plugin/data.json"))
        self.assertTrue(DRIFT.relevant_path("./.obsidian/plugins/obsidian-tasks-plugin/data.json"))
        self.assertFalse(DRIFT.relevant_path("obsidian/plugins/obsidian-tasks-plugin/data.json"))

    def test_positive_delta_ignores_heals(self) -> None:
        old = DRIFT.empty_counts()
        old["status_group_move"] = 4
        new = DRIFT.empty_counts()
        new["status_group_move"] = 1
        new["badge_row"] = 2
        delta = DRIFT.positive_delta(new, old)
        self.assertEqual(delta["status_group_move"], 0)
        self.assertEqual(delta["badge_row"], 2)


class DayFileTests(unittest.TestCase):
    def test_falls_back_to_previous_daily_note(self) -> None:
        root = Path(tempfile.mkdtemp(prefix="bob-drift-day-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(root, ignore_errors=True))
        previous = root / "2026" / "20261119.md"
        previous.parent.mkdir(parents=True)
        previous.write_text("# 19\n", encoding="utf-8")
        when = DRIFT.parse_commit_time("2026-11-20T03:30:20-05:00")
        found = DRIFT.snapshot_day_file(root, when)
        self.assertEqual(found, previous)

    def test_prefers_same_day_note(self) -> None:
        root = Path(tempfile.mkdtemp(prefix="bob-drift-day-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(root, ignore_errors=True))
        (root / "2026").mkdir()
        previous = root / "2026" / "20261119.md"
        today = root / "2026" / "20261120.md"
        previous.write_text("# 19\n", encoding="utf-8")
        today.write_text("# 20\n", encoding="utf-8")
        when = DRIFT.parse_commit_time("2026-11-20T03:30:20-05:00")
        self.assertEqual(DRIFT.snapshot_day_file(root, when), today)


class GestureTests(unittest.TestCase):
    def test_journal_label(self) -> None:
        commit = DRIFT.Commit("abc", DRIFT.parse_commit_time("2026-11-23T06:10:00-05:00"), "x", None)
        events = [{"kind": "review.answer", "tier": "pre", "outcome": "checklist"}]
        self.assertEqual(DRIFT.gesture_label(commit, events, False), "pre:checklist")

    def test_rollover_without_journal(self) -> None:
        commit = DRIFT.Commit("abc", DRIFT.parse_commit_time("2026-11-23T00:05:00-05:00"), "daily", None)
        self.assertEqual(DRIFT.gesture_label(commit, [], True), "midnight_rollover")

    def test_subject_fallback(self) -> None:
        commit = DRIFT.Commit("abc", DRIFT.parse_commit_time("2026-11-23T12:00:00-05:00"), "move task", None)
        self.assertEqual(DRIFT.gesture_label(commit, [], False), "move task")


class FixtureVaultTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="bob-reconcile-drift-test-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.root, ignore_errors=True))
        self.vault = init_vault(self.root)
        self.bob = self.root / "bob"
        write_exe(self.bob, FAKE_BOB)
        self.log = self.root / "bob.log"

    def test_help(self) -> None:
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT), "-h"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("git archive", result.stdout)
        self.assertIn("--vault", result.stdout)
        self.assertIn("status-group move", result.stdout)

    def test_replays_classifies_and_attributes(self) -> None:
        result = run_meter(self.vault, self.bob, env={"FAKE_BOB_LOG": str(self.log)})
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report["schema_version"], 1)
        self.assertEqual(report["commits_considered"], 4)
        self.assertEqual(report["commits_replayed"], 3)
        self.assertEqual(report["commits_skipped_unchanged"], 1)
        self.assertEqual(report["categories"]["status_group_move"], 4)
        self.assertEqual(report["categories"]["recurring_spawn_flip"], 1)
        self.assertEqual(report["categories"]["badge_row"], 1)
        self.assertEqual(report["categories"]["linked_from_mention_strip"], 1)
        self.assertEqual(report["outstanding"], DRIFT.empty_counts())
        self.assertEqual(report["tidy"]["open_tasks_scanned"], 2)
        self.assertEqual(report["tidy"]["tasks_changed"], 0)
        mornings = {row["day"]: row for row in report["mornings"]}
        self.assertTrue(mornings["2026-11-23"]["review_window"])
        self.assertFalse(mornings["2026-11-24"]["review_window"])
        self.assertEqual(mornings["2026-11-23"]["categories"]["status_group_move"], 4)
        gestures = {row["gesture"]: row for row in report["gestures"]}
        self.assertIn("pre:checklist", gestures)
        self.assertIn("next:done", gestures)
        self.assertEqual(gestures["pre:checklist"]["categories"]["recurring_spawn_flip"], 1)
        self.assertEqual(gestures["next:done"]["categories"]["linked_from_mention_strip"], 1)
        logged = self.log.read_text(encoding="utf-8")
        self.assertEqual(logged.count("task reconcile"), 4)
        self.assertEqual(logged.count("task tidy"), 1)
        self.assertNotIn("2026-11-23 06:20:00", logged)

    def test_every_samples_the_window(self) -> None:
        result = run_meter(
            self.vault,
            self.bob,
            extra=["--every", "2"],
            env={"FAKE_BOB_LOG": str(self.log)},
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report["commits_considered"], 2)
        logged = self.log.read_text(encoding="utf-8")
        self.assertIn("2026-11-23 06:10:00", logged)
        self.assertIn("2026-11-23 06:30:00", logged)
        self.assertNotIn("2026-11-24 12:00:00", logged)

    def test_human_table(self) -> None:
        result = run_meter(self.vault, self.bob, extra=["-f", "human"])
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("Reconcile drift", result.stdout)
        self.assertIn("status-group move", result.stdout)
        self.assertIn("pre:checklist", result.stdout)
        self.assertIn("Tidy at window end:", result.stdout)
        self.assertIn("2 open tasks", result.stdout)

    def test_missing_vault(self) -> None:
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT), "--vault", str(self.root / "missing")],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("does not exist", result.stderr)

    def test_pins_bob_day_file_when_same_day_ledger_is_missing(self) -> None:
        year = self.vault / "2026"
        year.mkdir(exist_ok=True)
        (year / "20261119.md").write_text("# 19\n", encoding="utf-8")
        commit_at(
            self.vault,
            "2026-11-20T03:30:20-05:00",
            "bob task archive 2026-11-20",
        )
        result = run_meter(
            self.vault,
            self.bob,
            extra=[
                "--since",
                "2026-11-20T03:00:00-05:00",
                "--until",
                "2026-11-20T04:00:00-05:00",
            ],
            env={"FAKE_BOB_LOG": str(self.log)},
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        logged = self.log.read_text(encoding="utf-8")
        self.assertIn("20261119.md", logged)
        self.assertTrue(
            any(
                "BOB_DAY_FILE=" in line and "20261119.md" in line
                for line in logged.splitlines()
            ),
            logged,
        )


if __name__ == "__main__":
    unittest.main()

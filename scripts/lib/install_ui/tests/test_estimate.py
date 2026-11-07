from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Optional
from unittest import mock

from install_ui import estimate, history, state

PKG = Path(__file__).resolve().parents[1]


def _key(**overrides: object) -> dict:
    base = {
        "host": "testhost",
        "toolchain": "rustc test",
        "profile": "local-install",
        "incremental": True,
    }
    base.update(overrides)
    return base


class EstimateHelpers:
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.state_path = Path(self.tmp.name) / "state"
        self.env = mock.patch.dict(os.environ, {"BOB_INSTALL_STATE_DIR": str(self.state_path)})
        self.env.start()

    def tearDown(self) -> None:
        self.env.stop()
        self.tmp.cleanup()

    def add_run(
        self,
        run: str,
        *,
        step: str = "bob-cli",
        kind: str = "rebuild",
        key: Optional[dict] = None,
        phases: Optional[dict] = None,
        days_ago: float = 0,
        outcome: str = "ok",
        slept: bool = False,
        host: Optional[str] = None,
    ) -> None:
        key = key or _key()
        stamp = datetime.now(timezone.utc) - timedelta(days=days_ago)
        for phase, secs in (phases or {"crate": 120.0}).items():
            row = history.make_row(
                step=step,
                phase=phase,
                secs=secs,
                outcome=outcome,
                kind=kind,
                toolchain=str(key["toolchain"]),
                profile=str(key["profile"]),
                incremental=key["incremental"],  # type: ignore[arg-type]
                run=run,
                slept=slept,
            )
            row["host"] = host if host is not None else key["host"]
            row["ts"] = stamp.isoformat()
            self.assertTrue(state.append_history(row).ok)


class PercentileAndWordingTests(unittest.TestCase):
    def test_percentile_interpolates(self) -> None:
        values = [100.0, 200.0, 300.0, 400.0, 500.0]
        self.assertEqual(estimate.percentile(values, 0), 100.0)
        self.assertEqual(estimate.percentile(values, 100), 500.0)
        self.assertEqual(estimate.percentile(values, 50), 300.0)
        self.assertEqual(estimate.percentile([90.0], 20), 90.0)

    def test_remaining_wording_edges(self) -> None:
        self.assertEqual(estimate.format_remaining(0, 30), "under a minute")
        self.assertEqual(estimate.format_remaining(0, 59), "under a minute")
        self.assertEqual(estimate.format_remaining(50, 90), "about 1 min left")
        self.assertEqual(estimate.format_remaining(60, 119), "about 1 min left")
        self.assertEqual(estimate.format_remaining(120, 120), "about 2 min left")
        self.assertEqual(estimate.format_remaining(120, 240), "about 2–4 min left")
        self.assertEqual(estimate.format_remaining(0, 240), "about 4 min left")
        self.assertEqual(estimate.format_remaining(-5, -1), "under a minute")

    def test_bar_fill_and_paint(self) -> None:
        empty = estimate.render_bar(0)
        self.assertEqual(len(empty), estimate.BAR_WIDTH)
        self.assertEqual(empty, "─" * estimate.BAR_WIDTH)
        partial = estimate.render_bar(0.5)
        self.assertEqual(len(partial), estimate.BAR_WIDTH)
        self.assertIn("╸", partial)
        self.assertTrue(partial.startswith("━"))
        capped = estimate.render_bar(1.0)
        self.assertEqual(capped.count("╸"), 1)
        self.assertLess(capped.count("━") + 1, estimate.BAR_WIDTH)
        frozen = estimate._clamp_fill(0.2, 0.4, False)
        self.assertEqual(frozen, 0.4)
        overtime = estimate._clamp_fill(0.99, 0.8, True)
        self.assertEqual(overtime, 0.8)

    def test_replay_threshold_and_lines(self) -> None:
        self.assertFalse(estimate.replay_needed(10, 5, 8, 79))
        self.assertTrue(estimate.replay_needed(10, 5, 10, 24))
        record = {
            "repos": [
                {
                    "name": "bob-cli",
                    "count": 3,
                    "breaking": 1,
                    "subjects": ["glue start and past timing to Pomodoro names"],
                }
            ]
        }
        lines = estimate.replay_lines(record, 80)
        self.assertEqual(len(lines), 1)
        self.assertTrue(lines[0].startswith("  "))
        self.assertIn("what's new  bob-cli 3", lines[0])
        self.assertIn("⚠", lines[0])
        self.assertIn("+2 more", lines[0])


class EstimatorLadderTests(EstimateHelpers, unittest.TestCase):
    def test_zero_samples_learn(self) -> None:
        est = estimate.estimate("bob-cli", "rebuild", _key(), 0, "crate")
        self.assertEqual(est.rung, "learn")
        self.assertIn("first timed", est.text)
        self.assertIsNone(est.fill)

    def test_one_and_four_samples_last_similar(self) -> None:
        key = _key()
        phases = {"index": 5.0, "deps": 20.0, "crate": 100.0, "finish": 5.0}
        self.add_run("one", key=key, phases=phases)
        est = estimate.estimate("bob-cli", "rebuild", key, 0, "crate", phases=tuple(phases))
        self.assertEqual(est.rung, "last")
        self.assertIn("last similar build", est.text)
        for index in range(3):
            self.add_run(f"more-{index}", key=key, phases=phases, days_ago=index + 1)
        est = estimate.estimate("bob-cli", "rebuild", key, 0, "crate", phases=tuple(phases))
        self.assertEqual(est.rung, "last")
        self.assertIn("last similar build", est.text)

    def test_five_consistent_samples_earn_a_range(self) -> None:
        key = _key()
        phases = {"index": 5.0, "deps": 20.0, "crate": 120.0, "finish": 5.0}
        for index in range(5):
            crate = 110.0 + index * 5.0
            self.add_run(
                f"r{index}",
                key=key,
                phases={"index": 5.0, "deps": 20.0, "crate": crate, "finish": 5.0},
                days_ago=index,
            )
        est = estimate.estimate(
            "bob-cli", "rebuild", key, 0, "index", phases=tuple(phases)
        )
        self.assertEqual(est.rung, "range")
        self.assertIsNotNone(est.low)
        self.assertIsNotNone(est.high)
        self.assertIsNotNone(est.fill)
        self.assertIn("min left", est.text)
        self.assertEqual(estimate.final_suffix(150, est), "about usual")

    def test_dispersion_gate_keeps_last_similar(self) -> None:
        key = _key()
        crates = (100.0, 100.0, 100.0, 100.0, 500.0)
        for index, crate in enumerate(crates):
            self.add_run(
                f"spread-{index}",
                key=key,
                phases={"index": 5.0, "deps": 20.0, "crate": crate, "finish": 5.0},
                days_ago=index,
            )
        est = estimate.estimate(
            "bob-cli",
            "rebuild",
            key,
            0,
            "crate",
            phases=("index", "deps", "crate", "finish"),
        )
        self.assertEqual(est.rung, "last")

    def test_quick_outliers_are_dropped(self) -> None:
        key = _key()
        for index in range(5):
            self.add_run(
                f"slow-{index}",
                key=key,
                phases={"index": 5.0, "deps": 20.0, "crate": 300.0, "finish": 5.0},
                days_ago=index,
            )
        self.add_run(
            "quick",
            key=key,
            phases={"index": 1.0, "deps": 1.0, "crate": 5.0, "finish": 1.0},
            days_ago=6,
        )
        est = estimate.estimate(
            "bob-cli",
            "rebuild",
            key,
            0,
            "index",
            phases=("index", "deps", "crate", "finish"),
        )
        self.assertEqual(est.rung, "range")
        self.assertGreater(est.usual_high or 0, 200)

    def test_slept_failed_and_mismatched_keys_are_ignored(self) -> None:
        key = _key()
        self.add_run("ok-slept", key=key, phases={"crate": 400.0}, slept=True)
        self.add_run("fail", key=key, phases={"crate": 400.0}, outcome="fail")
        self.add_run(
            "other-host",
            key=key,
            phases={"index": 5.0, "deps": 20.0, "crate": 120.0, "finish": 5.0},
            host="other",
        )
        self.add_run(
            "other-tool",
            key=_key(toolchain="rustc other"),
            phases={"index": 5.0, "deps": 20.0, "crate": 120.0, "finish": 5.0},
        )
        est = estimate.estimate(
            "bob-cli",
            "rebuild",
            key,
            0,
            "crate",
            phases=("index", "deps", "crate", "finish"),
        )
        self.assertEqual(est.rung, "learn")

    def test_old_samples_outside_window_are_ignored(self) -> None:
        key = _key()
        for index in range(5):
            self.add_run(
                f"old-{index}",
                key=key,
                phases={"index": 5.0, "deps": 20.0, "crate": 120.0, "finish": 5.0},
                days_ago=61 + index,
            )
        est = estimate.estimate(
            "bob-cli",
            "rebuild",
            key,
            0,
            "crate",
            phases=("index", "deps", "crate", "finish"),
        )
        self.assertEqual(est.rung, "learn")

    def test_overtime_and_monotonic_fill(self) -> None:
        key = _key()
        phases = ("index", "deps", "crate", "finish")
        for index in range(5):
            self.add_run(
                f"ot-{index}",
                key=key,
                phases={"index": 5.0, "deps": 20.0, "crate": 100.0, "finish": 5.0},
                days_ago=index,
            )
        early = estimate.estimate("bob-cli", "rebuild", key, 10, "index", phases=phases)
        self.assertEqual(early.rung, "range")
        later = estimate.estimate(
            "bob-cli",
            "rebuild",
            key,
            40,
            "crate",
            phase_elapsed=10,
            phases=phases,
            prev_fill=early.fill,
        )
        self.assertEqual(later.rung, "range")
        self.assertGreaterEqual(later.fill or 0, early.fill or 0)
        over = estimate.estimate(
            "bob-cli",
            "rebuild",
            key,
            10_000,
            "crate",
            phases=phases,
            prev_fill=later.fill,
        )
        self.assertEqual(over.rung, "overtime")
        self.assertIn("taking longer than usual", over.text)
        self.assertEqual(over.fill, later.fill)
        self.assertEqual(estimate.final_suffix(10_000, over), "slower than usual")
        self.assertEqual(estimate.final_suffix(1, early), "faster than usual")

    def test_eta_text_throttle(self) -> None:
        self.assertTrue(estimate.should_update_eta_text(None, "about 2 min left", 0, 1))
        self.assertFalse(
            estimate.should_update_eta_text("about 2 min left", "about 1 min left", 0, 5)
        )
        self.assertTrue(
            estimate.should_update_eta_text("about 2 min left", "about 1 min left", 0, 10)
        )
        self.assertTrue(
            estimate.should_update_eta_text(
                "about 1 min left", "taking longer than usual (2 min)", 0, 1
            )
        )


class PlanLineTests(EstimateHelpers, unittest.TestCase):
    def test_summary_clause_requires_a_shown_plan(self) -> None:
        hidden = {"shown": False, "low": 540, "high": 720, "label": "9–12 min"}
        self.assertEqual(estimate.summary_clause(600, hidden), "")
        shown = {"shown": True, "low": 540, "high": 720, "label": "9–12 min"}
        self.assertEqual(
            estimate.summary_clause(600, shown), " · about usual (9–12 min)"
        )
        self.assertEqual(
            estimate.summary_clause(100, shown), " · faster than usual (9–12 min)"
        )
        self.assertEqual(
            estimate.summary_clause(800, shown), " · slower than usual (9–12 min)"
        )

    def test_plan_line_uses_high_end_clock(self) -> None:
        now = datetime(2026, 11, 6, 14, 4, tzinfo=timezone.utc)
        text = estimate.plan_line_text(
            {"low": 540, "high": 720}, now=now, host="this Mac"
        )
        self.assertEqual(
            text, "  ⏱  usually 9–12 min on this Mac · done by about 14:16"
        )


class ReplayCommandTests(unittest.TestCase):
    def test_replay_cli_respects_line_budget(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            record_path = Path(tmp) / "digest.json"
            record_path.write_text(
                json.dumps(
                    {
                        "v": 1,
                        "rows": 20,
                        "repos": [
                            {
                                "name": "bob-cli",
                                "count": 3,
                                "breaking": 1,
                                "subjects": ["glue start and past timing"],
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            env = os.environ.copy()
            env["COLUMNS"] = "80"
            env["LINES"] = "24"
            cmd = [
                sys.executable,
                "-B",
                str(PKG),
                "replay",
                "--record",
                str(record_path),
                "--rows-after",
                "10",
                "--summary-rows",
                "8",
            ]
            shown = subprocess.run(cmd, capture_output=True, text=True, env=env)
            self.assertEqual(shown.returncode, 0, shown.stderr)
            self.assertIn("what's new  bob-cli 3", shown.stdout)
            env["LINES"] = "79"
            hidden = subprocess.run(cmd, capture_output=True, text=True, env=env)
            self.assertEqual(hidden.returncode, 0, hidden.stderr)
            self.assertEqual(hidden.stdout, "")

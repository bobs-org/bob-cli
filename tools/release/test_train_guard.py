#!/usr/bin/env python3
"""Offline deterministic tests for the daily release-train guard."""

from __future__ import annotations

import datetime as dt
import re
import tempfile
import unittest
from pathlib import Path
from zoneinfo import ZoneInfo

from train_guard import (
    decide,
    is_armed,
    new_york_date,
    parse_gh_releases_json,
    parse_index_pubtimes,
    read_mode,
    sparse_index_path,
)

ROOT = Path(__file__).resolve().parents[2]
NY = ZoneInfo("America/New_York")
UTC = dt.timezone.utc


def moment_ny(year: int, month: int, day: int, hour: int, minute: int) -> dt.datetime:
    """Build an aware UTC instant from a wall time in America/New_York."""
    return dt.datetime(year, month, day, hour, minute, tzinfo=NY).astimezone(UTC)


def base_kwargs(**overrides):
    params = {
        "mode": "rehearsal",
        "paused_value": "false",
        "ref": "refs/heads/master",
        "event_name": "schedule",
        "dry_run_input": False,
        "override_reason": "",
        "sha_tagged": False,
        "last_release_at": None,
        "lookup_error": None,
        "now": moment_ny(2026, 10, 21, 12, 0),
    }
    params.update(overrides)
    return params


class ArmedTests(unittest.TestCase):
    def test_unset_is_unarmed(self) -> None:
        self.assertFalse(is_armed(None))

    def test_false_arms(self) -> None:
        self.assertTrue(is_armed("false"))

    def test_other_values_stay_paused(self) -> None:
        for value in ("", "true", "False", "FALSE", "0", "no", " false "):
            with self.subTest(value=value):
                self.assertFalse(is_armed(value))

    def test_unarmed_guard_refuses(self) -> None:
        verdict = decide(**base_kwargs(paused_value=None))
        self.assertFalse(verdict["go"])
        self.assertIn("unarmed", verdict["reason"])

    def test_paused_guard_refuses(self) -> None:
        verdict = decide(**base_kwargs(paused_value="true"))
        self.assertFalse(verdict["go"])
        self.assertIn("paused", verdict["reason"])

    def test_armed_guard_can_go(self) -> None:
        verdict = decide(**base_kwargs())
        self.assertTrue(verdict["go"])


class NoPreviousReleaseTests(unittest.TestCase):
    def test_first_release_goes(self) -> None:
        verdict = decide(**base_kwargs(last_release_at=None))
        self.assertTrue(verdict["go"])
        self.assertIn("first release", verdict["reason"])

    def test_first_release_dry_run(self) -> None:
        verdict = decide(**base_kwargs(last_release_at=None, dry_run_input=True))
        self.assertTrue(verdict["go"])
        self.assertTrue(verdict["dry_run"])


class DailyCeilingTests(unittest.TestCase):
    def test_released_yesterday_goes(self) -> None:
        verdict = decide(**base_kwargs(
            last_release_at=moment_ny(2026, 10, 20, 12, 0),
            now=moment_ny(2026, 10, 21, 12, 0),
        ))
        self.assertTrue(verdict["go"])

    def test_released_today_skips(self) -> None:
        verdict = decide(**base_kwargs(
            last_release_at=moment_ny(2026, 10, 21, 8, 0),
            now=moment_ny(2026, 10, 21, 12, 0),
        ))
        self.assertFalse(verdict["go"])
        self.assertIn("already released", verdict["reason"])

    def test_override_without_reason_still_skips(self) -> None:
        verdict = decide(**base_kwargs(
            last_release_at=moment_ny(2026, 10, 21, 8, 0),
            now=moment_ny(2026, 10, 21, 12, 0),
            override_reason="",
        ))
        self.assertFalse(verdict["go"])

    def test_override_with_reason_goes(self) -> None:
        verdict = decide(**base_kwargs(
            last_release_at=moment_ny(2026, 10, 21, 8, 0),
            now=moment_ny(2026, 10, 21, 12, 0),
            override_reason="hotfix: bad tag",
        ))
        self.assertTrue(verdict["go"])
        self.assertIn("hotfix override", verdict["reason"].lower())
        self.assertIn("hotfix: bad tag", verdict["reason"])

    def test_override_whitespace_only_skips(self) -> None:
        verdict = decide(**base_kwargs(
            last_release_at=moment_ny(2026, 10, 21, 8, 0),
            now=moment_ny(2026, 10, 21, 12, 0),
            override_reason="   ",
        ))
        self.assertFalse(verdict["go"])

    def test_late_night_boundary_uses_new_york_dates(self) -> None:
        # 23:30 ET yesterday vs 00:30 ET today share a UTC date but are
        # different New York days, so the train must go.
        last = moment_ny(2026, 10, 20, 23, 30)
        now = moment_ny(2026, 10, 21, 0, 30)
        self.assertEqual(last.astimezone(UTC).date(), now.astimezone(UTC).date())
        self.assertNotEqual(new_york_date(last), new_york_date(now))
        verdict = decide(**base_kwargs(last_release_at=last, now=now))
        self.assertTrue(verdict["go"])

    def test_same_utc_date_same_ny_date_skips(self) -> None:
        last = moment_ny(2026, 10, 21, 0, 30)
        now = moment_ny(2026, 10, 21, 12, 0)
        verdict = decide(**base_kwargs(last_release_at=last, now=now))
        self.assertFalse(verdict["go"])


class DstTransitionTests(unittest.TestCase):
    def test_spring_forward_same_ny_date(self) -> None:
        # 2026-03-08: 02:00 EST jumps to 03:00 EDT. Both instants are Mar 8
        # in New York despite the missing hour.
        before = dt.datetime(2026, 3, 8, 6, 30, tzinfo=UTC)  # 01:30 EST
        after = dt.datetime(2026, 3, 8, 7, 30, tzinfo=UTC)  # 03:30 EDT
        self.assertEqual(new_york_date(before), dt.date(2026, 3, 8))
        self.assertEqual(new_york_date(after), dt.date(2026, 3, 8))
        verdict = decide(**base_kwargs(last_release_at=before, now=after))
        self.assertFalse(verdict["go"])

    def test_spring_forward_next_day_goes(self) -> None:
        last = dt.datetime(2026, 3, 8, 7, 30, tzinfo=UTC)  # Mar 8 EDT
        now = dt.datetime(2026, 3, 9, 12, 0, tzinfo=UTC)  # Mar 9 EDT
        self.assertNotEqual(new_york_date(last), new_york_date(now))
        verdict = decide(**base_kwargs(last_release_at=last, now=now))
        self.assertTrue(verdict["go"])

    def test_fall_back_same_ny_date(self) -> None:
        # 2026-11-01: 02:00 EDT falls back to 01:00 EST. Both instants are
        # Nov 1 in New York despite the repeated hour.
        first = dt.datetime(2026, 11, 1, 4, 30, tzinfo=UTC)  # 00:30 EDT
        second = dt.datetime(2026, 11, 1, 6, 30, tzinfo=UTC)  # 01:30 EST
        self.assertEqual(new_york_date(first), dt.date(2026, 11, 1))
        self.assertEqual(new_york_date(second), dt.date(2026, 11, 1))
        verdict = decide(**base_kwargs(last_release_at=first, now=second))
        self.assertFalse(verdict["go"])

    def test_fall_back_next_day_goes(self) -> None:
        last = dt.datetime(2026, 11, 1, 6, 30, tzinfo=UTC)  # Nov 1 EST
        now = dt.datetime(2026, 11, 2, 5, 30, tzinfo=UTC)  # Nov 2 EST
        self.assertNotEqual(new_york_date(last), new_york_date(now))
        verdict = decide(**base_kwargs(last_release_at=last, now=now))
        self.assertTrue(verdict["go"])


class RefAndTagTests(unittest.TestCase):
    def test_tagged_sha_skips_even_when_armed(self) -> None:
        verdict = decide(**base_kwargs(sha_tagged=True))
        self.assertFalse(verdict["go"])
        self.assertIn("already carries", verdict["reason"])

    def test_non_master_ref_refused(self) -> None:
        verdict = decide(**base_kwargs(ref="refs/heads/ci/release/x"))
        self.assertFalse(verdict["go"])
        self.assertIn("refs/heads/master", verdict["reason"])

    def test_non_master_ref_allowed_in_dry_run(self) -> None:
        verdict = decide(**base_kwargs(ref="refs/heads/ci/release/x", dry_run_input=True))
        self.assertTrue(verdict["go"])
        self.assertTrue(verdict["dry_run"])

    def test_push_event_is_always_dry_run(self) -> None:
        verdict = decide(**base_kwargs(event_name="push", ref="refs/heads/ci/release/x"))
        self.assertTrue(verdict["dry_run"])
        # A push hook with no previous release still reports go (dry run).
        self.assertTrue(verdict["go"])

    def test_override_does_not_bypass_ref_check(self) -> None:
        verdict = decide(**base_kwargs(
            ref="refs/heads/feature",
            override_reason="hotfix",
            last_release_at=moment_ny(2026, 10, 21, 8, 0),
            now=moment_ny(2026, 10, 21, 12, 0),
        ))
        self.assertFalse(verdict["go"])

    def test_override_does_not_bypass_pause(self) -> None:
        verdict = decide(**base_kwargs(
            paused_value="true",
            override_reason="hotfix",
        ))
        self.assertFalse(verdict["go"])

    def test_override_does_not_bypass_tag(self) -> None:
        verdict = decide(**base_kwargs(
            sha_tagged=True,
            override_reason="hotfix",
            last_release_at=moment_ny(2026, 10, 20, 12, 0),
        ))
        self.assertFalse(verdict["go"])


class IndexParsingTests(unittest.TestCase):
    def test_max_pubtime_wins(self) -> None:
        text = "\n".join([
            '{"name":"bob-obsidian","vers":"0.2.0","pubtime":"2026-10-20T12:00:00Z","yanked":false}',
            '{"name":"bob-obsidian","vers":"0.2.1","pubtime":"2026-10-21T08:00:00Z","yanked":false}',
        ])
        stamp = parse_index_pubtimes(text)
        assert stamp is not None
        self.assertEqual(stamp, dt.datetime(2026, 10, 21, 8, 0, tzinfo=UTC))

    def test_yanked_newest_still_counts(self) -> None:
        text = "\n".join([
            '{"name":"bob-obsidian","vers":"0.2.0","pubtime":"2026-10-20T12:00:00Z","yanked":false}',
            '{"name":"bob-obsidian","vers":"0.2.1","pubtime":"2026-10-21T08:00:00Z","yanked":true}',
        ])
        stamp = parse_index_pubtimes(text)
        assert stamp is not None
        self.assertEqual(stamp, dt.datetime(2026, 10, 21, 8, 0, tzinfo=UTC))
        # Yanking never resets the guard: same-day still skips.
        verdict = decide(**base_kwargs(
            mode="live",
            last_release_at=stamp,
            now=moment_ny(2026, 10, 21, 12, 0),
        ))
        self.assertFalse(verdict["go"])

    def test_empty_index_is_no_release(self) -> None:
        self.assertIsNone(parse_index_pubtimes(""))
        self.assertIsNone(parse_index_pubtimes("not json\nstill not json\n"))

    def test_lookup_404_fails_closed(self) -> None:
        verdict = decide(**base_kwargs(
            mode="live",
            lookup_error="could not fetch https://index.crates.io/bo/b-/bob-obsidian: HTTP 404",
            last_release_at=None,
        ))
        self.assertFalse(verdict["go"])
        self.assertTrue(verdict["hard_fail"])

    def test_lookup_network_error_fails_closed(self) -> None:
        verdict = decide(**base_kwargs(
            mode="live",
            lookup_error="could not fetch index: <urlopen error timed out>",
            last_release_at=moment_ny(2026, 10, 20, 12, 0),
        ))
        self.assertFalse(verdict["go"])
        self.assertTrue(verdict["hard_fail"])


class RehearsalParsingTests(unittest.TestCase):
    def test_newest_created_at_wins(self) -> None:
        payload = (
            '[{"tagName":"v0.2.0","createdAt":"2026-10-20T12:00:00Z"},'
            ' {"tagName":"v0.2.1","createdAt":"2026-10-21T08:00:00Z"}]'
        )
        stamp = parse_gh_releases_json(payload)
        assert stamp is not None
        self.assertEqual(stamp, dt.datetime(2026, 10, 21, 8, 0, tzinfo=UTC))

    def test_empty_releases_is_no_release(self) -> None:
        self.assertIsNone(parse_gh_releases_json("[]"))

    def test_drafts_count(self) -> None:
        payload = (
            '[{"tagName":"v0.2.1","createdAt":"2026-10-21T08:00:00Z",'
            '"isDraft":true,"isPrerelease":false}]'
        )
        stamp = parse_gh_releases_json(payload)
        assert stamp is not None
        verdict = decide(**base_kwargs(
            last_release_at=stamp,
            now=moment_ny(2026, 10, 21, 12, 0),
        ))
        self.assertFalse(verdict["go"])


class SparseIndexPathTests(unittest.TestCase):
    def test_one_char(self) -> None:
        self.assertEqual(sparse_index_path("x"), "1/x")

    def test_two_chars(self) -> None:
        self.assertEqual(sparse_index_path("ab"), "2/ab")

    def test_three_chars(self) -> None:
        self.assertEqual(sparse_index_path("abc"), "3/a/abc")

    def test_long_name(self) -> None:
        self.assertEqual(sparse_index_path("bob-obsidian"), "bo/b-/bob-obsidian")
        self.assertEqual(sparse_index_path("serde"), "se/rd/serde")


class ModeTests(unittest.TestCase):
    def _write(self, directory: Path, body: str) -> Path:
        path = directory / "release-plz.toml"
        path.write_text(body)
        return path

    def test_git_only_true_is_rehearsal(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = self._write(Path(temp), "[workspace]\ngit_only = true\n")
            self.assertEqual(read_mode(path), "rehearsal")

    def test_git_only_false_is_live(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = self._write(Path(temp), "[workspace]\ngit_only = false\n")
            self.assertEqual(read_mode(path), "live")

    def test_missing_key_is_live(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = self._write(Path(temp), "[workspace]\n")
            self.assertEqual(read_mode(path), "live")

    def test_missing_file_is_live(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            self.assertEqual(read_mode(Path(temp) / "absent.toml"), "live")


class PinnedVersionTests(unittest.TestCase):
    VERSION_RE = re.compile(r"0\.3\.\d+")

    def _release_plz_versions(self, text: str) -> set[str]:
        return set(self.VERSION_RE.findall(text))

    def test_release_yml_pin_matches_preview(self) -> None:
        workflow = ROOT / ".github" / "workflows" / "release.yml"
        self.assertTrue(workflow.is_file(), "release.yml must exist for the train phase")
        workflow_text = workflow.read_text()
        workflow_versions = self._release_plz_versions(workflow_text)
        self.assertIn("0.3.173", workflow_versions)

        justfile = ROOT / "justfile"
        preview_helper = ROOT / "tools" / "release" / "release_preview.py"
        candidates: list[str] = []
        if justfile.is_file():
            candidates.append(justfile.read_text())
        if preview_helper.is_file():
            candidates.append(preview_helper.read_text())
        self.assertTrue(candidates, "justfile or release_preview.py must pin release-plz")
        for text in candidates:
            with self.subTest(source=text[:40]):
                self.assertIn("0.3.173", self._release_plz_versions(text))


if __name__ == "__main__":
    unittest.main()

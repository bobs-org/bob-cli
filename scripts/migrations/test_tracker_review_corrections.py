#!/usr/bin/env python3
"""Fixture tests for the one-time reference cleanup helper."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from tracker_review_corrections import (
    FROZEN_CUTOFF,
    apply_manifest,
    atomic_write_text,
    build_parser,
    build_preview,
    complete_line,
    parse_date_strict,
    read_text_raw,
    split_text_lines,
)


def make_vault(files: dict[str, str]) -> tempfile.TemporaryDirectory:
    tmp = tempfile.TemporaryDirectory()
    vault = Path(tmp.name)
    for rel, text in files.items():
        target = vault / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8", newline="")
    tmp.vault = vault  # type: ignore[attr-defined]
    return tmp


class CutoffTests(unittest.TestCase):
    def test_only_strictly_older_explicit_dates_complete(self):
        tmp = make_vault(
            {
                "old.md": "- [ ] #task Old [created:: 2026-09-25] ^ref\n",
                "cutoff.md": "- [ ] #task Edge [created:: 2026-09-26] ^ref\n",
                "new.md": "- [ ] #task New [created:: 2026-09-27] ^ref\n",
            }
        )
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            self.assertEqual(
                [(e["path"], e["line"]) for e in manifest["entries"]],
                [("old.md", 1)],
            )
            reasons = {x["path"]: x["reason"] for x in manifest["exceptions"]}
            self.assertEqual(reasons["cutoff.md"], "created-on-or-after-cutoff")
            self.assertEqual(reasons["new.md"], "created-on-or-after-cutoff")
        finally:
            tmp.cleanup()

    def test_invalid_created_is_exception(self):
        tmp = make_vault({"bad.md": "- [ ] #task Bad [created:: soon] ^ref\n"})
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            self.assertEqual(manifest["entries"], [])
            self.assertEqual(manifest["exceptions"][0]["reason"], "invalid-created")
        finally:
            tmp.cleanup()

    def test_missing_created_needs_history(self):
        tmp = make_vault({"nodate.md": "- [ ] #task Plain ^ref\n"})
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            self.assertEqual(manifest["entries"], [])
            self.assertEqual(
                manifest["exceptions"][0]["reason"],
                "missing-created-needs-history",
            )
        finally:
            tmp.cleanup()

    def test_closed_templates_and_lookalikes_excluded(self):
        tmp = make_vault(
            {
                "done.md": "- [x] #task Done [created:: 2020-01-01] [completion:: 2020-01-02] ^ref\n",
                "_templates/t.md": "- [ ] #task T [created:: 2020-01-01] ^ref\n",
                "near.md": "- [ ] #task Near [created:: 2020-01-01] ^ref-extra\n",
            }
        )
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            self.assertEqual(manifest["entries"], [])
            self.assertEqual(manifest["exceptions"], [])
        finally:
            tmp.cleanup()

    def test_completion_format_and_idempotence(self):
        line = "- [ ] #task Old [created:: 2026-09-01] [fresh:: 2026-09-02] ^ref"
        after = complete_line(line, date(2026, 10, 3))
        self.assertIn("- [x]", after)
        self.assertIn("[completion:: 2026-10-03]", after)
        self.assertIn("[fresh:: 2026-09-02]", after)
        self.assertTrue(after.endswith("^ref"))
        tmp = make_vault({"a.md": line + "\n"})
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            report = apply_manifest(tmp.vault, manifest)
            self.assertEqual(len(report["applied"]), 1)
            self.assertEqual(report["stale"], [])
            rerun = build_preview(tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head")
            self.assertEqual(rerun["entries"], [])
        finally:
            tmp.cleanup()

    def test_stale_guard_and_crlf_preserved(self):
        tmp = make_vault(
            {"a.md": "- [ ] #task Old [created:: 2026-09-01] ^ref\r\nline2\r\n"}
        )
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            target = tmp.vault / "a.md"
            target.write_text(
                "- [ ] #task Changed [created:: 2026-09-01] ^ref\r\nline2\r\n",
                encoding="utf-8",
                newline="",
            )
            report = apply_manifest(tmp.vault, manifest)
            self.assertEqual(report["applied"], [])
            self.assertEqual(len(report["stale"]), 1)
        finally:
            tmp.cleanup()

    def test_crlf_apply_does_not_double_carriage_returns(self):
        tmp = make_vault(
            {"a.md": "- [ ] #task Old [created:: 2026-09-01] ^ref\r\nline2\r\n"}
        )
        try:
            lines, newline = split_text_lines(read_text_raw(tmp.vault / "a.md"))
            self.assertEqual(newline, "\r\n")
            self.assertEqual(lines[0].endswith("\r"), False)
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            report = apply_manifest(tmp.vault, manifest)
            self.assertEqual(len(report["applied"]), 1)
            self.assertEqual(report["stale"], [])
            data = (tmp.vault / "a.md").read_bytes()
            self.assertNotIn(b"\r\r", data)
            self.assertTrue(data.endswith(b"\r\n"))
            self.assertIn(b"\r\nline2\r\n", data)
            self.assertIn(b"- [x] #task Old", data)
            self.assertIn(b"[completion:: 2026-10-03]", data)
        finally:
            tmp.cleanup()

    def test_u2028_in_non_target_line_round_trips(self):
        before = "- [ ] #task Old [created:: 2026-09-01] ^ref\nnote with separator\n"
        self.assertIn(" ", before)
        tmp = make_vault({"a.md": before})
        try:
            lines, _newline = split_text_lines(read_text_raw(tmp.vault / "a.md"))
            self.assertEqual(len(lines), 2, lines)
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            self.assertEqual(len(manifest["entries"]), 1)
            report = apply_manifest(tmp.vault, manifest)
            self.assertEqual(len(report["applied"]), 1)
            self.assertEqual(report["stale"], [])
            data = (tmp.vault / "a.md").read_bytes()
            self.assertIn(" ".encode("utf-8"), data)
            self.assertIn(b"- [x] #task Old", data)
            after_lines, _ = split_text_lines(data.decode("utf-8"))
            self.assertEqual(len(after_lines), 2, after_lines)
        finally:
            tmp.cleanup()

    def test_atomic_write_keeps_0644_mode(self):
        import os
        import stat as stat_module

        tmp = make_vault(
            {"a.md": "- [ ] #task Old [created:: 2026-09-01] ^ref\n"}
        )
        try:
            target = tmp.vault / "a.md"
            os.chmod(target, 0o644)
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            report = apply_manifest(tmp.vault, manifest)
            self.assertEqual(len(report["applied"]), 1)
            mode = stat_module.S_IMODE(os.stat(target).st_mode)
            self.assertEqual(oct(mode), oct(0o644))
        finally:
            tmp.cleanup()

    def test_atomic_write_text_copies_mode_directly(self):
        import os
        import stat as stat_module

        tmp = make_vault({"a.md": "before\n"})
        try:
            target = tmp.vault / "a.md"
            os.chmod(target, 0o640)
            atomic_write_text(target, "after\n")
            mode = stat_module.S_IMODE(os.stat(target).st_mode)
            self.assertEqual(oct(mode), oct(0o640))
            self.assertEqual(target.read_text(encoding="utf-8"), "after\n")
        finally:
            tmp.cleanup()

    def test_duplicate_ref_anchor_in_one_file_is_reported(self):
        tmp = make_vault(
            {
                "dup.md": (
                    "- [ ] #task First [created:: 2026-09-01] ^ref\n"
                    "- [ ] #task Second [created:: 2026-09-01] ^ref\n"
                )
            }
        )
        try:
            manifest = build_preview(
                tmp.vault, date(2026, 9, 26), date(2026, 10, 3), "head"
            )
            self.assertTrue(
                any(
                    "duplicate ^ref anchor in one file" in error
                    for error in manifest["scan_errors"]
                ),
                manifest["scan_errors"],
            )
            self.assertEqual(
                [(entry["path"], entry["line"]) for entry in manifest["entries"]],
                [("dup.md", 1)],
            )
        finally:
            tmp.cleanup()

    def test_frozen_cutoff_refuses_expansion(self):
        parser = build_parser()
        self.assertEqual(parser.parse_args(["--cutoff", "2026-09-26"]).cutoff, "2026-09-26")
        self.assertEqual(FROZEN_CUTOFF, "2026-09-26")
        self.assertIsNone(parse_date_strict("Sept 26"))

    def test_options_are_alphabetical_with_short_aliases(self):
        parser = build_parser()
        longs = [action.option_strings[-1] for action in parser._actions if action.option_strings]
        longs = [opt for opt in longs if opt != "--help"]
        self.assertEqual(longs, sorted(longs))
        for action in parser._actions:
            if len(action.option_strings) == 2:
                self.assertTrue(action.option_strings[0].startswith("-"))
                self.assertFalse(action.option_strings[0].startswith("--"))


if __name__ == "__main__":
    unittest.main()

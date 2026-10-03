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
    build_parser,
    build_preview,
    complete_line,
    parse_date_strict,
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

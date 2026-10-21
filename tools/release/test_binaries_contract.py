#!/usr/bin/env python3
"""Stdlib contract between binstall metadata and release-binaries.yml."""

from __future__ import annotations

import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CARGO_TOML = ROOT / "Cargo.toml"
BINARIES_WORKFLOW = ROOT / ".github" / "workflows" / "release-binaries.yml"

BIN_RE = re.compile(r"^\s+bin:\s*[\"']?([^\"'\n]+)[\"']?\s*$", re.MULTILINE)
ARCHIVE_RE = re.compile(r"^\s+archive:\s*[\"']?([^\"'\n]+)[\"']?\s*$", re.MULTILINE)
TARGET_RE = re.compile(r"^\s+target:\s*([A-Za-z0-9_.-]+)\s*$", re.MULTILINE)


class BinariesContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.cargo = tomllib.loads(CARGO_TOML.read_text(encoding="utf-8"))
        cls.workflow = BINARIES_WORKFLOW.read_text(encoding="utf-8")
        binstall = cls.cargo["package"]["metadata"]["binstall"]
        cls.pkg_url = binstall["pkg-url"]
        bins = BIN_RE.search(cls.workflow)
        archive = ARCHIVE_RE.search(cls.workflow)
        if bins is None:
            raise AssertionError("release-binaries.yml has no bin: input")
        if archive is None:
            raise AssertionError("release-binaries.yml has no archive: input")
        cls.action_bins = [part.strip() for part in bins.group(1).split(",") if part.strip()]
        cls.archive = archive.group(1).strip()
        cls.targets = TARGET_RE.findall(cls.workflow)

    def test_pkg_url_does_not_use_bin(self) -> None:
        self.assertNotIn("{ bin }", self.pkg_url)
        self.assertNotIn("{bin}", self.pkg_url)

    def test_archive_does_not_use_dollar_bin(self) -> None:
        self.assertGreater(
            len(self.action_bins),
            1,
            "archive $bin is only forbidden when multiple bins are listed",
        )
        self.assertNotIn("$bin", self.archive)

    def test_action_bins_match_cargo_toml(self) -> None:
        cargo_bins = [entry["name"] for entry in self.cargo.get("bin", [])]
        self.assertEqual(self.action_bins, cargo_bins)

    def test_pkg_url_filename_matches_archive_for_each_target(self) -> None:
        self.assertTrue(self.targets, "release-binaries.yml lists no matrix targets")
        for target in self.targets:
            with self.subTest(target=target):
                rendered_url = self.pkg_url.replace("{ target }", target)
                filename = rendered_url.rsplit("/", 1)[-1]
                rendered_archive = self.archive.replace("$target", target)
                self.assertEqual(filename, f"{rendered_archive}.tar.gz")


if __name__ == "__main__":
    unittest.main()

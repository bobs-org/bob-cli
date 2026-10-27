#!/usr/bin/env python3
"""Offline tests for workspace version and tag-name helpers."""

from __future__ import annotations

import fnmatch
import json
import tempfile
import tomllib
import unittest
from pathlib import Path

from workspace_versions import (
    ASSETS_PACKAGE,
    CHANGELOG_PATHS,
    PACKAGE_ORDER,
    ROOT_PACKAGE,
    commit_subject,
    format_next_versions,
    format_no_release,
    load_release_plz_config,
    load_versions_json,
    main,
    package_git_release_enable,
    package_tag_template,
    read_versions,
    tag_for,
    write_versions_json,
)

ROOT = Path(__file__).resolve().parents[2]
CONFIG = load_release_plz_config(ROOT)


class ReleasePlzConfigTests(unittest.TestCase):
    def test_root_and_assets_use_distinct_tag_templates(self) -> None:
        root_template = package_tag_template(CONFIG, ROOT_PACKAGE)
        assets_template = package_tag_template(CONFIG, ASSETS_PACKAGE)
        self.assertEqual(root_template, "v{{ version }}")
        self.assertEqual(assets_template, "bob-obsidian-assets-v{{ version }}")
        self.assertNotEqual(root_template, assets_template)

    def test_equal_versions_cannot_claim_the_same_tag(self) -> None:
        root_tag = tag_for(ROOT_PACKAGE, "0.1.0", CONFIG)
        assets_tag = tag_for(ASSETS_PACKAGE, "0.1.0", CONFIG)
        self.assertEqual(root_tag, "v0.1.0")
        self.assertEqual(assets_tag, "bob-obsidian-assets-v0.1.0")
        self.assertNotEqual(root_tag, assets_tag)
        self.assertTrue(fnmatch.fnmatch(root_tag, "v*"))
        self.assertFalse(fnmatch.fnmatch(assets_tag, "v*"))

    def test_assets_github_release_is_disabled(self) -> None:
        self.assertTrue(package_git_release_enable(CONFIG, ROOT_PACKAGE))
        self.assertFalse(package_git_release_enable(CONFIG, ASSETS_PACKAGE))

    def test_workspace_keeps_rehearsal_git_only(self) -> None:
        self.assertIs(CONFIG["workspace"]["git_only"], True)
        self.assertNotIn("publish", CONFIG["workspace"])
        for entry in CONFIG["package"]:
            self.assertNotIn("publish", entry)
            self.assertNotIn("release", entry)
            self.assertNotIn("publish_no_verify", entry)

    def test_changelog_paths_cover_both_packages(self) -> None:
        self.assertEqual(
            CHANGELOG_PATHS,
            (
                Path("CHANGELOG.md"),
                Path("crates/bob-obsidian-assets/CHANGELOG.md"),
            ),
        )

    def test_release_workflow_uses_workspace_versions_and_root_binaries(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("tools/release/workspace_versions.py", workflow)
        self.assertIn("crates/bob-obsidian-assets/CHANGELOG.md", workflow)
        self.assertIn("--package bob-obsidian", workflow)
        self.assertNotIn(
            'tomllib.load(open("Cargo.toml","rb"))["package"]["version"]',
            workflow,
        )


class ReadVersionsTests(unittest.TestCase):
    def test_reads_this_checkout(self) -> None:
        cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        assets = tomllib.loads(
            (ROOT / "crates/bob-obsidian-assets/Cargo.toml").read_text(encoding="utf-8")
        )
        versions = read_versions(ROOT)
        self.assertEqual(list(versions), list(PACKAGE_ORDER))
        self.assertEqual(versions[ROOT_PACKAGE], cargo["package"]["version"])
        self.assertEqual(versions[ASSETS_PACKAGE], assets["package"]["version"])


class FormatTests(unittest.TestCase):
    def test_no_release_lists_both_current_tags(self) -> None:
        text = format_no_release(
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"}, CONFIG
        )
        self.assertEqual(
            text,
            "no release: nothing user-visible since v0.4.0, "
            "bob-obsidian-assets-v0.1.0\n",
        )

    def test_next_versions_root_only(self) -> None:
        text = format_next_versions(
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"},
            {ROOT_PACKAGE: "0.4.1", ASSETS_PACKAGE: "0.1.0"},
            CONFIG,
        )
        self.assertEqual(
            text,
            "next versions:\n"
            "- bob-obsidian: 0.4.0 -> 0.4.1 (tag v0.4.1)\n"
            "- bob-obsidian-assets: 0.1.0 (unchanged)\n",
        )

    def test_next_versions_assets_change(self) -> None:
        text = format_next_versions(
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"},
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.1"},
            CONFIG,
        )
        self.assertIn("bob-obsidian: 0.4.0 (unchanged)", text)
        self.assertIn(
            "bob-obsidian-assets: 0.1.0 -> 0.1.1 (tag bob-obsidian-assets-v0.1.1)",
            text,
        )

    def test_commit_subject_root_only(self) -> None:
        subject = commit_subject(
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"},
            {ROOT_PACKAGE: "0.4.1", ASSETS_PACKAGE: "0.1.0"},
            CONFIG,
        )
        self.assertEqual(subject, "chore(release): v0.4.1")

    def test_commit_subject_assets_only(self) -> None:
        subject = commit_subject(
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"},
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.1"},
            CONFIG,
        )
        self.assertEqual(subject, "chore(release): bob-obsidian-assets-v0.1.1")

    def test_commit_subject_mixed(self) -> None:
        subject = commit_subject(
            {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"},
            {ROOT_PACKAGE: "0.4.1", ASSETS_PACKAGE: "0.1.1"},
            CONFIG,
        )
        self.assertEqual(subject, "chore(release): v0.4.1, bob-obsidian-assets-v0.1.1")

    def test_commit_subject_changelog_only(self) -> None:
        versions = {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"}
        self.assertEqual(commit_subject(versions, versions, CONFIG), "chore(release): changelog")


class JsonRoundTripTests(unittest.TestCase):
    def test_write_and_load(self) -> None:
        versions = {ROOT_PACKAGE: "0.4.0", ASSETS_PACKAGE: "0.1.0"}
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "versions.json"
            write_versions_json(path, versions)
            self.assertEqual(json.loads(path.read_text()), versions)
            self.assertEqual(load_versions_json(path), versions)

    def test_main_write_json(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "out.json"
            code = main(["--root", str(ROOT), "--write-json", str(path)])
            self.assertEqual(code, 0)
            data = json.loads(path.read_text())
            self.assertEqual(set(data), {ROOT_PACKAGE, ASSETS_PACKAGE})


if __name__ == "__main__":
    unittest.main()

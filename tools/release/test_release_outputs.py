#!/usr/bin/env python3
"""Offline deterministic tests for release-plz JSON output parsing."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from release_outputs import (
    ReleaseOutputsError,
    format_summary,
    last_json_object,
    list_releases,
    main,
    parse_file,
    select_release,
)

ONE_RELEASE = (
    '{"releases":[{"package_name":"bob-obsidian","prs":[],'
    '"tag":"v0.2.0","version":"0.2.0"}]}'
)
EMPTY_RELEASES = '{"releases":[]}'
OTHER_PACKAGE = (
    '{"releases":[{"package_name":"other-crate","prs":[],'
    '"tag":"v1.0.0","version":"1.0.0"}]}'
)
ASSETS_ONLY = (
    '{"releases":[{"package_name":"bob-obsidian-assets","prs":[],'
    '"tag":"bob-obsidian-assets-v0.1.1","version":"0.1.1"}]}'
)
MIXED_ASSETS_FIRST = (
    '{"releases":['
    '{"package_name":"bob-obsidian-assets","prs":[],'
    '"tag":"bob-obsidian-assets-v0.1.1","version":"0.1.1"},'
    '{"package_name":"bob-obsidian","prs":[],'
    '"tag":"v0.2.0","version":"0.2.0"}'
    "]}"
)


def write_temp(directory: Path, body: str, name: str = "out.json") -> Path:
    path = directory / name
    path.write_text(body)
    return path


class LastJsonObjectTests(unittest.TestCase):
    def test_single_object(self) -> None:
        payload = last_json_object(ONE_RELEASE + "\n")
        self.assertEqual(payload["releases"][0]["tag"], "v0.2.0")

    def test_leading_log_lines(self) -> None:
        text = "\n".join(
            [
                "release-plz: checking packages",
                "info: nothing interesting",
                ONE_RELEASE,
            ]
        )
        payload = last_json_object(text)
        self.assertEqual(payload["releases"][0]["version"], "0.2.0")

    def test_last_object_wins(self) -> None:
        text = "\n".join([EMPTY_RELEASES, ONE_RELEASE])
        payload = last_json_object(text)
        self.assertEqual(payload["releases"][0]["tag"], "v0.2.0")

    def test_missing_object_is_error(self) -> None:
        with self.assertRaises(ReleaseOutputsError):
            last_json_object("no json here\nstill no json\n")

    def test_malformed_object_is_error(self) -> None:
        with self.assertRaises(ReleaseOutputsError):
            last_json_object("{not json\n")


class SelectReleaseTests(unittest.TestCase):
    def test_matching_package(self) -> None:
        tag, version = select_release(json_obj(ONE_RELEASE), "bob-obsidian")
        self.assertEqual(tag, "v0.2.0")
        self.assertEqual(version, "0.2.0")

    def test_empty_releases(self) -> None:
        tag, version = select_release(json_obj(EMPTY_RELEASES), "bob-obsidian")
        self.assertEqual(tag, "")
        self.assertEqual(version, "")

    def test_other_package_only(self) -> None:
        tag, version = select_release(json_obj(OTHER_PACKAGE), "bob-obsidian")
        self.assertEqual(tag, "")
        self.assertEqual(version, "")

    def test_mixed_assets_first_still_selects_root(self) -> None:
        tag, version = select_release(json_obj(MIXED_ASSETS_FIRST), "bob-obsidian")
        self.assertEqual(tag, "v0.2.0")
        self.assertEqual(version, "0.2.0")

    def test_assets_only_is_not_a_binary_release(self) -> None:
        tag, version = select_release(json_obj(ASSETS_ONLY), "bob-obsidian")
        self.assertEqual(tag, "")
        self.assertEqual(version, "")

    def test_missing_releases_key(self) -> None:
        with self.assertRaises(ReleaseOutputsError):
            select_release({"ok": True}, "bob-obsidian")

    def test_non_object_payload(self) -> None:
        with self.assertRaises(ReleaseOutputsError):
            select_release(["releases"], "bob-obsidian")

    def test_matching_package_missing_tag(self) -> None:
        payload = {"releases": [{"package_name": "bob-obsidian", "version": "0.2.0"}]}
        with self.assertRaises(ReleaseOutputsError):
            select_release(payload, "bob-obsidian")


def json_obj(text: str) -> object:
    return last_json_object(text)


class ListReleasesTests(unittest.TestCase):
    def test_mixed_preserves_assets_first_order(self) -> None:
        self.assertEqual(
            list_releases(json_obj(MIXED_ASSETS_FIRST)),
            [
                ("bob-obsidian-assets", "bob-obsidian-assets-v0.1.1", "0.1.1"),
                ("bob-obsidian", "v0.2.0", "0.2.0"),
            ],
        )

    def test_empty_releases(self) -> None:
        self.assertEqual(list_releases(json_obj(EMPTY_RELEASES)), [])

    def test_assets_missing_tag_is_error(self) -> None:
        payload = {
            "releases": [
                {"package_name": "bob-obsidian-assets", "version": "0.1.1"},
            ]
        }
        with self.assertRaises(ReleaseOutputsError):
            list_releases(payload)


class FormatSummaryTests(unittest.TestCase):
    def test_released(self) -> None:
        text = format_summary(tag="v0.2.0", version="0.2.0", mode="rehearsal")
        self.assertIn("### Release: v0.2.0", text)
        self.assertIn("- bob-obsidian: `0.2.0` (tag `v0.2.0`)", text)
        self.assertIn("- binaries: `v0.2.0`", text)
        self.assertIn("- mode: `rehearsal`", text)

    def test_no_release(self) -> None:
        text = format_summary(tag="", version="", mode="rehearsal")
        self.assertEqual(
            text, "No release: release-plz found nothing new to release\n"
        )

    def test_mixed_assets_first_keeps_root_headline_and_binaries(self) -> None:
        text = format_summary(
            tag="v0.2.0",
            version="0.2.0",
            mode="rehearsal",
            releases=list_releases(json_obj(MIXED_ASSETS_FIRST)),
        )
        self.assertIn("### Release: v0.2.0", text)
        self.assertIn(
            "- bob-obsidian-assets: `0.1.1` (tag `bob-obsidian-assets-v0.1.1`)",
            text,
        )
        self.assertIn("- bob-obsidian: `0.2.0` (tag `v0.2.0`)", text)
        self.assertIn("- binaries: `v0.2.0`", text)

    def test_assets_only_reports_package_without_binaries(self) -> None:
        text = format_summary(
            tag="",
            version="",
            mode="rehearsal",
            releases=list_releases(json_obj(ASSETS_ONLY)),
        )
        self.assertIn("### Release: bob-obsidian-assets-v0.1.1", text)
        self.assertIn(
            "- bob-obsidian-assets: `0.1.1` (tag `bob-obsidian-assets-v0.1.1`)",
            text,
        )
        self.assertIn("- binaries: none", text)
        self.assertNotIn("No release:", text)


class ParseFileTests(unittest.TestCase):
    def test_reads_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = write_temp(Path(temp), "log line\n" + ONE_RELEASE + "\n")
            tag, version = parse_file(path, "bob-obsidian")
            self.assertEqual((tag, version), ("v0.2.0", "0.2.0"))

    def test_empty_file_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = write_temp(Path(temp), "")
            with self.assertRaises(ReleaseOutputsError):
                parse_file(path, "bob-obsidian")

    def test_missing_file_is_error(self) -> None:
        with self.assertRaises(ReleaseOutputsError):
            parse_file(Path("/no/such/release-plz.json"), "bob-obsidian")


class MainTests(unittest.TestCase):
    def test_one_release_writes_outputs_and_summary(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            json_path = write_temp(directory, ONE_RELEASE)
            output = directory / "github_output"
            summary = directory / "github_summary"
            env = {
                "GITHUB_OUTPUT": str(output),
                "GITHUB_STEP_SUMMARY": str(summary),
            }
            with patch.dict(os.environ, env, clear=False):
                code = main([str(json_path), "--package", "bob-obsidian", "--mode", "rehearsal"])
            self.assertEqual(code, 0)
            self.assertEqual(output.read_text(), "tag=v0.2.0\nversion=0.2.0\n")
            self.assertIn("### Release: v0.2.0", summary.read_text())
            self.assertIn("- bob-obsidian: `0.2.0` (tag `v0.2.0`)", summary.read_text())
            self.assertIn("- binaries: `v0.2.0`", summary.read_text())
            self.assertIn("- mode: `rehearsal`", summary.read_text())

    def test_empty_releases_clears_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            json_path = write_temp(directory, EMPTY_RELEASES)
            output = directory / "github_output"
            summary = directory / "github_summary"
            env = {
                "GITHUB_OUTPUT": str(output),
                "GITHUB_STEP_SUMMARY": str(summary),
            }
            with patch.dict(os.environ, env, clear=False):
                code = main([str(json_path)])
            self.assertEqual(code, 0)
            self.assertEqual(output.read_text(), "tag=\nversion=\n")
            self.assertEqual(
                summary.read_text(),
                "No release: release-plz found nothing new to release\n",
            )

    def test_leading_logs_then_json(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            json_path = write_temp(
                directory, "checking\n" + ONE_RELEASE + "\n"
            )
            output = directory / "github_output"
            env = {"GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(directory / "s")}
            with patch.dict(os.environ, env, clear=False):
                code = main([str(json_path)])
            self.assertEqual(code, 0)
            self.assertEqual(output.read_text(), "tag=v0.2.0\nversion=0.2.0\n")

    def test_other_package_clears_binary_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            json_path = write_temp(directory, OTHER_PACKAGE)
            output = directory / "github_output"
            summary = directory / "github_summary"
            env = {
                "GITHUB_OUTPUT": str(output),
                "GITHUB_STEP_SUMMARY": str(summary),
            }
            with patch.dict(os.environ, env, clear=False):
                code = main([str(json_path)])
            self.assertEqual(code, 0)
            self.assertEqual(output.read_text(), "tag=\nversion=\n")
            self.assertIn("- other-crate: `1.0.0` (tag `v1.0.0`)", summary.read_text())
            self.assertIn("- binaries: none", summary.read_text())

    def test_mixed_assets_first_writes_root_binary_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            json_path = write_temp(directory, MIXED_ASSETS_FIRST)
            output = directory / "github_output"
            summary = directory / "github_summary"
            env = {
                "GITHUB_OUTPUT": str(output),
                "GITHUB_STEP_SUMMARY": str(summary),
            }
            with patch.dict(os.environ, env, clear=False):
                code = main(
                    [str(json_path), "--package", "bob-obsidian", "--mode", "rehearsal"]
                )
            self.assertEqual(code, 0)
            self.assertEqual(output.read_text(), "tag=v0.2.0\nversion=0.2.0\n")
            body = summary.read_text()
            self.assertIn("### Release: v0.2.0", body)
            self.assertIn(
                "- bob-obsidian-assets: `0.1.1` (tag `bob-obsidian-assets-v0.1.1`)",
                body,
            )
            self.assertIn("- binaries: `v0.2.0`", body)

    def test_assets_only_clears_binary_outputs_and_summarizes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            json_path = write_temp(directory, ASSETS_ONLY)
            output = directory / "github_output"
            summary = directory / "github_summary"
            env = {
                "GITHUB_OUTPUT": str(output),
                "GITHUB_STEP_SUMMARY": str(summary),
            }
            with patch.dict(os.environ, env, clear=False):
                code = main([str(json_path)])
            self.assertEqual(code, 0)
            self.assertEqual(output.read_text(), "tag=\nversion=\n")
            body = summary.read_text()
            self.assertIn("### Release: bob-obsidian-assets-v0.1.1", body)
            self.assertIn("- binaries: none", body)
            self.assertNotIn("No release:", body)

    def test_malformed_output_exits_nonzero(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            json_path = write_temp(Path(temp), "not json at all\n")
            self.assertEqual(main([str(json_path)]), 1)

    def test_missing_file_exits_nonzero(self) -> None:
        self.assertEqual(main(["/no/such/release-plz.json"]), 1)


if __name__ == "__main__":
    unittest.main()

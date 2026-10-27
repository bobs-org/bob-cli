from __future__ import annotations

import io
from pathlib import Path
from unittest import mock
import tarfile
import tempfile
import unittest

from package_check import (
    ASSETS_REQUIRED_NOTICES,
    EXPECTED_PACKAGES,
    FAIL_BYTES,
    REQUIRED_NOTICES,
    ROOT_REQUIRED_NOTICES,
    WARN_BYTES,
    largest_contributors,
    main,
    normalize_package_path,
    package_listing_errors,
    root_include_bytes_errors,
    root_include_bytes_inputs,
    size_level,
    workspace_payload_errors,
)


class PackageCheckTests(unittest.TestCase):
    def test_normalizes_package_list_paths(self) -> None:
        self.assertEqual(normalize_package_path("./src\\lib.rs"), "src/lib.rs")
        self.assertEqual(normalize_package_path("/Cargo.toml"), "Cargo.toml")

    def test_rejects_forbidden_directories_and_missing_notices(self) -> None:
        paths = ["/src/lib.rs", "docs/index.md", "tests/example.rs", *REQUIRED_NOTICES[:-1]]
        errors = package_listing_errors(paths)
        self.assertIn("forbidden package path included: docs/index.md", errors)
        self.assertIn("forbidden package path included: tests/example.rs", errors)
        self.assertIn(
            "required license or notice missing from package: " + REQUIRED_NOTICES[-1],
            errors,
        )

    def test_accepts_required_notices_and_non_forbidden_prefixes(self) -> None:
        paths = [*REQUIRED_NOTICES, "src/docs/reference.rs", "tests-support/README.md"]
        self.assertEqual(package_listing_errors(paths), [])

    def test_root_requires_moment_notice_not_moved_payloads(self) -> None:
        self.assertEqual(
            package_listing_errors(
                [*ROOT_REQUIRED_NOTICES, "src/lib.rs"], ROOT_REQUIRED_NOTICES
            ),
            [],
        )
        errors = package_listing_errors(["LICENSE", "src/lib.rs"], ROOT_REQUIRED_NOTICES)
        self.assertIn(
            "required license or notice missing from package: "
            "src/native/dataview/tasks/vendor/MOMENT_LICENSE",
            errors,
        )

    def test_assets_requires_every_moved_notice(self) -> None:
        self.assertEqual(
            package_listing_errors(
                [*ASSETS_REQUIRED_NOTICES, "src/lib.rs"], ASSETS_REQUIRED_NOTICES
            ),
            [],
        )
        for missing in ASSETS_REQUIRED_NOTICES:
            remaining = [p for p in ASSETS_REQUIRED_NOTICES if p != missing]
            errors = package_listing_errors(remaining, ASSETS_REQUIRED_NOTICES)
            self.assertIn(
                f"required license or notice missing from package: {missing}",
                errors,
            )

    def test_root_archive_rejects_moved_payload_tree(self) -> None:
        paths = [
            *ROOT_REQUIRED_NOTICES,
            "scripts/web_clip/vendor/mermaid.min.js",
        ]
        errors = workspace_payload_errors("bob-obsidian", paths)
        self.assertIn(
            "moved payload tree must live in bob-obsidian-assets: "
            "scripts/web_clip/vendor/mermaid.min.js",
            errors,
        )
        self.assertEqual(
            workspace_payload_errors("bob-obsidian", [*ROOT_REQUIRED_NOTICES]),
            [],
        )

    def test_assets_archive_requires_each_payload_tree(self) -> None:
        full = [
            "assets/print_fonts/Inter-Regular.otf",
            "scripts/web_clip/fonts/DejaVuSans.ttf",
            "scripts/web_clip/vendor/mermaid.min.js",
        ]
        self.assertEqual(workspace_payload_errors("bob-obsidian-assets", full), [])
        thin = ["assets/print_fonts/Inter-Regular.otf"]
        errors = workspace_payload_errors("bob-obsidian-assets", thin)
        self.assertTrue(any("scripts/web_clip/vendor/" in e for e in errors))
        self.assertTrue(any("scripts/web_clip/fonts/" in e for e in errors))

    def test_root_include_bytes_inputs_are_packaged(self) -> None:
        inputs = root_include_bytes_inputs()
        self.assertTrue(inputs, "expected root include_bytes inputs")
        for rel in inputs:
            self.assertNotIn("..", rel)
        # Every discovered input passes when the listing contains it.
        self.assertEqual(root_include_bytes_errors(inputs), [])
        # An empty listing reports each existing input as missing.
        errors = root_include_bytes_errors([])
        self.assertEqual(len(errors), len(inputs))
        self.assertTrue(
            all(e.startswith("root include_bytes input missing") for e in errors)
        )

    def test_size_thresholds_are_strictly_above_the_budget(self) -> None:
        self.assertEqual(size_level(WARN_BYTES), "ok")
        self.assertEqual(size_level(WARN_BYTES + 1), "warning")
        self.assertEqual(size_level(FAIL_BYTES), "warning")
        self.assertEqual(size_level(FAIL_BYTES + 1), "error")

    def test_size_failure_applies_to_either_archive(self) -> None:
        self.assertEqual(size_level(FAIL_BYTES + 1), "error")
        self.assertEqual(size_level(WARN_BYTES + 1), "warning")

    def test_largest_contributors_group_tar_members_by_top_level_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            crate_path = Path(temp) / "sample.crate"
            with tarfile.open(crate_path, mode="w:gz") as archive:
                for name, content in (
                    ("sample-0.1.0/src/lib.rs", b"s" * 30),
                    ("sample-0.1.0/src/main.rs", b"m" * 10),
                    ("sample-0.1.0/assets/font.ttf", b"f" * 50),
                ):
                    member = tarfile.TarInfo(name)
                    member.size = len(content)
                    archive.addfile(member, io.BytesIO(content))
            self.assertEqual(
                largest_contributors(crate_path),
                [("assets", 50), ("src", 40)],
            )

    def test_main_packages_workspace_with_full_verification_by_default(self) -> None:
        commands: list[list[str]] = []

        def fake_run(command: list[str], *, capture: bool = False):
            commands.append(list(command))
            result = mock.Mock()
            if command[:2] == ["cargo", "metadata"]:
                import json

                result.returncode = 0
                result.stdout = json.dumps(
                    {
                        "packages": [
                            {"name": name, "version": "0.0.0", "publish": None}
                            for name in EXPECTED_PACKAGES
                        ],
                        "target_directory": tempfile.gettempdir(),
                    }
                )
                result.stderr = ""
                return result
            if "--list" in command:
                result.returncode = 0
                name = command[-1]
                if name == "bob-obsidian":
                    result.stdout = "\n".join([*ROOT_REQUIRED_NOTICES, "src/lib.rs"])
                else:
                    result.stdout = "\n".join(
                        [
                            *ASSETS_REQUIRED_NOTICES,
                            "assets/print_fonts/Inter-Regular.otf",
                            "scripts/web_clip/fonts/DejaVuSans.ttf",
                            "scripts/web_clip/vendor/mermaid.min.js",
                            "src/lib.rs",
                        ]
                    )
                result.stderr = ""
                return result
            result.returncode = 0
            result.stdout = ""
            result.stderr = ""
            return result

        with tempfile.TemporaryDirectory() as temp:
            package_dir = Path(temp) / "package"
            package_dir.mkdir()
            archives = []
            for name in EXPECTED_PACKAGES:
                crate_path = package_dir / f"{name}-0.0.0.crate"
                with tarfile.open(crate_path, mode="w:gz") as archive:
                    member = tarfile.TarInfo(f"{name}-0.0.0/src/lib.rs")
                    member.size = 3
                    archive.addfile(member, io.BytesIO(b"abc"))
                archives.append(crate_path)
            with (
                mock.patch("package_check._run", side_effect=fake_run),
                mock.patch(
                    "package_check._workspace_packages",
                    return_value={
                        name: ("0.0.0", Path(temp)) for name in EXPECTED_PACKAGES
                    },
                ),
                mock.patch(
                    "package_check.root_include_bytes_inputs", return_value=[]
                ),
            ):
                self.assertEqual(main([]), 0)
                first_commands = [c for c in commands if c[:2] == ["cargo", "package"]]
                self.assertTrue(
                    any("--workspace" in c for c in first_commands),
                    f"expected --workspace packaging, got {first_commands}",
                )
                self.assertFalse(
                    any("--no-verify" in c for c in first_commands),
                    f"full verification must be the default, got {first_commands}",
                )
                list_commands = [c for c in first_commands if "--list" in c]
                self.assertEqual(len(list_commands), len(EXPECTED_PACKAGES))
                for name in EXPECTED_PACKAGES:
                    self.assertTrue(
                        any(c[-1] == name and "-p" in c for c in list_commands),
                        f"expected stable per-package listing for {name}",
                    )
                self.assertFalse(
                    any("--message-format" in c or "json" in c for c in list_commands),
                    "must use stable Cargo output, not unstable JSON",
                )
                commands.clear()
                self.assertEqual(main(["--no-verify"]), 0)
                second_commands = [c for c in commands if c[:2] == ["cargo", "package"]]
                self.assertTrue(any("--no-verify" in c for c in second_commands))

    def test_main_fails_on_missing_expected_package(self) -> None:
        with mock.patch(
            "package_check._workspace_packages",
            return_value={"bob-obsidian": ("0.4.0", Path(tempfile.gettempdir()))},
        ):
            self.assertEqual(main([]), 1)


if __name__ == "__main__":
    unittest.main()

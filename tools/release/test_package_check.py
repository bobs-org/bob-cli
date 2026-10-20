from __future__ import annotations

import io
from pathlib import Path
import tarfile
import tempfile
import unittest

from package_check import (
    FAIL_BYTES,
    REQUIRED_NOTICES,
    WARN_BYTES,
    largest_contributors,
    normalize_package_path,
    package_listing_errors,
    size_level,
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

    def test_size_thresholds_are_strictly_above_the_budget(self) -> None:
        self.assertEqual(size_level(WARN_BYTES), "ok")
        self.assertEqual(size_level(WARN_BYTES + 1), "warning")
        self.assertEqual(size_level(FAIL_BYTES), "warning")
        self.assertEqual(size_level(FAIL_BYTES + 1), "error")

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


if __name__ == "__main__":
    unittest.main()

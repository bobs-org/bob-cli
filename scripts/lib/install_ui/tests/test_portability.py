from __future__ import annotations

import ast
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Optional

PKG = Path(__file__).resolve().parents[1]
LIB = Path(__file__).resolve().parents[2]
REPO = Path(__file__).resolve().parents[4]
LOADER = REPO / "scripts" / "lib" / "install_ui.sh"
INSTALL_ALL = REPO / "scripts" / "install_all"

FUTURE = "from __future__ import annotations"

FORBIDDEN = (
    (re.compile(r"declare\s+-A\b"), "associative arrays"),
    (re.compile(r"\bmapfile\b"), "mapfile"),
    (re.compile(r"\breadarray\b"), "readarray"),
    (re.compile(r"\$\{[^}]+,,\}"), "${x,,}"),
    (re.compile(r"\$\{[^}]+^^\}"), "${x^^}"),
    (re.compile(r"\|\&"), "|&"),
    (re.compile(r";;&"), ";;&"),
    (re.compile(r"\bcoproc\b"), "coproc"),
    (re.compile(r"\[\[\s+-v\b"), "[[ -v"),
    (re.compile(r"declare\s+-n\b"), "namerefs"),
    (re.compile(r"\$\{[^[]+\[-\d+\]\}"), "negative array index"),
    (re.compile(r"\$\{[^}]+@Q\}"), "${x@Q}"),
    (re.compile(r"\bwait\s+-n\b"), "wait -n"),
    (re.compile(r"\bEPOCHSECONDS\b"), "EPOCHSECONDS"),
    (re.compile(r"\bEPOCHREALTIME\b"), "EPOCHREALTIME"),
)


def _code_files() -> list[Path]:
    return sorted(p for p in PKG.rglob("*.py") if p.is_file())


def _first_code_line(src: str) -> str:
    for line in src.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        return stripped
    return ""


def _bash_code_line(line: str) -> str:
    stripped = line.strip()
    if not stripped or stripped.startswith("#"):
        return ""
    if " #" in stripped:
        stripped = stripped.split(" #", 1)[0].rstrip()
    return stripped


class SyntaxPortabilityTests(unittest.TestCase):
    def test_python_39_syntax_and_future_import(self) -> None:
        files = _code_files()
        self.assertTrue(files)
        for path in files:
            src = path.read_text(encoding="utf-8")
            try:
                ast.parse(src, filename=str(path), feature_version=(3, 9))
            except SyntaxError as exc:
                self.fail(f"{path}: not valid Python 3.9 ({exc})")
            self.assertEqual(
                _first_code_line(src),
                FUTURE,
                f"{path} must start with {FUTURE!r}",
            )


class Bash32LintTests(unittest.TestCase):
    def test_forbidden_constructs(self) -> None:
        for path in (INSTALL_ALL, LOADER):
            self.assertTrue(path.is_file(), path)
            for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                code = _bash_code_line(line)
                if not code:
                    continue
                for pattern, label in FORBIDDEN:
                    self.assertIsNone(
                        pattern.search(code),
                        f"{path}:{lineno} uses bash>3.2 {label}: {line}",
                    )


class LoaderTests(unittest.TestCase):
    def _source(self, body: str, env: Optional[dict] = None) -> subprocess.CompletedProcess:
        script = f"set -euo pipefail\nsource '{LOADER}'\n{body}\n"
        merged = os.environ.copy()
        if env:
            merged.update(env)
        return subprocess.run(
            ["bash", "-c", script],
            capture_output=True,
            text=True,
            env=merged,
        )

    def test_discovers_and_caches_python(self) -> None:
        env = os.environ.copy()
        env.pop("BOB_INSTALL_UI_PYTHON", None)
        env.pop("BOB_INSTALL_UI", None)
        proc = self._source(
            'install_ui_python >/dev/null\n'
            'printf "path=%s\\ncache=%s\\n" "$BOB_INSTALL_UI_PYTHON" "$BOB_INSTALL_UI_PYTHON"\n',
            env=env,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        lines = dict(
            line.split("=", 1) for line in proc.stdout.strip().splitlines() if "=" in line
        )
        self.assertTrue(lines["path"])
        self.assertEqual(lines["path"], lines["cache"])

    def test_bob_install_ui_zero_skips(self) -> None:
        proc = self._source("install_ui_python", env={"BOB_INSTALL_UI": "0"})
        self.assertEqual(proc.returncode, 1)

    def test_override_too_old_is_unavailable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "old-python"
            fake.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
            fake.chmod(0o755)
            proc = self._source(
                "install_ui_python",
                env={"BOB_INSTALL_UI_PYTHON": str(fake), "BOB_INSTALL_UI": "1"},
            )
            self.assertEqual(proc.returncode, 1)

    def test_install_ui_runs_helper(self) -> None:
        env = os.environ.copy()
        env.pop("BOB_INSTALL_UI", None)
        proc = self._source("install_ui --help >/dev/null", env=env)
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_mark_installed_silent_on_failure(self) -> None:
        env = os.environ.copy()
        env.pop("BOB_INSTALL_UI", None)
        proc = self._source(
            'install_ui_mark_installed bob-cli "/no/such/checkout"\nprintf ok\n',
            env=env,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "ok")
        self.assertEqual(proc.stderr, "")


if __name__ == "__main__":
    unittest.main()

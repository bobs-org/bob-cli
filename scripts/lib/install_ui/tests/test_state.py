from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

from install_ui import state

LIB = Path(__file__).resolve().parents[2]
PKG = Path(__file__).resolve().parents[1]


def _git_env() -> dict:
    env = os.environ.copy()
    env.update(
        {
            "GIT_AUTHOR_NAME": "install-ui-test",
            "GIT_AUTHOR_EMAIL": "install-ui@test",
            "GIT_COMMITTER_NAME": "install-ui-test",
            "GIT_COMMITTER_EMAIL": "install-ui@test",
            "GIT_TERMINAL_PROMPT": "0",
        }
    )
    return env


def _run_git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        env=_git_env(),
    )


def _init_repo(path: Path, *, dirty: bool = False) -> str:
    path.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "init"],
        cwd=path,
        check=True,
        capture_output=True,
        text=True,
        env=_git_env(),
    )
    _run_git(path, "config", "user.email", "install-ui@test")
    _run_git(path, "config", "user.name", "install-ui-test")
    (path / "file.txt").write_text("hello\n", encoding="utf-8")
    _run_git(path, "add", "file.txt")
    _run_git(path, "commit", "-m", "init")
    sha = _run_git(path, "rev-parse", "HEAD").stdout.strip()
    if dirty:
        (path / "file.txt").write_text("dirty\n", encoding="utf-8")
    return sha


class _StateDirCase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.state_root = Path(self._tmp.name) / "install-all"
        self.state_root.mkdir()
        self._env = mock.patch.dict(
            os.environ, {"BOB_INSTALL_STATE_DIR": str(self.state_root)}
        )
        self._env.start()

    def tearDown(self) -> None:
        self._env.stop()
        self._tmp.cleanup()


class WatermarkTests(_StateDirCase):
    def test_roundtrip(self) -> None:
        clock = lambda: datetime(2026, 11, 6, 13, 24, 51, tzinfo=timezone.utc)
        written = state.write_watermark(
            "bob-cli",
            {
                "sha": "a" * 40,
                "dirty": False,
                "path": "/tmp/bob-cli",
            },
            clock=clock,
        )
        self.assertTrue(written.ok, written.error)
        got = state.get_watermark("bob-cli")
        self.assertTrue(got.ok)
        record = got.value
        self.assertEqual(record["sha"], "a" * 40)
        self.assertEqual(record["dirty"], False)
        self.assertEqual(record["path"], "/tmp/bob-cli")
        self.assertTrue(record["at"].startswith("2026-11-06T13:24:51"))
        path = self.state_root / "installed.json"
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_corrupt_installed_json_is_empty(self) -> None:
        (self.state_root / "installed.json").write_text("{not json", encoding="utf-8")
        data = state.read_installed()
        self.assertEqual(data["repos"], {})
        missing = state.get_watermark("bob-cli")
        self.assertFalse(missing.ok)

    def test_unwritable_dir(self) -> None:
        blocker = Path(self._tmp.name) / "blocked"
        blocker.write_text("not a directory", encoding="utf-8")
        with mock.patch.dict(os.environ, {"BOB_INSTALL_STATE_DIR": str(blocker)}):
            result = state.write_watermark(
                "bob-cli",
                {"sha": "a" * 40, "dirty": False, "path": "/tmp"},
            )
        self.assertFalse(result.ok)
        self.assertIn("state unavailable", result.error)

    def test_snapshot_and_cli_mark_installed(self) -> None:
        repo = Path(self._tmp.name) / "repo"
        sha = _init_repo(repo)
        snap = state.snapshot_repo(str(repo))
        self.assertTrue(snap.ok, snap.error)
        self.assertEqual(snap.value["sha"], sha)
        self.assertFalse(snap.value["dirty"])

        env = os.environ.copy()
        env["NO_COLOR"] = "1"
        proc = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "mark-installed",
                "--repo",
                "bob-cli",
                "--dir",
                str(repo),
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        water = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "watermark",
                "--repo",
                "bob-cli",
                "--json",
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(water.returncode, 0, water.stderr)
        payload = json.loads(water.stdout)
        self.assertEqual(payload["sha"], sha)
        self.assertFalse(payload["dirty"])
        self.assertEqual(payload["path"], str(repo.resolve()))

        (repo / "file.txt").write_text("changed\n", encoding="utf-8")
        proc = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "mark-installed",
                "--repo",
                "bob-cli",
                "--dir",
                str(repo),
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        water = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "watermark",
                "--repo",
                "bob-cli",
                "--json",
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertTrue(json.loads(water.stdout)["dirty"])

    def test_mark_installed_always_exits_zero(self) -> None:
        env = os.environ.copy()
        env["NO_COLOR"] = "1"
        proc = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "mark-installed",
                "--repo",
                "bob-cli",
                "--dir",
                str(Path(self._tmp.name) / "missing"),
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(proc.returncode, 0)
        self.assertTrue(proc.stderr.strip())

    def test_watermark_missing_exits_one(self) -> None:
        env = os.environ.copy()
        env["NO_COLOR"] = "1"
        proc = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "watermark",
                "--repo",
                "nope",
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(proc.returncode, 1)
        self.assertIn("no watermark", proc.stderr)

    def test_lock_contention_two_repos(self) -> None:
        worker = r"""
import os, sys
sys.path.insert(0, sys.argv[1])
from install_ui import state
os.environ["BOB_INSTALL_STATE_DIR"] = sys.argv[2]
name = sys.argv[3]
result = state.write_watermark(
    name, {"sha": name[0] * 40, "dirty": False, "path": "/tmp/" + name}
)
sys.exit(0 if result.ok else 1)
"""
        procs = [
            subprocess.Popen(
                [sys.executable, "-B", "-c", worker, str(LIB), str(self.state_root), name],
            )
            for name in ("bob-cli", "bob-plugins")
        ]
        codes = [p.wait() for p in procs]
        self.assertEqual(codes, [0, 0])
        data = state.read_installed()
        self.assertIn("bob-cli", data["repos"])
        self.assertIn("bob-plugins", data["repos"])
        json.loads((self.state_root / "installed.json").read_text(encoding="utf-8"))


class HistoryTests(_StateDirCase):
    def test_concurrent_appends(self) -> None:
        worker = r"""
import os, sys
sys.path.insert(0, sys.argv[1])
from install_ui import state
os.environ["BOB_INSTALL_STATE_DIR"] = sys.argv[2]
tag = sys.argv[3]
n = int(sys.argv[4])
for i in range(n):
    result = state.append_history({"v": 1, "tag": tag, "i": i})
    if not result.ok:
        sys.stderr.write(result.error + "\n")
        sys.exit(1)
"""
        procs = [
            subprocess.Popen(
                [sys.executable, "-B", "-c", worker, str(LIB), str(self.state_root), tag, "40"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            for tag in ("a", "b")
        ]
        for proc in procs:
            stdout, stderr = proc.communicate()
            self.assertEqual(proc.returncode, 0, stderr)
        rows = state.read_history()
        self.assertTrue(rows.ok, rows.error)
        values = rows.value
        self.assertEqual(len(values), 80)
        tags = {(row["tag"], row["i"]) for row in values}
        expected = {(tag, i) for tag in ("a", "b") for i in range(40)}
        self.assertEqual(tags, expected)

    def test_malformed_lines_skipped(self) -> None:
        path = self.state_root / "runs.jsonl"
        path.write_bytes(b"{nope}\n{\"v\":1,\"ok\":true}\n\n")
        rows = state.read_history()
        self.assertTrue(rows.ok)
        self.assertEqual(rows.value, [{"v": 1, "ok": True}])

    def test_trim_to_newest_1500(self) -> None:
        path = self.state_root / "runs.jsonl"
        lines = [json.dumps({"i": i}) + "\n" for i in range(2000)]
        path.write_text("".join(lines), encoding="utf-8")
        result = state.append_history({"i": 2000})
        self.assertTrue(result.ok, result.error)
        rows = state.read_history()
        self.assertEqual(len(rows.value), 1500)
        self.assertEqual(rows.value[0]["i"], 501)
        self.assertEqual(rows.value[-1]["i"], 2000)


class LogAndCrashTests(_StateDirCase):
    def test_log_prune_keeps_newest_ten_per_step(self) -> None:
        logs = self.state_root / "logs"
        logs.mkdir()
        for i in range(12):
            p = logs / f"20261106-14040{i:02d}-1-bob-cli.log"
            p.write_text("x", encoding="utf-8")
            os.utime(p, (i, i))
        allocated = state.allocate_log("20261106-140499-9", "bob-cli")
        self.assertTrue(allocated.ok, allocated.error)
        remaining = sorted(p.name for p in logs.glob("*-bob-cli.log"))
        self.assertEqual(len(remaining), 10)
        self.assertIn("20261106-140499-9-bob-cli.log", remaining)
        self.assertNotIn("20261106-140400-1-bob-cli.log", remaining)

    def test_run_dir_prune(self) -> None:
        runs = self.state_root / "runs"
        runs.mkdir()
        for i in range(12):
            d = runs / f"run-{i:02d}"
            d.mkdir()
            os.utime(d, (i, i))
        result = state.allocate_run_dir("run-new")
        self.assertTrue(result.ok, result.error)
        names = sorted(p.name for p in runs.iterdir())
        self.assertEqual(len(names), 10)
        self.assertIn("run-new", names)

    def test_crash_log_trim(self) -> None:
        big = "x" * (state.CRASH_LOG_MAX + 50)
        result = state.append_crash(big)
        self.assertTrue(result.ok, result.error)
        size = (self.state_root / "crash.log").stat().st_size
        self.assertLessEqual(size, state.CRASH_LOG_MAX)

    def test_internal_error_exit_70(self) -> None:
        env = os.environ.copy()
        env["NO_COLOR"] = "1"
        env["BOB_INSTALL_STATE_DIR"] = str(self.state_root)
        proc = subprocess.run(
            [
                sys.executable,
                "-B",
                "-c",
                "import os, sys\n"
                "sys.path.insert(0, sys.argv[1])\n"
                "os.environ['BOB_INSTALL_STATE_DIR'] = sys.argv[2]\n"
                "from install_ui.__main__ import _crash\n"
                "try:\n"
                "    raise RuntimeError('boom')\n"
                "except RuntimeError:\n"
                "    sys.exit(_crash())\n",
                str(LIB),
                str(self.state_root),
            ],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(proc.returncode, 70)
        self.assertIn("internal error", proc.stderr)
        crash = (self.state_root / "crash.log").read_text(encoding="utf-8")
        self.assertIn("RuntimeError: boom", crash)


class StateDirResolutionTests(unittest.TestCase):
    def test_override_xdg_and_home(self) -> None:
        with mock.patch.dict(
            os.environ, {"BOB_INSTALL_STATE_DIR": "/tmp/custom-install-ui"}, clear=False
        ):
            self.assertEqual(state.state_dir(), Path("/tmp/custom-install-ui"))
        env = os.environ.copy()
        env.pop("BOB_INSTALL_STATE_DIR", None)
        env["XDG_STATE_HOME"] = "/tmp/xdg"
        with mock.patch.dict(os.environ, env, clear=True):
            os.environ.pop("BOB_INSTALL_STATE_DIR", None)
            self.assertEqual(
                state.state_dir(), Path("/tmp/xdg/bob-cli/install-all")
            )
        env.pop("XDG_STATE_HOME", None)
        with mock.patch.dict(os.environ, env, clear=True):
            os.environ.pop("BOB_INSTALL_STATE_DIR", None)
            os.environ.pop("XDG_STATE_HOME", None)
            with mock.patch.object(Path, "home", return_value=Path("/home/tester")):
                self.assertEqual(
                    state.state_dir(),
                    Path("/home/tester/.local/state/bob-cli/install-all"),
                )


if __name__ == "__main__":
    unittest.main()

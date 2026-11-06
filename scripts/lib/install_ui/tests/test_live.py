from __future__ import annotations

import errno
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
from pathlib import Path
from unittest import mock

from install_ui import estimate, history, parsers, state, term

PKG = Path(__file__).resolve().parents[1]


def _git(repo: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "GIT_AUTHOR_NAME": "install-ui-test",
            "GIT_AUTHOR_EMAIL": "install-ui@test",
            "GIT_COMMITTER_NAME": "install-ui-test",
            "GIT_COMMITTER_EMAIL": "install-ui@test",
            "GIT_CONFIG_NOSYSTEM": "1",
        },
    )
    return proc.stdout.strip()


def _repo(path: Path) -> str:
    path.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init", str(path)], check=True, capture_output=True)
    _git(path, "config", "user.name", "install-ui-test")
    _git(path, "config", "user.email", "install-ui@test")
    (path / "Cargo.toml").write_text("[workspace]\nmembers=[]\n", encoding="utf-8")
    (path / "Cargo.lock").write_text("version = 3\n", encoding="utf-8")
    (path / "src.rs").write_text("fn main() {}\n", encoding="utf-8")
    _git(path, "add", ".")
    _git(path, "commit", "-m", "test: initial")
    (path / "target" / "local-install").mkdir(parents=True)
    return _git(path, "rev-parse", "HEAD")


class ParserTests(unittest.TestCase):
    def test_cargo_phases_chatter_and_lock_wait(self) -> None:
        parser = parsers.PhaseDetector("cargo", "bob-obsidian")
        cases = (
            ("Updating crates.io index", True, "index"),
            ("Compiling serde v1.0.0", True, "deps"),
            ("Compiling bob-obsidian v0.1.0", True, "crate"),
            ("Building [=====>    ] 253/258: bob-obsidian", True, "crate"),
            ("Finished `local-install` profile [optimized]", True, "finish"),
            ("Blocking waiting for file lock on build directory", True, None),
            ("warning: a useful warning", False, None),
        )
        for line, chatter, phase in cases:
            with self.subTest(line=line):
                parsed = parser.parse(line)
                self.assertEqual(parsed.chatter, chatter)
                self.assertEqual(parsed.phase, phase)
        self.assertTrue(parser.parse(cases[-2][0]).lock_wait)
        self.assertEqual(parsers.progress_counter("Building [====] 12/40: crate"), (12, 40))

    def test_swiftpm_two_builds_then_install(self) -> None:
        parser = parsers.PhaseDetector("swiftpm")
        self.assertEqual(parser.parse("Building for production... ").phase, "app")
        self.assertEqual(parser.parse("Build complete! (2.1s)").phase, "app")
        self.assertEqual(parser.parse("Building for production... ").phase, "helper")
        self.assertEqual(parser.parse("Build complete! (1.1s)").phase, "install")
        self.assertTrue(parser.parse("/tmp/Bob Mac Capture.app").chatter)


class HistoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.repo_path = self.root / "repo"
        self.sha = _repo(self.repo_path)
        self.state_path = self.root / "state"
        self.env = mock.patch.dict(
            os.environ,
            {
                "BOB_INSTALL_STATE_DIR": str(self.state_path),
                "CARGO_TARGET_DIR": str(self.repo_path / "target"),
            },
        )
        self.env.start()

    def tearDown(self) -> None:
        self.env.stop()
        self.tmp.cleanup()

    def test_kind_prediction_uses_watermark_and_dependency_changes(self) -> None:
        state.write_watermark(
            "bob-cli",
            {"sha": self.sha, "dirty": False, "path": str(self.repo_path)},
        )
        cargo_home = self.root / "cargo"
        (cargo_home / "bin").mkdir(parents=True)
        (cargo_home / "bin" / "bob").touch()
        with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}):
            self.assertEqual(history.predict_kind("bob-cli", str(self.repo_path)), "noop")
        (self.repo_path / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
        self.assertEqual(history.predict_kind("bob-cli", str(self.repo_path)), "deps")
        (self.repo_path / "Cargo.lock").write_text("version = 3\n", encoding="utf-8")
        (self.repo_path / "src.rs").write_text("fn main() { println!(\"x\"); }\n", encoding="utf-8")
        self.assertEqual(history.predict_kind("bob-cli", str(self.repo_path)), "rebuild")
        (self.repo_path / "target" / "local-install").rmdir()
        self.assertEqual(history.predict_kind("bob-cli", str(self.repo_path)), "cold")

    def test_record_and_learning_rung(self) -> None:
        key = {
            "host": history.host_name(),
            "toolchain": "rustc test",
            "profile": "local-install",
            "incremental": True,
        }
        self.assertEqual(
            estimate.estimate("bob-cli", "rebuild", key, 0, "crate").text,
            "first timed build on " + term.host_label(),
        )
        for outcome, slept in (("fail", False), ("ok", True)):
            row = history.make_row(
                step="bob-cli",
                phase="crate",
                secs=999,
                outcome=outcome,
                kind="rebuild",
                toolchain=key["toolchain"],
                profile=key["profile"],
                incremental=key["incremental"],
                run=f"ignored-{outcome}-{slept}",
                slept=slept,
            )
            row["host"] = key["host"]
            self.assertTrue(state.append_history(row).ok)
        self.assertEqual(
            estimate.estimate("bob-cli", "rebuild", key, 0, "crate").text,
            "first timed build on " + term.host_label(),
        )
        for phase, secs in (("deps", 30), ("crate", 90)):
            row = history.make_row(
                step="bob-cli",
                phase=phase,
                secs=secs,
                outcome="ok",
                kind="rebuild",
                toolchain=key["toolchain"],
                profile=key["profile"],
                incremental=key["incremental"],
                run="matching-run",
            )
            row["host"] = key["host"]
            self.assertTrue(state.append_history(row).ok)
        self.assertEqual(
            estimate.estimate("bob-cli", "rebuild", key, 0, "crate").text,
            "last similar build 2m 00s",
        )

    def test_record_and_kind_cli_commands(self) -> None:
        record = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "record",
                "--step",
                "bob-cli",
                "--phase",
                "plugin-sync",
                "--secs",
                "12.5",
                "--outcome",
                "ok",
                "--kind",
                "rebuild",
                "--toolchain",
                "rustc test",
                "--profile",
                "local-install",
                "--incremental",
                "on",
            ],
            capture_output=True,
            text=True,
            env=os.environ.copy(),
        )
        self.assertEqual(record.returncode, 0, record.stderr)
        row = state.read_history().value[0]
        self.assertEqual(row["phase"], "plugin-sync")
        self.assertEqual(row["secs"], 12.5)
        self.assertTrue(row["incremental"])
        kind = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "kind",
                "--step",
                "bob-cli",
                "--repo",
                str(self.repo_path),
            ],
            capture_output=True,
            text=True,
            env=os.environ.copy(),
        )
        self.assertEqual(kind.returncode, 0, kind.stderr)
        self.assertIn(kind.stdout.strip(), {"cold", "deps", "rebuild", "noop"})


class RunnerPtyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.repo_path = self.root / "repo"
        _repo(self.repo_path)
        self.state_path = self.root / "state"
        self.env = os.environ.copy()
        self.env.update(
            {
                "BOB_INSTALL_STATE_DIR": str(self.state_path),
                "CARGO_TARGET_DIR": str(self.repo_path / "target"),
                "TERM": "xterm",
                "COLUMNS": "38",
                "LINES": "20",
                "PYTHONUNBUFFERED": "1",
            }
        )
        self.parent_env = mock.patch.dict(os.environ, self.env)
        self.parent_env.start()
        self.env.pop("NO_COLOR", None)

    def tearDown(self) -> None:
        self.parent_env.stop()
        self.tmp.cleanup()

    def _start(
        self,
        child_code: str,
        *,
        outer_code: str = "",
        step: str = "bob-cli",
        parser: str = "cargo",
    ):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 38, 0, 0))
        started = self.root / "started"
        rows_file = self.root / "rows"
        command = [
            sys.executable,
            "-B",
            str(PKG),
            "run",
            "--step",
            step,
            "--parser",
            parser,
            "--repo",
            str(self.repo_path),
            "--started-file",
            str(started),
            "--rows-file",
            str(rows_file),
            "--",
            sys.executable,
            "-c",
            child_code,
        ]
        if outer_code:
            command = [
                sys.executable,
                "-c",
                outer_code,
                *command[1:],
            ]
        proc = subprocess.Popen(
            command,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=self.env,
            start_new_session=True,
            close_fds=True,
        )
        os.close(slave)
        return proc, master, started, rows_file

    def _collect(self, proc, master, timeout: float = 8.0) -> bytes:
        output = bytearray()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError as exc:
                    if exc.errno == errno.EIO:
                        if proc.poll() is not None:
                            break
                        time.sleep(0.01)
                        continue
                    raise
                if not chunk:
                    break
                output.extend(chunk)
            if proc.poll() is not None and not ready:
                break
        if proc.poll() is None:
            proc.kill()
            proc.wait()
            self.fail("runner did not finish: " + output.decode("utf-8", errors="replace"))
        return bytes(output)

    def test_records_all_phases_replays_warnings_and_restores_cursor(self) -> None:
        code = (
            "import time; "
            "print('Updating crates.io index', flush=True); "
            "print('Compiling serde v1.0.0', flush=True); "
            "print('warning: keep this visible', flush=True); "
            "print('Compiling bob-obsidian v0.1.0', flush=True); "
            "print('Building [=====> ] 12/40: bob-obsidian', flush=True); "
            "print('Finished `local-install` profile', flush=True); "
            "print('Installing /tmp/bob', flush=True); time.sleep(0.15)"
        )
        proc, master, started, rows_file = self._start(code)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 0)
        self.assertTrue(started.exists())
        self.assertIn("warning: keep this visible", output)
        self.assertIn("✓ built", output)
        self.assertIn("\033[?25h", output)
        frames = [part for part in output.split("\r") if any(ch in part for ch in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")]
        self.assertTrue(frames)
        for frame in frames:
            visible = frame.split("\033[K", 1)[-1].split("\n", 1)[0]
            self.assertLessEqual(term.cell_width(visible), 37)
        self.assertGreaterEqual(int(rows_file.read_text().strip()), 1)
        history_rows = state.read_history().value
        self.assertEqual({row["phase"] for row in history_rows}, {"index", "deps", "crate", "finish"})
        for row in history_rows:
            self.assertEqual(row["outcome"], "ok")
            self.assertIn("waited", row)
            self.assertIn("toolchain", row)
        logs = list((self.state_path / "logs").glob("*.log"))
        self.assertEqual(len(logs), 1)
        raw_log = logs[0].read_text(encoding="utf-8")
        self.assertIn("warning: keep this visible", raw_log)
        self.assertEqual(raw_log.count("Compiling serde"), 1)

    def test_failure_preserves_child_status_and_excerpt(self) -> None:
        code = "import sys; print('error: broken build', flush=True); sys.exit(101)"
        proc, master, _, _ = self._start(code)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 101)
        self.assertIn("build failed", output)
        self.assertIn("error: broken build", output)

        proc, master, _, _ = self._start("raise SystemExit(72)")
        self._collect(proc, master)
        os.close(master)
        self.assertEqual(proc.returncode, 72)

    def test_swiftpm_runner_tracks_app_helper_and_install(self) -> None:
        code = (
            "print('Building for production...', flush=True); "
            "print('[1/2] Compiling app', flush=True); "
            "print('Build complete!', flush=True); "
            "print('Building for production...', flush=True); "
            "print('Build complete!', flush=True); "
            "print('Signing the app', flush=True)"
        )
        proc, master, _, _ = self._start(
            code,
            parser="swiftpm",
            step="bob-mac-capture",
        )
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 0)
        self.assertIn("Signing the app", output)
        rows = state.read_history().value
        self.assertEqual({row["phase"] for row in rows}, {"app", "helper", "install"})

    def test_lock_wait_is_shown_and_removed_from_phase_training_time(self) -> None:
        code = (
            "import time; print('Compiling dependency v1', flush=True); "
            "print('Blocking waiting for file lock on build directory', flush=True); "
            "time.sleep(0.25); print('Compiling bob-obsidian v0.1.0', flush=True)"
        )
        proc, master, _, _ = self._start(code)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 0)
        self.assertIn("waiting for another build to relea…", output)
        rows = {row["phase"]: row for row in state.read_history().value}
        self.assertGreater(rows["deps"]["waited"], 0.1)
        self.assertLess(rows["deps"]["secs"], 0.2)

    def test_100k_chatter_lines_and_three_seconds_of_silence(self) -> None:
        code = (
            "import sys,time; sys.stdout.write('Compiling dependency v1\\n'*100000); "
            "sys.stdout.flush(); time.sleep(3)"
        )
        proc, master, _, _ = self._start(code)
        output = self._collect(proc, master, timeout=12).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 0)
        frames = [part for part in output.split("\r") if any(ch in part for ch in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")]
        self.assertGreaterEqual(len(frames), 10)
        self.assertIn("✓ built", output)

    def test_sigterm_is_forwarded_and_reraised(self) -> None:
        pid_file = self.root / "term-child-pid"
        code = f"import os,time; open({str(pid_file)!r}, 'w').write(str(os.getpid())); time.sleep(10)"
        proc, master, _, _ = self._start(code)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not pid_file.exists():
            time.sleep(0.02)
        self.assertTrue(pid_file.exists())
        os.kill(proc.pid, signal.SIGTERM)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, -signal.SIGTERM)
        self.assertIn("interrupted after", output)
        self.assertTrue(all(row["outcome"] == "interrupted" for row in state.read_history().value))

    def test_non_tty_is_raw_passthrough_without_history(self) -> None:
        started = self.root / "non-tty-started"
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(PKG),
                "run",
                "--step",
                "bob-cli",
                "--parser",
                "cargo",
                "--repo",
                str(self.repo_path),
                "--started-file",
                str(started),
                "--",
                sys.executable,
                "-c",
                "import sys; print('raw stream'); sys.exit(23)",
            ],
            capture_output=True,
            text=True,
            env=self.env,
        )
        self.assertEqual(result.returncode, 23)
        self.assertEqual(result.stdout, "raw stream\n")
        self.assertTrue(started.exists())
        self.assertFalse((self.state_path / "runs.jsonl").exists())

    def test_spawn_failure_does_not_touch_started_file(self) -> None:
        started = self.root / "not-started"
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 38, 0, 0))
        proc = subprocess.Popen(
            [
                sys.executable,
                "-B",
                str(PKG),
                "run",
                "--step",
                "bob-cli",
                "--parser",
                "cargo",
                "--repo",
                str(self.repo_path),
                "--started-file",
                str(started),
                "--",
                str(self.root / "missing-command"),
            ],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=self.env,
            start_new_session=True,
            close_fds=True,
        )
        os.close(slave)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 1)
        self.assertFalse(started.exists())
        self.assertIn("could not start build", output)

    def test_renderer_failure_switches_to_passthrough_and_keeps_status(self) -> None:
        inner = (
            "import sys; sys.path.insert(0, sys.argv[1]); "
            "from install_ui import live; import argparse; "
            "live.term.terminal_size=lambda *a, **k: (_ for _ in ()).throw(OSError('render')); "
            "a=argparse.Namespace(command=sys.argv[2:5], parser='cargo', repo=sys.argv[5], "
            "step='bob-cli', profile=None, crate=None, done='built', started_file=sys.argv[6], "
            "rows_file=sys.argv[7]); raise SystemExit(live.run(a))"
        )
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 38, 0, 0))
        started = self.root / "started-fault"
        rows_file = self.root / "rows-fault"
        proc = subprocess.Popen(
            [
                sys.executable,
                "-c",
                inner,
                str(PKG.parent),
                sys.executable,
                "-c",
                "import sys; print('raw passthrough', flush=True); sys.exit(37)",
                str(self.repo_path),
                str(started),
                str(rows_file),
            ],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=self.env,
            start_new_session=True,
            close_fds=True,
        )
        os.close(slave)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 37, output)
        self.assertIn("raw passthrough", output)
        self.assertTrue(started.exists())

    def test_renderer_uses_an_injectable_monotonic_clock(self) -> None:
        outer = (
            "import sys; sys.path.insert(0, sys.argv[1]); "
            "from install_ui import live; import argparse; "
            "ticks=iter(range(0, 100000, 10)); clock=lambda: next(ticks); "
            "a=argparse.Namespace(command=sys.argv[2:5], parser='cargo', repo=sys.argv[5], "
            "step='bob-cli', profile=None, crate=None, done='built', started_file=sys.argv[6], "
            "rows_file=sys.argv[7], clock=clock); raise SystemExit(live.run(a))"
        )
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 38, 0, 0))
        started = self.root / "started-clock"
        rows_file = self.root / "rows-clock"
        proc = subprocess.Popen(
            [
                sys.executable,
                "-c",
                outer,
                str(PKG.parent),
                sys.executable,
                "-c",
                "import time; print('Compiling dep v1', flush=True); time.sleep(0.2)",
                str(self.repo_path),
                str(started),
                str(rows_file),
            ],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=self.env,
            start_new_session=True,
            close_fds=True,
        )
        os.close(slave)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, 0)
        self.assertIn("0:20", output)

    def test_group_sigint_reaps_child_records_interruption_and_reraises(self) -> None:
        child_pid_file = self.root / "child-pid"
        code = (
            "import os,time; "
            f"open({str(child_pid_file)!r}, 'w').write(str(os.getpid())); "
            "time.sleep(10)"
        )
        proc, master, started, _ = self._start(code)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not child_pid_file.exists():
            time.sleep(0.02)
        self.assertTrue(started.exists())
        child_pid = int(child_pid_file.read_text())
        os.killpg(proc.pid, signal.SIGINT)
        output = self._collect(proc, master).decode("utf-8", errors="replace")
        os.close(master)
        self.assertEqual(proc.returncode, -signal.SIGINT)
        self.assertIn("interrupted after", output)
        rows = state.read_history().value
        self.assertEqual({row["outcome"] for row in rows}, {"interrupted"})
        with self.assertRaises(ProcessLookupError):
            os.kill(child_pid, 0)


if __name__ == "__main__":
    unittest.main()

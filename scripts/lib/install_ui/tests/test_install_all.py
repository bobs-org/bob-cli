from __future__ import annotations

import fcntl
import json
import os
import pty
import select
import signal
import shutil
import socket
import struct
import subprocess
import tempfile
import termios
import time
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Optional

from install_ui import term

REPO_ROOT = Path(__file__).resolve().parents[4]
INSTALL_ALL_SRC = REPO_ROOT / "scripts" / "install_all"
LIB_SRC = REPO_ROOT / "scripts" / "lib"

FIXED_DATE = "2026-01-01T00:00:00+00:00"


def _base_git_env() -> dict:
    env = os.environ.copy()
    env.update(
        {
            "GIT_AUTHOR_NAME": "install-all-test",
            "GIT_AUTHOR_EMAIL": "install-all@test",
            "GIT_COMMITTER_NAME": "install-all-test",
            "GIT_COMMITTER_EMAIL": "install-all@test",
            "GIT_AUTHOR_DATE": FIXED_DATE,
            "GIT_COMMITTER_DATE": FIXED_DATE,
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_CONFIG_NOSYSTEM": "1",
        }
    )
    return env


def _git(
    cwd: Path, *args: str, env: Optional[dict] = None
) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(cwd), *args],
        check=True,
        capture_output=True,
        text=True,
        env=env or _base_git_env(),
    )


FAKE_JUST = """#!/usr/bin/env bash
set -u
jf=""
recipe=""
show=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --justfile) jf="$2"; shift 2 ;;
    --justfile=*) jf="${1#--justfile=}"; shift ;;
    --show) show=1; shift ;;
    -*) shift ;;
    *) recipe="$1"; shift; break ;;
  esac
done
source "$jf"
fn="recipe_${recipe//-/_}"
if [[ "$show" == "1" ]]; then
  declare -F "$fn" >/dev/null
  exit $?
fi
"$fn" "$@"
"""

FAKE_BOBCLI_JUSTFILE = """recipe_install() {
  printf '%s' "${BOB_INSTALL_ALL:-unset}" > "$SANDBOX_OUT/bob_install_all_env"
  # shellcheck disable=SC1091
  source "$SANDBOX_BOBCLI/scripts/lib/install_ui.sh"
  install_ui_run bob-cli cargo "$SANDBOX_BOBCLI" built --crate bob-obsidian \
    --profile local-install -- env RUSTC_WRAPPER= cargo install --path .
  status=$?
  [[ "$status" == "0" ]] || return "$status"
  install_ui_mark_installed bob-cli "$SANDBOX_BOBCLI"
  return 0
}
recipe_install_all() {
  "$SANDBOX_BOBCLI/scripts/install_all" "$@"
}
"""

FAKE_MAC_JUSTFILE = """recipe_install() {
  local status app
  printf 'install\n' >> "$SANDBOX_OUT/mac_order"
  echo "Building for production..."
  echo "Build complete!"
  if [[ -n "${FAKE_MAC_SLEEP:-}" ]]; then sleep "$FAKE_MAC_SLEEP"; fi
  status="${FAKE_MAC_INSTALL_STATUS:-0}"
  # Like the real install, a swapped-in app gets a fresh executable mtime.
  if [[ "$status" == 0 || "$status" == 72 ]]; then
    app="$HOME/Applications/Bob Mac Capture.app/Contents/MacOS"
    mkdir -p "$app"
    printf 'fake app\n' > "$app/BobMacCapture"
  fi
  return "$status"
}
recipe_signing_check() {
  printf 'signing-check\n' >> "$SANDBOX_OUT/mac_order"
  echo "${FAKE_SIGNING_IDENTITY:-Developer ID Application: Test}"
  if [[ "${FAKE_SIGNING_STATUS:-0}" != "0" ]]; then
    echo "fake signing check requested an unlock" >&2
  fi
  return "${FAKE_SIGNING_STATUS:-0}"
}
"""

FAKE_CARGO = """#!/usr/bin/env bash
echo "Compiling bob-obsidian v0.1.0"
if [[ -n "${FAKE_CARGO_STARTS:-}" ]]; then
  printf 'started\n' >> "$FAKE_CARGO_STARTS"
fi
if [[ -n "${FAKE_CARGO_SLEEP:-}" ]]; then sleep "$FAKE_CARGO_SLEEP"; fi
if [[ "${FAKE_JUST_INSTALL_FAIL:-}" == "1" ]]; then
  echo "error: fake cargo failure" >&2
  exit 1
fi
echo "Finished local-install profile"
"""

FAKE_BOB = """#!/usr/bin/env bash
if [[ "${1:-}" == "plugins" && "${2:-}" == "sync" ]]; then
  if [[ " $* " == *" --dry-run "* ]]; then
    if [[ "${FAKE_BOB_DRY_FAIL:-}" == "1" ]]; then
      printf '{"ok": false, "error": "fake preview reason"}\\n'
      exit 1
    fi
    if [[ "${FAKE_BOB_DRY_COPIED:-0}" == "0" ]]; then
      printf '{"copied": 0, "plugins": [], "not_configured": %s}\\n' "${FAKE_BOB_NOT_CONFIGURED:-[]}"
    else
      printf '{"copied": %s, "plugins": [{"id": "sample-plugin", "files": ' "${FAKE_BOB_DRY_COPIED}"
      printf '[{"name": "main.js", "action": "updated"}, '
      printf '{"name": "styles.css", "action": "unchanged"}]}], "not_configured": %s}\\n' "${FAKE_BOB_NOT_CONFIGURED:-[]}"
    fi
    exit 0
  fi
  if [[ "${FAKE_BOB_SYNC_FAIL:-}" == "1" ]]; then
    echo "error: fake sync failed" >&2
    exit 1
  fi
  if [[ "${FAKE_BOB_SYNC_WARNING:-}" == "1" ]]; then
    echo "  ⚠ sample-plugin  skipped main.js (dirty in vault)"
    echo "             ⚠ vault bundle is protected"
    echo "             +this diff must stay in the log"
  fi
  if [[ "${FAKE_BOB_SYNC_RAW:-}" == "1" ]]; then
    echo "fake sync raw diff: +source"
  fi
  exit 0
fi
if [[ "${1:-}" == "plugins" && "${2:-}" == "list" ]]; then
  list_plugins="${FAKE_BOB_LIST_PLUGINS:-}"
  if [[ -z "$list_plugins" ]]; then
    list_plugins=\'{"id": "sample-plugin", "sync": "synced", "configured": true}\'
  fi
  printf '{"count": 9, "synced": 9, "drift": %s, "plugins": [%s]}\\n' "${FAKE_BOB_DRIFT:-0}" "$list_plugins"
  exit 0
fi
echo "fake bob: unexpected $*" >&2
exit 1
"""

FAKE_UNAME = """#!/bin/sh
echo "${FAKE_UNAME:-Linux}"
"""

FAKE_OSASCRIPT = """#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$SANDBOX_OUT/osascript.log"
if [[ " $* " == *"is running"* ]]; then
  echo "false"
fi
exit 0
"""

FAKE_PGREP = """#!/bin/sh
if [ "${FAKE_OBSIDIAN_RUNNING:-}" = "1" ]; then exit 0; fi
exit 1
"""

FAKE_OPEN = """#!/bin/sh
exit 0
"""


class Sandbox:
    """An isolated install-all world: bare remotes plus three work clones."""

    def __init__(self, tmp: Path) -> None:
        self.root = tmp
        self.bin = tmp / "bin"
        self.home = tmp / "home"
        self.state = tmp / "state"
        self.parent = tmp / "parent"
        self.out = tmp / "out"
        self.remotes = tmp / "remotes"
        self.pushers = tmp / "pushers"
        self.bobcli = self.parent / "bob-cli"
        self.plugins = self.parent / "bob-plugins"
        self.maccap = self.parent / "bob-mac-capture"
        self._files = 0
        for path in (
            self.bin,
            self.home,
            self.state,
            self.parent,
            self.out,
            self.remotes,
            self.pushers,
        ):
            path.mkdir(parents=True, exist_ok=True)
        self._write_bins()
        self._seed_repo(
            "bob-cli",
            {
                "justfile": FAKE_BOBCLI_JUSTFILE,
                "README.md": "fake bob-cli\n",
            },
            copy_scripts=True,
        )
        self._seed_repo(
            "bob-plugins",
            {"plugins/placeholder.md": "fake plugin\n"},
        )
        self._seed_repo(
            "bob-mac-capture",
            {
                "justfile": FAKE_MAC_JUSTFILE,
                "Scripts/install.sh": "#!/bin/sh\nexit 0\n",
            },
        )
        (self.home / "bob" / ".obsidian").mkdir(parents=True, exist_ok=True)

    def _write_bin(self, name: str, body: str) -> None:
        path = self.bin / name
        path.write_text(body, encoding="utf-8")
        path.chmod(0o755)

    def _write_bins(self) -> None:
        self._write_bin("just", FAKE_JUST)
        self._write_bin("bob", FAKE_BOB)
        self._write_bin("cargo", FAKE_CARGO)
        self._write_bin("uname", FAKE_UNAME)
        self._write_bin("osascript", FAKE_OSASCRIPT)
        self._write_bin("pgrep", FAKE_PGREP)
        self._write_bin("open", FAKE_OPEN)

    def _seed_repo(self, name: str, files: dict, copy_scripts: bool = False) -> None:
        seed = self.root / f"seed-{name}"
        seed.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "init"],
            cwd=seed,
            check=True,
            capture_output=True,
            text=True,
            env=_base_git_env(),
        )
        if copy_scripts:
            scripts = seed / "scripts"
            (scripts / "lib" / "install_ui").mkdir(parents=True, exist_ok=True)
            shutil.copyfile(INSTALL_ALL_SRC, scripts / "install_all")
            os.chmod(scripts / "install_all", 0o755)
            shutil.copyfile(LIB_SRC / "install_ui.sh", scripts / "lib" / "install_ui.sh")
            for child in (LIB_SRC / "install_ui").iterdir():
                if child.is_file():
                    shutil.copyfile(child, scripts / "lib" / "install_ui" / child.name)
        for rel, body in files.items():
            target = seed / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(body, encoding="utf-8")
        _git(seed, "config", "user.email", "install-all@test")
        _git(seed, "config", "user.name", "install-all-test")
        _git(seed, "add", "-A")
        _git(seed, "commit", "-m", f"chore({name}): seed checkout")
        subprocess.run(
            ["git", "clone", "--bare", str(seed), str(self.remotes / f"{name}.git")],
            check=True,
            capture_output=True,
            text=True,
            env=_base_git_env(),
        )
        for dest in (self.parent / name, self.pushers / name):
            subprocess.run(
                ["git", "clone", str(self.remotes / f"{name}.git"), str(dest)],
                check=True,
                capture_output=True,
                text=True,
                env=_base_git_env(),
            )
            _git(dest, "config", "user.email", "install-all@test")
            _git(dest, "config", "user.name", "install-all-test")

    def commit_and_push(
        self, name: str, message: str, path: Optional[str] = None
    ) -> tuple:
        """Add one commit on the remote via the pusher clone; return (full, short).

        *path* is repo-relative; the default top-level ``note-N.txt`` counts
        as an app change for bob-mac-capture.
        """
        pusher = self.pushers / name
        self._files += 1
        rel = path or f"note-{self._files}.txt"
        target = pusher / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(f"{message}\n", encoding="utf-8")
        _git(pusher, "add", rel)
        _git(pusher, "commit", "-m", message)
        _git(pusher, "push", "origin", "HEAD")
        full = _git(pusher, "rev-parse", "HEAD").stdout.strip()
        short = _git(pusher, "rev-parse", "--short", "HEAD").stdout.strip()
        return full, short

    def base_env(self) -> dict:
        env = _base_git_env()
        env.update(
            {
                "HOME": str(self.home),
                "XDG_STATE_HOME": str(self.state),
                "BOB_DIR": str(self.home / "bob"),
                "TERM": "xterm-256color",
                "PATH": str(self.bin) + os.pathsep + env.get("PATH", ""),
                "SANDBOX_OUT": str(self.out),
                "SANDBOX_BOBCLI": str(self.bobcli),
                "BOB_INSTALL_STATE_DIR": "",
            }
        )
        # An empty override would resolve to the cwd; drop it so the state
        # directory falls back to the isolated XDG_STATE_HOME.
        del env["BOB_INSTALL_STATE_DIR"]
        # Seeded history rows record incremental=True, while incremental_for()
        # reads the caller's CARGO_INCREMENTAL (rust-cache exports 0 in CI).
        # Scrub it so the child install_all process always sees the default
        # (incremental enabled) regardless of the ambient environment.
        env.pop("CARGO_INCREMENTAL", None)
        return env

    def run(self, *args: str, **overrides: str) -> subprocess.CompletedProcess:
        env = self.base_env()
        env.update(overrides)
        return subprocess.run(
            [str(self.bobcli / "scripts" / "install_all"), *args],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            env=env,
            cwd=str(self.root),
            timeout=180,
        )

    def run_via_just(self, *args: str, **overrides: str) -> subprocess.CompletedProcess:
        env = self.base_env()
        env.update(overrides)
        return subprocess.run(
            [
                "just",
                "--justfile",
                str(self.bobcli / "justfile"),
                "install-all",
                *args,
            ],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            env=env,
            cwd=str(self.root),
            timeout=180,
        )

    @property
    def mac_app(self) -> Path:
        return self.home / "Applications" / "Bob Mac Capture.app"

    def mac_order(self) -> list:
        path = self.out / "mac_order"
        if not path.exists():
            return []
        return path.read_text(encoding="utf-8").splitlines()

    def run_record(self, run_id: str, name: str) -> Optional[dict]:
        path = self.state / "bob-cli" / "install-all" / "runs" / run_id / name
        if not path.exists():
            return None
        return json.loads(path.read_text(encoding="utf-8"))

    def watermark_sha(self, repo: str) -> str:
        raw = (self.state / "bob-cli" / "install-all" / "installed.json").read_text(
            encoding="utf-8"
        )
        return json.loads(raw)["repos"][repo]["sha"]

    def digest_record(self) -> Optional[dict]:
        runs = self.state / "bob-cli" / "install-all" / "runs"
        if not runs.is_dir():
            return None
        records = sorted(runs.glob("*/digest.json"))
        if not records:
            return None
        return json.loads(records[-1].read_text(encoding="utf-8"))


def _run_install_all_pty(
    sbox: Sandbox,
    *args: str,
    interrupt_when: Optional[Path] = None,
    cols: int = 80,
    lines: int = 24,
    **overrides: str,
) -> tuple[int, bytes, bytes]:
    env = sbox.base_env()
    env.update(overrides)
    master, slave = pty.openpty()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", lines, cols, 0, 0))
    except OSError:
        pass
    proc = subprocess.Popen(
        [str(sbox.bobcli / "scripts" / "install_all"), *args],
        stdin=slave,
        stdout=slave,
        stderr=subprocess.PIPE,
        env=env,
        cwd=str(sbox.root),
        start_new_session=True,
    )
    os.close(slave)
    chunks = []
    interrupted = False
    deadline = time.monotonic() + 40
    try:
        while True:
            if interrupt_when is not None and interrupt_when.exists() and not interrupted:
                os.killpg(proc.pid, signal.SIGINT)
                interrupted = True
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    chunk = b""
                if chunk:
                    chunks.append(chunk)
                elif proc.poll() is not None:
                    break
            if proc.poll() is not None and not ready:
                break
            if time.monotonic() >= deadline:
                proc.kill()
                proc.wait()
                raise AssertionError("install-all PTY run did not finish")
    finally:
        os.close(master)
    assert proc.stderr is not None
    err = proc.stderr.read()
    proc.stderr.close()
    proc.wait(timeout=5)
    return proc.returncode, b"".join(chunks), err


class _InstallAllCase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.sbox = Sandbox(Path(self._tmp.name))

    def tearDown(self) -> None:
        self._tmp.cleanup()


class HeaderAndDigestTests(_InstallAllCase):
    def test_nothing_new_has_rows_but_no_digest(self) -> None:
        proc = self.sbox.run()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("INSTALL ALL", out)
        self.assertIn("bob-cli", out)
        self.assertIn("up to date", out)
        self.assertIn("bob-plugins", out)
        self.assertNotIn("WHAT'S NEW", out)
        self.assertIn("ALL INSTALLED", out)
        self.assertNotIn("\a", out)
        env_marker = (self.sbox.out / "bob_install_all_env").read_text(
            encoding="utf-8"
        )
        self.assertEqual(env_marker, "1")
        record = self.sbox.digest_record()
        self.assertIsNotNone(record)
        assert record is not None
        self.assertEqual(record["rows"], 0)

    def test_self_update_reexec_prints_digest_once(self) -> None:
        full, short = self.sbox.commit_and_push(
            "bob-cli", "feat(cli): teach install-all the pull-first order"
        )
        proc = self.sbox.run()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertEqual(out.count("INSTALL ALL"), 1)
        self.assertIn("1 new commit", out)
        self.assertIn("WHAT'S NEW", out)
        self.assertIn("teach install-all the pull-first order", out)
        self.assertIn(f"installed @ {short}", out)
        self.assertIn("ALL INSTALLED", out)
        self.assertLess(out.index("bob-cli"), out.index("WHAT'S NEW"))
        self.assertLess(out.index("WHAT'S NEW"), out.index("BOB-CLI"))
        self.assertEqual(self.sbox.watermark_sha("bob-cli"), full)
        record = self.sbox.digest_record()
        self.assertIsNotNone(record)
        assert record is not None
        self.assertGreater(record["rows"], 0)
        names = [repo["name"] for repo in record["repos"]]
        self.assertIn("bob-cli", names)

    def test_sibling_update_shows_in_digest(self) -> None:
        self.sbox.commit_and_push(
            "bob-plugins", "fix(plugins): copy only changed plugin files"
        )
        proc = self.sbox.run()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("1 new commit", out)
        self.assertIn("WHAT'S NEW", out)
        self.assertIn("copy only changed plugin files", out)
        names = [repo["name"] for repo in self.sbox.digest_record()["repos"]]
        self.assertIn("bob-plugins", names)
        # The plugin sync succeeded with no drift, so its watermark advances.
        head = _git(self.sbox.plugins, "rev-parse", "HEAD").stdout.strip()
        self.assertEqual(self.sbox.watermark_sha("bob-plugins"), head)

    def test_ui_zero_skips_digest_but_keeps_pull_rows(self) -> None:
        proc = self.sbox.run(BOB_INSTALL_UI="0")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("bob-cli", out)
        self.assertIn("up to date", out)
        self.assertNotIn("WHAT'S NEW", out)
        self.assertIn("ALL INSTALLED", out)
        self.assertIsNone(self.sbox.digest_record())


class LegacyUpgradeTests(_InstallAllCase):
    def _pull_work_clone(self, name: str) -> tuple:
        old = _git(self.sbox.parent / name, "rev-parse", "--short", "HEAD").stdout.strip()
        _git(self.sbox.parent / name, "pull", "--ff-only", "--no-rebase")
        new = _git(self.sbox.parent / name, "rev-parse", "--short", "HEAD").stdout.strip()
        return old, new

    def test_legacy_detail_resolves_into_digest(self) -> None:
        self.sbox.commit_and_push(
            "bob-cli", "feat(cli): teach install-all the pull-first order"
        )
        old, new = self._pull_work_clone("bob-cli")
        proc = self.sbox.run(
            BOB_INSTALL_ALL_SELF_PULL="updated",
            BOB_INSTALL_ALL_SELF_PULL_DETAIL=f"1 new commit · {old} → {new}",
            BOB_INSTALL_ALL_HEADER_DONE="1",
            BOB_INSTALL_ALL_CLONES_OFFERED="1",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("teach install-all the pull-first order", out)
        self.assertIn("in this pull", out)
        self.assertNotIn("changes unavailable", out)

    def test_legacy_garbage_detail_prints_one_dim_row(self) -> None:
        proc = self.sbox.run(
            BOB_INSTALL_ALL_SELF_PULL="updated",
            BOB_INSTALL_ALL_SELF_PULL_DETAIL="something the old script never wrote",
            BOB_INSTALL_ALL_HEADER_DONE="1",
            BOB_INSTALL_ALL_CLONES_OFFERED="1",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("changes unavailable for this upgrade", out)
        self.assertNotIn("WHAT'S NEW", out)


class FailureAndRetryTests(_InstallAllCase):
    def test_failed_sibling_pull_warns_and_keeps_going(self) -> None:
        _git(
            self.sbox.plugins,
            "remote",
            "set-url",
            "origin",
            str(self.sbox.root / "no-such-remote.git"),
        )
        proc = self.sbox.run()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("pull failed", out)
        self.assertIn("INSTALLED WITH 1 WARNING", out)

    def test_retry_after_failed_install_shows_same_commits(self) -> None:
        first = self.sbox.run()
        self.assertEqual(first.returncode, 0, first.stderr)
        self.sbox.commit_and_push(
            "bob-cli", "feat(cli): teach install-all the pull-first order"
        )
        failed = self.sbox.run(FAKE_JUST_INSTALL_FAIL="1")
        self.assertEqual(failed.returncode, 1, failed.stderr)
        self.assertIn("teach install-all the pull-first order", failed.stdout)
        self.assertIn("STEP FAILED", failed.stdout)
        retry = self.sbox.run()
        self.assertEqual(retry.returncode, 0, retry.stderr)
        # The pull is a no-op now, but the watermark still predates the
        # commit, so the same digest range shows again.
        self.assertIn("teach install-all the pull-first order", retry.stdout)
        self.assertIn("ALL INSTALLED", retry.stdout)

    def test_bob_cli_summary_duration_excludes_slow_sibling_pull(self) -> None:
        self.sbox.commit_and_push(
            "bob-plugins", "fix(plugins): exercise delayed sibling pull"
        )
        real_git = shutil.which("git")
        assert real_git is not None
        self.sbox._write_bin(
            "git",
            "#!/bin/sh\n"
            "case \"$*\" in *bob-plugins*pull*) sleep 2 ;; esac\n"
            f"exec {real_git} \"$@\"\n",
        )
        proc = self.sbox.run()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        cli_row = next(
            line for line in proc.stdout.splitlines()
            if line.startswith("  ✓  bob-cli") and "installed @" in line
        )
        duration = cli_row.rsplit("  ", 1)[-1]
        self.assertTrue(duration.endswith("s"), cli_row)
        self.assertLess(int(duration[:-1]), 2, cli_row)


class QuietBuildTests(_InstallAllCase):
    def test_pty_live_runner_replaces_cargo_output_with_built_row(self) -> None:
        starts = self.sbox.out / "cargo-starts"
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_CARGO_SLEEP="0.4",
            FAKE_CARGO_STARTS=str(starts),
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("compiling bob-obsidian", out)
        self.assertIn("✓ built", out)
        self.assertTrue(any(ch in out for ch in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"), out)
        self.assertEqual(starts.read_text(encoding="utf-8").splitlines(), ["started"])

    def test_verbose_option_is_forwarded_and_streams_raw_output(self) -> None:
        proc = self.sbox.run_via_just(
            "-v",
            FAKE_BOB_SYNC_RAW="1",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("Finished local-install profile", proc.stdout)
        self.assertIn("fake sync raw diff: +source", proc.stdout)
        self.assertNotIn("↻ sample-plugin", proc.stdout)
        self.assertIn("-v, --verbose", self.sbox.run("--help").stdout)

        status, raw, err = _run_install_all_pty(
            self.sbox,
            "--verbose",
            FAKE_BOB_SYNC_RAW="1",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("Finished local-install profile", out)
        self.assertIn("fake sync raw diff: +source", out)
        self.assertNotIn("✓ built", out)

    def test_live_build_failure_shows_excerpt_and_failure_summary(self) -> None:
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_JUST_INSTALL_FAIL="1",
        )
        out = raw.decode("utf-8", "replace")
        plain = term.strip_ansi(out)
        self.assertEqual(status, 1, err.decode("utf-8", "replace"))
        self.assertIn("build failed", out)
        self.assertIn("error: fake cargo failure", out)
        self.assertIn("full log", out)
        self.assertIn("rerun with  just install-all -v", out)
        self.assertIn("  ✗ build failed", out)
        self.assertIn("      error: fake cargo failure", plain)
        self.assertIn("STEP FAILED", out)

    def test_ctrl_c_stops_after_started_build_without_summary_or_bell(self) -> None:
        starts = self.sbox.out / "interrupted-cargo-starts"
        status, raw, err = _run_install_all_pty(
            self.sbox,
            interrupt_when=starts,
            FAKE_CARGO_SLEEP="15",
            FAKE_CARGO_STARTS=str(starts),
            BOB_INSTALL_BELL_AFTER="0",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 130, err.decode("utf-8", "replace"))
        self.assertEqual(out.count("interrupted after"), 1, out)
        self.assertNotIn("SUMMARY", out)
        self.assertNotIn("BOB-PLUGINS", out)
        self.assertNotIn("\a", out)
        self.assertEqual(starts.read_text(encoding="utf-8").splitlines(), ["started"])

    def test_plugin_sync_diff_is_logged_and_warning_rows_stay_visible(self) -> None:
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_BOB_DRY_COPIED="1",
            FAKE_BOB_SYNC_WARNING="1",
            FAKE_BOB_SYNC_RAW="1",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("↻ sample-plugin  main.js", out)
        self.assertIn("⚠ sample-plugin  skipped main.js", out)
        self.assertIn("⚠ vault bundle is protected", out)
        self.assertNotIn("+this diff must stay in the log", out)
        self.assertNotIn("fake sync raw diff", out)
        log_dir = self.sbox.state / "bob-cli" / "install-all" / "logs"
        logs = list(log_dir.glob("*-bob-plugins.log"))
        self.assertEqual(len(logs), 1)
        log = logs[0].read_text(encoding="utf-8")
        self.assertIn("+this diff must stay in the log", log)
        self.assertIn("fake sync raw diff", log)

    def test_plugin_sync_failure_prints_log_excerpt_and_path(self) -> None:
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_BOB_SYNC_FAIL="1",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 1, err.decode("utf-8", "replace"))
        self.assertIn("plugin sync log excerpt", out)
        self.assertIn("error: fake sync failed", out)
        self.assertIn("full log", out)

    def test_failed_preview_prints_json_error(self) -> None:
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_BOB_DRY_FAIL="1",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 1, err.decode("utf-8", "replace"))
        self.assertIn("could not preview plugin sync", out)
        self.assertIn("fake preview reason", out)

    def test_unconfigured_plugins_named_and_excluded_from_counts(
        self,
    ) -> None:
        rows = (
            '{"id": "sample-plugin", "sync": "synced", "configured": true}, '
            '{"id": "drift-plugin", "sync": "drift", "configured": true}, '
            '{"id": "left-out", "sync": "drift", "configured": false}'
        )
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_BOB_DRY_COPIED="1",
            FAKE_BOB_NOT_CONFIGURED='["left-out"]',
            FAKE_BOB_LIST_PLUGINS=rows,
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("not in plugins.sync; skipped: left-out", out)
        self.assertIn("1/2 plugins in sync", out)
        self.assertIn("1 drifted", out)

    def test_sync_and_obsidian_restart_append_timed_history_rows(self) -> None:
        cli = self.sbox.home / ".local" / "bin" / "obsidian"
        cli.parent.mkdir(parents=True, exist_ok=True)
        cli.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        cli.chmod(0o755)
        proc = self.sbox.run(
            FAKE_BOB_DRY_COPIED="1",
            FAKE_OBSIDIAN_RUNNING="1",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        history = self.sbox.state / "bob-cli" / "install-all" / "runs.jsonl"
        rows = [json.loads(line) for line in history.read_text(encoding="utf-8").splitlines()]
        observed = {(row["step"], row["phase"], row["outcome"]) for row in rows}
        self.assertIn(("bob-plugins", "sync", "ok"), observed)
        self.assertIn(("obsidian", "restart", "ok"), observed)


class MacSigningPreflightTests(_InstallAllCase):
    def test_signing_preflight_runs_before_live_install_and_keeps_exit_72(self) -> None:
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_UNAME="Darwin",
            FAKE_MAC_INSTALL_STATUS="72",
            FAKE_MAC_SLEEP="0.3",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        signing_row = out.index("signing  Developer ID Application: Test")
        built_row = out.index("built and signed")
        self.assertLess(signing_row, built_row)
        self.assertIn("restart it via", out)
        self.assertEqual(
            (self.sbox.out / "mac_order").read_text(encoding="utf-8").splitlines(),
            ["signing-check", "install"],
        )

    def test_locked_signing_preflight_shows_remedy_and_skips_install(self) -> None:
        status, raw, err = _run_install_all_pty(
            self.sbox,
            FAKE_UNAME="Darwin",
            FAKE_SIGNING_STATUS="77",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 1, err.decode("utf-8", "replace"))
        self.assertIn("signing identity locked in this session", out)
        self.assertIn("security unlock-keychain", out)
        self.assertEqual(
            (self.sbox.out / "mac_order").read_text(encoding="utf-8").splitlines(),
            ["signing-check"],
        )


class DarwinTests(_InstallAllCase):
    def test_exit_72_warns_advances_watermark_and_posts_banner(self) -> None:
        proc = self.sbox.run(
            FAKE_UNAME="Darwin",
            FAKE_MAC_INSTALL_STATUS="72",
            BOB_INSTALL_NOTIFY_AFTER="0",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = proc.stdout
        self.assertIn("bob-mac-capture", out)
        self.assertIn("restart it via", out)
        self.assertIn("INSTALLED WITH", out)
        head = _git(self.sbox.maccap, "rev-parse", "HEAD").stdout.strip()
        self.assertEqual(self.sbox.watermark_sha("bob-mac-capture"), head)
        banner_log = (self.sbox.out / "osascript.log").read_text(encoding="utf-8")
        self.assertIn("INSTALLED WITH 1 WARNING", banner_log)

    def test_notify_zero_suppresses_macos_banner(self) -> None:
        proc = self.sbox.run(
            FAKE_UNAME="Darwin",
            BOB_INSTALL_NOTIFY="0",
            BOB_INSTALL_NOTIFY_AFTER="0",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse((self.sbox.out / "osascript.log").exists())


class BellTests(_InstallAllCase):
    def _run_on_pty(self, **overrides: str) -> bytes:
        status, output, err = _run_install_all_pty(self.sbox, **overrides)
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        return output

    def test_bell_rings_on_pty_but_not_on_pipe(self) -> None:
        piped = self.sbox.run(BOB_INSTALL_BELL_AFTER="0")
        self.assertEqual(piped.returncode, 0, piped.stderr)
        self.assertNotIn("\a", piped.stdout)
        tty_out = self._run_on_pty(BOB_INSTALL_BELL_AFTER="0")
        self.assertIn(b"\a", tty_out)

    def test_notify_zero_suppresses_bell_on_pty(self) -> None:
        tty_out = self._run_on_pty(
            BOB_INSTALL_BELL_AFTER="0", BOB_INSTALL_NOTIFY="0"
        )
        self.assertNotIn(b"\a", tty_out)


def _seed_bobcli_history(sbox: Sandbox, *, n: int = 6, kind: str = "cold") -> None:
    sbox._write_bin("rustc", "#!/bin/sh\necho 'rustc 1.95.0 (install-ui-test)'\n")
    host = socket.gethostname()
    path = sbox.state / "bob-cli" / "install-all"
    path.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc)
    rows = []
    crates = [12.0 + index for index in range(n)]
    for index in range(n):
        stamp = (now - timedelta(days=n - index)).isoformat()
        run = f"seed-{index}"
        for phase, secs in (
            ("index", 1.0),
            ("deps", 2.0),
            ("crate", crates[index]),
            ("finish", 1.0),
        ):
            rows.append(
                json.dumps(
                    {
                        "v": 1,
                        "run": run,
                        "ts": stamp,
                        "host": host,
                        "step": "bob-cli",
                        "phase": phase,
                        "kind": kind,
                        "secs": secs,
                        "waited": 0.0,
                        "outcome": "ok",
                        "toolchain": "rustc 1.95.0 (install-ui-test)",
                        "profile": "local-install",
                        "incremental": True,
                        "slept": False,
                    }
                )
            )
    (path / "runs.jsonl").write_text("\n".join(rows) + "\n", encoding="utf-8")


class EarnedEtaTests(_InstallAllCase):
    def test_seeded_history_shows_bar_range_plan_and_summary(self) -> None:
        _seed_bobcli_history(self.sbox)
        self.sbox.commit_and_push(
            "bob-cli", "feat(cli): glue start and past timing to Pomodoro names"
        )
        status, raw, err = _run_install_all_pty(
            self.sbox,
            cols=100,
            lines=24,
            FAKE_CARGO_SLEEP="0.8",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("usually", out)
        self.assertIn("done by about", out)
        self.assertTrue("─" in out and ("╸" in out or "━" in out), out)
        self.assertTrue(
            "min left" in out or "under a minute" in out,
            out,
        )
        self.assertRegex(out, r"(faster|about|slower) than usual|about usual")
        self.assertIn("ALL INSTALLED", out)

    def test_empty_history_shows_learning_text_without_plan(self) -> None:
        self.sbox._write_bin(
            "rustc", "#!/bin/sh\necho 'rustc 1.95.0 (install-ui-test)'\n"
        )
        status, raw, err = _run_install_all_pty(
            self.sbox,
            cols=100,
            lines=24,
            FAKE_CARGO_SLEEP="0.3",
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("first timed", out)
        self.assertNotIn("usually", out)
        self.assertNotIn(" · about usual", out)
        self.assertNotIn("faster than usual", out)
        self.assertNotIn("slower than usual", out)



class MacCaptureSkipTests(_InstallAllCase):
    """Darwin runs rebuild Mac Capture only when its app no longer matches."""

    def _install(self, *args: str, **overrides: str) -> subprocess.CompletedProcess:
        env = {"FAKE_UNAME": "Darwin"}
        env.update(overrides)
        proc = self.sbox.run(*args, **env)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        return proc

    def _maccap_head(self) -> str:
        return _git(self.sbox.maccap, "rev-parse", "HEAD").stdout.strip()

    def test_second_run_with_nothing_new_skips(self) -> None:
        first = self._install()
        self.assertIn("rebuild  app not installed at ~/Applications/", first.stdout)
        self.assertEqual(self.sbox.mac_order(), ["install"])
        head = self._maccap_head()
        proc = self._install()
        self.assertEqual(self.sbox.mac_order(), ["install"])
        self.assertIn(f"app current @{head[:7]}", proc.stdout)
        self.assertIn("not rebuilt", proc.stdout)
        self.assertNotIn("rebuild  ", proc.stdout)
        self.assertRegex(
            proc.stdout,
            r"–  bob-mac-capture +up to date · \w+ · app current @\w+ · not rebuilt",
        )
        self.assertIn("ALL INSTALLED", proc.stdout.strip().splitlines()[-1])
        self.assertEqual(self.sbox.watermark_sha("bob-mac-capture"), head)

    def test_docs_only_push_skips_and_advances_watermark(self) -> None:
        self._install(BOB_INSTALL_RUN_ID="docs-1")
        old = self._maccap_head()
        new, _ = self.sbox.commit_and_push(
            "bob-mac-capture", "fix(docs): correct the capture panel guide", "docs/guide.md"
        )
        proc = self._install(BOB_INSTALL_RUN_ID="docs-2")
        self.assertEqual(self.sbox.mac_order(), ["install"])
        # This run's digest still reads the old watermark.
        self.assertIn("correct the capture panel guide", proc.stdout)
        digest = self.sbox.run_record("docs-2", "digest.json") or {}
        self.assertIn("bob-mac-capture", [repo["name"] for repo in digest.get("repos", [])])
        self.assertIn(f"only docs/tests changed since @{old[:7]} · not rebuilt", proc.stdout)
        self.assertEqual(self.sbox.watermark_sha("bob-mac-capture"), new)

        again = self._install(BOB_INSTALL_RUN_ID="docs-3")
        self.assertEqual(self.sbox.mac_order(), ["install"])
        self.assertIn(f"app current @{new[:7]}", again.stdout)
        self.assertNotIn("correct the capture panel guide", again.stdout)
        digest = self.sbox.run_record("docs-3", "digest.json") or {}
        self.assertNotIn(
            "bob-mac-capture", [repo["name"] for repo in digest.get("repos", [])]
        )

    def test_sources_push_rebuilds(self) -> None:
        self._install()
        old = self._maccap_head()
        new, _ = self.sbox.commit_and_push(
            "bob-mac-capture", "feat(panel): widen the capture panel", "Sources/App/Panel.swift"
        )
        proc = self._install()
        self.assertIn(f"rebuild  1 app file changed since @{old[:7]}", proc.stdout)
        self.assertNotIn("not rebuilt", proc.stdout)
        self.assertEqual(self.sbox.mac_order(), ["install", "install"])
        self.assertEqual(self.sbox.watermark_sha("bob-mac-capture"), new)

    def test_force_flag_reinstalls_current_app_across_self_update(self) -> None:
        self._install()
        # The bob-cli commit makes install_all re-exec itself, so -f reaches
        # the new script only through the environment handoff.
        self.sbox.commit_and_push("bob-cli", "fix(cli): exercise the force handoff")
        forced = self._install("-f")
        self.assertEqual(forced.stdout.count("INSTALL ALL"), 1)
        self.assertIn("1 new commit", forced.stdout)
        self.assertIn("rebuild  forced by --force-mac-capture", forced.stdout)
        self.assertEqual(self.sbox.mac_order(), ["install", "install"])

        long_form = self._install("--force-mac-capture")
        self.assertIn("rebuild  forced by --force-mac-capture", long_form.stdout)
        self.assertEqual(len(self.sbox.mac_order()), 3)

        plain = self._install()
        self.assertIn("not rebuilt", plain.stdout)
        self.assertEqual(len(self.sbox.mac_order()), 3)
        self.assertIn("-f, --force-mac-capture", self.sbox.run("--help").stdout)

    def test_deleted_app_bundle_reinstalls(self) -> None:
        self._install()
        shutil.rmtree(self.sbox.mac_app)
        proc = self._install()
        self.assertIn(
            "rebuild  app not installed at ~/Applications/Bob Mac Capture.app",
            proc.stdout,
        )
        self.assertEqual(self.sbox.mac_order(), ["install", "install"])
        self.assertTrue((self.sbox.mac_app / "Contents" / "MacOS" / "BobMacCapture").exists())

    def test_artifact_newer_than_install_record_reinstalls(self) -> None:
        self._install()
        exe = self.sbox.mac_app / "Contents" / "MacOS" / "BobMacCapture"
        future = time.time() + 3600
        os.utime(exe, (future, future))
        proc = self._install()
        self.assertIn("rebuild  app was replaced outside install-all", proc.stdout)
        self.assertEqual(self.sbox.mac_order(), ["install", "install"])

    def test_skip_on_pty_never_runs_signing_check(self) -> None:
        status, raw, err = _run_install_all_pty(self.sbox, FAKE_UNAME="Darwin")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertEqual(self.sbox.mac_order(), ["signing-check", "install"])
        status, raw, err = _run_install_all_pty(self.sbox, FAKE_UNAME="Darwin")
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("not rebuilt", out)
        self.assertNotIn("signing  ", out)
        self.assertEqual(self.sbox.mac_order(), ["signing-check", "install"])

    def test_ui_zero_always_reinstalls(self) -> None:
        self._install()
        proc = self._install(BOB_INSTALL_UI="0")
        self.assertEqual(self.sbox.mac_order(), ["install", "install"])
        self.assertNotIn("not rebuilt", proc.stdout)
        self.assertNotIn("rebuild  ", proc.stdout)
        self.assertIn("installed ~/Applications/Bob Mac Capture.app", proc.stdout)

    def test_failed_pull_with_current_app_warns(self) -> None:
        self._install()
        _git(
            self.sbox.maccap,
            "remote",
            "set-url",
            "origin",
            str(self.sbox.root / "no-such-remote.git"),
        )
        proc = self._install()
        self.assertEqual(self.sbox.mac_order(), ["install"])
        self.assertRegex(
            proc.stdout,
            r"⚠  bob-mac-capture +pull failed — using local checkout · "
            r"app current @\w+ · not rebuilt",
        )
        self.assertIn("INSTALLED WITH 1 WARNING", proc.stdout)

    def test_skipped_step_is_left_out_of_plan_record(self) -> None:
        _seed_bobcli_history(self.sbox)
        self._install()
        # A bob-cli commit keeps its predicted kind off the no-op path.
        self.sbox.commit_and_push("bob-cli", "fix(cli): keep the plan honest")
        status, raw, err = _run_install_all_pty(
            self.sbox, cols=100, FAKE_UNAME="Darwin", BOB_INSTALL_RUN_ID="plan-skip"
        )
        out = raw.decode("utf-8", "replace")
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        self.assertIn("not rebuilt", out)
        self.assertIn("usually", out)
        plan = self.sbox.run_record("plan-skip", "plan.json")
        self.assertIsNotNone(plan)
        assert plan is not None
        self.assertTrue(plan["shown"], plan)

        # A forced rebuild needs Mac Capture history this host lacks, so no
        # plan range is earned.
        status, raw, err = _run_install_all_pty(
            self.sbox,
            "--force-mac-capture",
            cols=100,
            FAKE_UNAME="Darwin",
            BOB_INSTALL_RUN_ID="plan-forced",
        )
        self.assertEqual(status, 0, err.decode("utf-8", "replace"))
        forced = self.sbox.run_record("plan-forced", "plan.json")
        self.assertIsNotNone(forced)
        assert forced is not None
        self.assertFalse(forced["shown"], forced)


if __name__ == "__main__":
    unittest.main()

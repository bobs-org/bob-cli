from __future__ import annotations

import json
import os
import pty
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Optional

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
while [[ $# -gt 0 ]]; do
  case "$1" in
    --justfile) jf="$2"; shift 2 ;;
    --justfile=*) jf="${1#--justfile=}"; shift ;;
    -*) shift ;;
    *) recipe="$1"; shift; break ;;
  esac
done
source "$jf"
fn="recipe_${recipe//-/_}"
"$fn" "$@"
"""

FAKE_BOBCLI_JUSTFILE = """recipe_install() {
  printf '%s' "${BOB_INSTALL_ALL:-unset}" > "$SANDBOX_OUT/bob_install_all_env"
  if [[ "${FAKE_JUST_INSTALL_FAIL:-}" == "1" ]]; then
    echo "error: fake cargo failure" >&2
    return 1
  fi
  echo "fake cargo: Compiling bob-obsidian"
  echo "fake cargo: Finished"
  # shellcheck disable=SC1091
  source "$SANDBOX_BOBCLI/scripts/lib/install_ui.sh"
  install_ui_mark_installed bob-cli "$SANDBOX_BOBCLI"
  return 0
}
"""

FAKE_MAC_JUSTFILE = """recipe_install() {
  echo "fake mac install step"
  return "${FAKE_MAC_INSTALL_STATUS:-0}"
}
"""

FAKE_BOB = """#!/usr/bin/env bash
if [[ "${1:-}" == "plugins" && "${2:-}" == "sync" ]]; then
  if [[ " $* " == *" --dry-run "* ]]; then
    printf '{"copied": %s}\\n' "${FAKE_BOB_DRY_COPIED:-0}"
    exit 0
  fi
  if [[ "${FAKE_BOB_SYNC_FAIL:-}" == "1" ]]; then
    echo "fake sync failed" >&2
    exit 1
  fi
  exit 0
fi
if [[ "${1:-}" == "plugins" && "${2:-}" == "list" ]]; then
  printf '{"count": 9, "synced": 9, "drift": %s}\\n' "${FAKE_BOB_DRIFT:-0}"
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

    def commit_and_push(self, name: str, message: str) -> tuple:
        """Add one commit on the remote via the pusher clone; return (full, short)."""
        pusher = self.pushers / name
        self._files += 1
        target = pusher / f"note-{self._files}.txt"
        target.write_text(f"{message}\n", encoding="utf-8")
        _git(pusher, "add", target.name)
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
                "PATH": str(self.bin) + os.pathsep + env.get("PATH", ""),
                "SANDBOX_OUT": str(self.out),
                "SANDBOX_BOBCLI": str(self.bobcli),
                "BOB_INSTALL_STATE_DIR": "",
            }
        )
        # An empty override would resolve to the cwd; drop it so the state
        # directory falls back to the isolated XDG_STATE_HOME.
        del env["BOB_INSTALL_STATE_DIR"]
        return env

    def run(self, **overrides: str) -> subprocess.CompletedProcess:
        env = self.base_env()
        env.update(overrides)
        return subprocess.run(
            [str(self.bobcli / "scripts" / "install_all")],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            env=env,
            cwd=str(self.root),
            timeout=180,
        )

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
        env = self.sbox.base_env()
        env.update(overrides)
        master, slave = pty.openpty()
        proc = subprocess.Popen(
            [str(self.sbox.bobcli / "scripts" / "install_all")],
            stdin=subprocess.DEVNULL,
            stdout=slave,
            stderr=subprocess.PIPE,
            env=env,
            cwd=str(self.sbox.root),
        )
        os.close(slave)
        chunks = []
        try:
            while True:
                try:
                    data = os.read(master, 1024)
                except OSError:
                    break
                if not data:
                    break
                chunks.append(data)
        finally:
            os.close(master)
        assert proc.stderr is not None
        err = proc.stderr.read()
        proc.stderr.close()
        proc.wait(timeout=180)
        self.assertEqual(proc.returncode, 0, err.decode("utf-8", "replace"))
        return b"".join(chunks)

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


if __name__ == "__main__":
    unittest.main()

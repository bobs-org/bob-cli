from __future__ import annotations

import fcntl
import json
import os
import subprocess
import tempfile
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable, Iterator, Optional

HISTORY_SOFT_LIMIT = 2000
HISTORY_KEEP = 1500
HISTORY_ROW_MAX = 4096
CRASH_LOG_MAX = 200 * 1024
LOGS_KEEP_PER_STEP = 10
RUN_DIRS_KEEP = 10
DIR_MODE = 0o700
FILE_MODE = 0o600

Clock = Callable[[], datetime]


@dataclass(frozen=True)
class Result:
    ok: bool
    value: object = None
    error: str = ""

    @staticmethod
    def success(value: object = None) -> "Result":
        return Result(True, value, "")

    @staticmethod
    def fail(error: str) -> "Result":
        return Result(False, None, error)


def state_dir() -> Path:
    override = os.environ.get("BOB_INSTALL_STATE_DIR")
    if override:
        return Path(override)
    xdg = os.environ.get("XDG_STATE_HOME")
    if xdg:
        return Path(xdg) / "bob-cli" / "install-all"
    return Path.home() / ".local" / "state" / "bob-cli" / "install-all"


def _ensure_dir(path: Path) -> None:
    path.mkdir(mode=DIR_MODE, parents=True, exist_ok=True)


def _chmod_file(path: Path) -> None:
    try:
        os.chmod(path, FILE_MODE)
    except OSError:
        pass


@contextmanager
def _exclusive_lock(lock_path: Path) -> Iterator[None]:
    _ensure_dir(lock_path.parent)
    fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, FILE_MODE)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


def _atomic_write(path: Path, data: bytes) -> None:
    _ensure_dir(path.parent)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=str(path.parent))
    tmp_path: Optional[Path] = Path(tmp)
    try:
        written = 0
        while written < len(data):
            written += os.write(fd, data[written:])
        os.fsync(fd)
        os.close(fd)
        fd = -1
        os.chmod(tmp, FILE_MODE)
        os.rename(tmp, path)
        tmp_path = None
        dir_fd = os.open(str(path.parent), os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        if fd >= 0:
            os.close(fd)
        if tmp_path is not None and tmp_path.exists():
            try:
                tmp_path.unlink()
            except OSError:
                pass


def _iso_now(clock: Optional[Clock] = None) -> str:
    now = clock() if clock is not None else datetime.now().astimezone()
    return now.isoformat(timespec="seconds")


def _empty_installed() -> dict:
    return {"v": 1, "repos": {}}


def _parse_installed(raw: str) -> dict:
    try:
        data = json.loads(raw)
    except (json.JSONDecodeError, ValueError, TypeError):
        return _empty_installed()
    if not isinstance(data, dict):
        return _empty_installed()
    repos = data.get("repos")
    if not isinstance(repos, dict):
        repos = {}
    cleaned = {}
    for name, record in repos.items():
        if isinstance(name, str) and isinstance(record, dict):
            cleaned[name] = record
    return {"v": 1, "repos": cleaned}


def read_installed() -> dict:
    """Return the watermark file, or an empty v1 document. Never raises."""
    path = state_dir() / "installed.json"
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError:
        return _empty_installed()
    except UnicodeError:
        return _empty_installed()
    return _parse_installed(raw)


def get_watermark(repo: str) -> Result:
    data = read_installed()
    record = data["repos"].get(repo)
    if not isinstance(record, dict):
        return Result.fail(f"no watermark for {repo}")
    return Result.success(record)


def write_watermark(
    repo: str,
    record: dict,
    *,
    clock: Optional[Clock] = None,
) -> Result:
    path = state_dir() / "installed.json"
    lock_path = state_dir() / "installed.json.lock"
    payload = {
        "sha": record["sha"],
        "at": record["at"] if "at" in record else _iso_now(clock),
        "dirty": bool(record.get("dirty")),
        "path": record["path"],
    }
    try:
        with _exclusive_lock(lock_path):
            data = read_installed()
            data["repos"][repo] = payload
            encoded = json.dumps(data, indent=2, sort_keys=True) + "\n"
            _atomic_write(path, encoded.encode("utf-8"))
        return Result.success(payload)
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")
    except (KeyError, TypeError, ValueError) as exc:
        return Result.fail(f"state unavailable: {exc}")


def snapshot_repo(repo_dir: str, *, timeout: float = 15.0) -> Result:
    """Read HEAD, dirty flag, and resolved path from a git checkout."""
    path = Path(repo_dir)
    try:
        resolved = str(path.resolve())
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")
    git = ["git", "-C", resolved]
    try:
        sha_proc = subprocess.run(
            git + ["rev-parse", "HEAD"],
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if sha_proc.returncode != 0:
            err = (sha_proc.stderr or sha_proc.stdout or "git rev-parse failed").strip()
            return Result.fail(err or "git rev-parse failed")
        sha = sha_proc.stdout.strip()
        if not sha:
            return Result.fail("git rev-parse returned an empty SHA")
        status_proc = subprocess.run(
            git + ["status", "--porcelain"],
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if status_proc.returncode != 0:
            err = (status_proc.stderr or "git status failed").strip()
            return Result.fail(err or "git status failed")
        dirty = bool(status_proc.stdout.strip())
        return Result.success({"sha": sha, "dirty": dirty, "path": resolved})
    except FileNotFoundError:
        return Result.fail("git is not on PATH")
    except (OSError, subprocess.TimeoutExpired) as exc:
        return Result.fail(f"state unavailable: {exc}")


def _history_path() -> Path:
    return state_dir() / "runs.jsonl"


def _history_lock_path() -> Path:
    return state_dir() / "runs.jsonl.lock"


def append_history(row: dict) -> Result:
    path = _history_path()
    try:
        line = json.dumps(row, separators=(",", ":"), ensure_ascii=False) + "\n"
        data = line.encode("utf-8")
        if len(data) > HISTORY_ROW_MAX:
            return Result.fail("history row exceeds 4 KiB")
        with _exclusive_lock(_history_lock_path()):
            _ensure_dir(path.parent)
            fd = os.open(
                str(path),
                os.O_WRONLY | os.O_CREAT | os.O_APPEND,
                FILE_MODE,
            )
            try:
                os.write(fd, data)
            finally:
                os.close(fd)
            _chmod_file(path)
            _trim_history_unlocked(path)
        return Result.success()
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")
    except (TypeError, ValueError) as exc:
        return Result.fail(f"state unavailable: {exc}")


def read_history() -> Result:
    path = _history_path()
    rows: list[dict] = []
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return Result.success(rows)
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")
    for line in raw.splitlines():
        if not line.strip():
            continue
        try:
            parsed = json.loads(line.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
            continue
        if isinstance(parsed, dict):
            rows.append(parsed)
    return Result.success(rows)


def _trim_history_unlocked(path: Path) -> None:
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return
    lines = raw.splitlines(keepends=True)
    if len(lines) <= HISTORY_SOFT_LIMIT:
        return
    keep = b"".join(lines[-HISTORY_KEEP:])
    _atomic_write(path, keep)


def trim_history() -> Result:
    path = _history_path()
    try:
        with _exclusive_lock(_history_lock_path()):
            _trim_history_unlocked(path)
        return Result.success()
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")


def allocate_log(run_id: str, step: str) -> Result:
    if "/" in run_id or "/" in step or not run_id or not step:
        return Result.fail("invalid run id or step")
    logs_dir = state_dir() / "logs"
    path = logs_dir / f"{run_id}-{step}.log"
    try:
        _ensure_dir(logs_dir)
        path.touch(mode=FILE_MODE, exist_ok=True)
        _chmod_file(path)
        _prune_logs(logs_dir, step)
        return Result.success(path)
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")


def _prune_logs(logs_dir: Path, step: str) -> None:
    suffix = f"-{step}.log"
    try:
        matches = [p for p in logs_dir.iterdir() if p.is_file() and p.name.endswith(suffix)]
    except OSError:
        return
    matches.sort(key=lambda p: (p.stat().st_mtime, p.name))
    extra = len(matches) - LOGS_KEEP_PER_STEP
    if extra <= 0:
        return
    for path in matches[:extra]:
        try:
            path.unlink()
        except OSError:
            pass


def allocate_run_dir(run_id: str) -> Result:
    if "/" in run_id or not run_id:
        return Result.fail("invalid run id")
    runs = state_dir() / "runs"
    path = runs / run_id
    try:
        _ensure_dir(path)
        _prune_run_dirs(runs)
        return Result.success(path)
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")


def _prune_run_dirs(runs: Path) -> None:
    try:
        dirs = [p for p in runs.iterdir() if p.is_dir()]
    except OSError:
        return
    dirs.sort(key=lambda p: (p.stat().st_mtime, p.name))
    extra = len(dirs) - RUN_DIRS_KEEP
    if extra <= 0:
        return
    for path in dirs[:extra]:
        _rmtree(path)


def _rmtree(path: Path) -> None:
    try:
        if path.is_dir() and not path.is_symlink():
            for child in path.iterdir():
                _rmtree(child)
            path.rmdir()
        else:
            path.unlink()
    except OSError:
        pass


def append_crash(text: str) -> Result:
    path = state_dir() / "crash.log"
    try:
        _ensure_dir(path.parent)
        payload = text if text.endswith("\n") else text + "\n"
        data = payload.encode("utf-8", errors="replace")
        fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, FILE_MODE)
        try:
            os.write(fd, data)
            try:
                size = os.fstat(fd).st_size
            except OSError:
                size = 0
        finally:
            os.close(fd)
        _chmod_file(path)
        if size > CRASH_LOG_MAX:
            _trim_crash(path)
        return Result.success(path)
    except OSError as exc:
        return Result.fail(f"state unavailable: {exc}")


def _trim_crash(path: Path) -> None:
    try:
        raw = path.read_bytes()
    except OSError:
        return
    if len(raw) <= CRASH_LOG_MAX:
        return
    tail = raw[-CRASH_LOG_MAX:]
    nl = tail.find(b"\n")
    if 0 <= nl < len(tail) - 1:
        tail = tail[nl + 1 :]
    try:
        _atomic_write(path, tail)
    except OSError:
        pass

from __future__ import annotations

import atexit
import os
import selectors
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Optional

from install_ui import estimate, history, parsers, state, term


SPINNER = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"


class _PhaseClock:
    def __init__(self, phases: tuple[str, ...], started: float) -> None:
        self.phases = phases
        self.index = 0
        self.started = started
        self.starts = {phases[0]: started}
        self.ends = {}
        self.waits = {name: 0.0 for name in phases}
        self.wait_started: Optional[float] = None

    def _close_wait(self, at: float) -> None:
        if self.wait_started is not None:
            name = self.phases[self.index]
            self.waits[name] += max(0.0, at - self.wait_started)
            self.wait_started = None

    def set_waiting(self, waiting: bool, at: float) -> None:
        if waiting and self.wait_started is None:
            self.wait_started = at
        elif not waiting:
            self._close_wait(at)

    def advance(self, phase: Optional[str], at: float) -> None:
        if phase not in self.phases:
            return
        next_index = self.phases.index(phase)
        if next_index <= self.index:
            return
        self._close_wait(at)
        self.ends[self.phases[self.index]] = at
        for index in range(self.index + 1, next_index):
            name = self.phases[index]
            self.starts[name] = at
            self.ends[name] = at
        self.index = next_index
        self.starts[phase] = at

    def finish(self, at: float) -> tuple[dict[str, float], dict[str, float]]:
        self._close_wait(at)
        current = self.phases[self.index]
        self.ends[current] = at
        for index in range(self.index + 1, len(self.phases)):
            name = self.phases[index]
            self.starts[name] = at
            self.ends[name] = at
        durations = {}
        for phase in self.phases:
            start = self.starts.get(phase)
            end = self.ends.get(phase)
            raw = max(0.0, end - start) if start is not None and end is not None else 0.0
            durations[phase] = max(0.0, raw - self.waits[phase])
        return durations, dict(self.waits)


def _touch_started(path_value: Optional[str]) -> None:
    if not path_value:
        return
    path = Path(path_value)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.touch(mode=0o600, exist_ok=True)
    try:
        path.chmod(0o600)
    except OSError:
        pass


def _write_rows_file(path_value: Optional[str], count: int) -> None:
    if not path_value:
        return
    try:
        path = Path(path_value)
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.write_text(f"{count}\n", encoding="utf-8")
    except OSError:
        pass


def _rows_for_line(line: str, cols: int) -> int:
    return max(1, (term.cell_width(line) + max(1, cols) - 1) // max(1, cols))


def _status_label(parser: str, phase: str, message: str, lock_wait: bool, crate: str) -> str:
    if lock_wait:
        return "waiting for another build to release the target directory"
    if parser == "cargo":
        if phase == "index":
            return term.sanitize(message.strip().lower()) or "updating crates.io index"
        if phase == "deps":
            if message.startswith("Compiling "):
                pieces = message.split()
                if len(pieces) > 1:
                    return "compiling " + pieces[1]
            return "compiling dependencies"
        if phase == "crate":
            return "compiling " + crate
        return "installing bob"
    if phase == "app":
        return "building Bob Mac Capture app"
    if phase == "helper":
        return "building Bob Mac Capture helper"
    return "signing and installing Bob Mac Capture"


def _make_frame(
    *,
    parser: str,
    crate: str,
    phase: str,
    message: str,
    lock_wait: bool,
    elapsed: float,
    counter: Optional[tuple[int, int]],
    eta_text: str,
    kind: str,
    spinner_index: int,
    cols: int,
) -> str:
    label = _status_label(parser, phase, message, lock_wait, crate)
    if parser == "cargo" and phase == "deps" and counter:
        label += f" {counter[0]}/{counter[1]}"
    parts = [SPINNER[spinner_index % len(SPINNER)], label, term.format_clock(elapsed)]
    if eta_text:
        parts.append(eta_text)
    if kind == "deps":
        parts.append("Cargo.lock changed, expect a longer build")
    plain = " · ".join((parts[0], parts[1] + " · " + parts[2], *parts[3:]))
    width = max(1, min(cols - 1, 100))
    clipped = term.truncate_cells(plain, width)
    spinner = parts[0]
    if clipped.startswith(spinner) and term.use_color(sys.stdout):
        clipped = term.paint("36", spinner) + clipped[len(spinner) :]
    return clipped


def _failure_excerpt(log_path: Optional[Path]) -> list[str]:
    if log_path is None:
        return []
    try:
        lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    start = next((i for i, line in enumerate(lines) if "error" in line.lower()), None)
    if start is not None:
        return lines[start : start + 40]
    return lines[-20:]


def _paint_error_line(line: str) -> str:
    low = line.lower()
    if "error" in low:
        return term.paint("1;31", line, stream=sys.stdout)
    if "warning" in low:
        return term.paint("33", line, stream=sys.stdout)
    return term.paint("2", line, stream=sys.stdout)


def _human_path(path: Path) -> str:
    try:
        return "~" + str(path).replace(str(Path.home()), "", 1) if str(path).startswith(str(Path.home())) else str(path)
    except Exception:
        return str(path)


def run(args) -> int:
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise ValueError("run requires a command after --")

    parser_name = args.parser
    clock = getattr(args, "clock", time.monotonic)
    wall_clock = getattr(args, "wall_clock", time.time)
    repo = str(Path(args.repo).expanduser().resolve())
    step = args.step
    profile = args.profile or ("local-install" if parser_name == "cargo" else "release")
    crate = args.crate or "bob-obsidian"
    run_key = history.run_id()
    toolchain = history.probe_toolchain(step, repo)
    kind = history.predict_kind(step, repo, profile, toolchain=toolchain)
    incremental = history.incremental_for(step)
    host = history.host_name()
    key = {
        "host": host,
        "toolchain": toolchain,
        "profile": profile,
        "incremental": incremental,
    }
    learned = estimate.estimate(step, kind, key, 0.0, "")
    child_env = os.environ.copy()
    if parser_name == "cargo":
        child_env["CARGO_TERM_PROGRESS_WHEN"] = "always"
        child_env["CARGO_TERM_PROGRESS_WIDTH"] = "80"
    detector = parsers.PhaseDetector(parser_name, crate=crate)
    is_live = bool(sys.stdout.isatty()) and os.environ.get("TERM") != "dumb"
    requested_signal: Optional[int] = None
    original_handlers = {}
    proc: Optional[subprocess.Popen] = None

    def _handle_forward(signum, _frame) -> None:
        nonlocal requested_signal
        if requested_signal is None:
            requested_signal = signum
        # Keyboard interrupts arrive for the whole foreground process group, so
        # the child already receives SIGINT. Avoid sending it twice while it is
        # handling the first interrupt. TERM and HUP may target only this runner.
        if signum == signal.SIGINT:
            return
        try:
            if proc is not None and proc.poll() is None:
                proc.send_signal(signum)
        except OSError:
            pass

    if is_live:
        for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            try:
                original_handlers[sig] = signal.getsignal(sig)
                signal.signal(sig, _handle_forward)
            except (ValueError, OSError):
                pass

    try:
        proc = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=child_env,
            bufsize=0,
        )
    except OSError as exc:
        sys.stderr.write(f"install_ui: could not start build: {exc}\n")
        for sig, handler in original_handlers.items():
            try:
                signal.signal(sig, handler)
            except (ValueError, OSError):
                pass
        return 1

    if requested_signal is not None:
        try:
            proc.send_signal(requested_signal)
        except OSError:
            pass

    try:
        _touch_started(args.started_file)
    except OSError as exc:
        # The child already exists, so never return a pre-start failure that could
        # cause the shell shim to launch the same install a second time.
        try:
            sys.stderr.write(f"install_ui: could not write started marker: {exc}\n")
        except Exception:
            pass

    run_started_mono = clock()
    run_started_wall = wall_clock()
    phase_clock = _PhaseClock(detector.phases, run_started_mono)
    selector = selectors.DefaultSelector()
    assert proc.stdout is not None
    selector.register(proc.stdout, selectors.EVENT_READ)

    log_path: Optional[Path] = None
    log_handle = None
    if is_live:
        allocated = state.allocate_log(run_key, step)
        if allocated.ok and isinstance(allocated.value, Path):
            log_path = allocated.value
            try:
                log_handle = open(log_path, "ab", buffering=0)
            except OSError:
                log_path = None

    passthrough = not is_live
    cursor_hidden = False
    lines_in_scrollback = 0
    line_buffer = bytearray()
    last_message = ""
    current_phase = detector.phases[0]
    current_counter: Optional[tuple[int, int]] = None
    lock_wait = False
    spinner_index = 0
    last_frame = 0.0
    display_broken = False
    if is_live:
        cursor_hidden = True
        try:
            sys.stdout.write("\033[?25l")
            sys.stdout.flush()
        except Exception:
            passthrough = True
            display_broken = True

    def _restore_cursor() -> None:
        nonlocal cursor_hidden
        if cursor_hidden:
            try:
                sys.stdout.write("\r\033[K\033[?25h")
                sys.stdout.flush()
            except Exception:
                pass
            cursor_hidden = False

    atexit.register(_restore_cursor)

    def _print_raw(data: bytes) -> None:
        try:
            stream = getattr(sys.stdout, "buffer", None)
            if stream is not None:
                stream.write(data)
                stream.flush()
            else:
                sys.stdout.write(data.decode("utf-8", errors="replace"))
                sys.stdout.flush()
        except Exception:
            pass

    def _print_scrollback(text: str) -> None:
        nonlocal lines_in_scrollback, display_broken, cursor_hidden, passthrough
        if passthrough:
            _print_raw((text + "\n").encode("utf-8", errors="replace"))
            return
        try:
            cols, _ = term.terminal_size(sys.stdout)
            sys.stdout.write("\r\033[K" + text + "\n")
            sys.stdout.flush()
            lines_in_scrollback += _rows_for_line(text, cols)
        except Exception:
            display_broken = True
            passthrough = True
            _print_raw((text + "\n").encode("utf-8", errors="replace"))
            if cursor_hidden:
                try:
                    sys.stdout.write("\033[?25h")
                    sys.stdout.flush()
                except Exception:
                    pass
                cursor_hidden = False

    def _process_line(raw_line: bytes) -> None:
        nonlocal last_message, current_phase, current_counter, lock_wait
        line = raw_line.decode("utf-8", errors="replace")
        parsed = detector.parse(line)
        now = clock()
        if parsed.lock_wait:
            lock_wait = True
            phase_clock.set_waiting(True, now)
            last_message = line.strip()
            return
        if lock_wait:
            phase_clock.set_waiting(False, now)
            lock_wait = False
        if parsed.phase:
            phase_clock.advance(parsed.phase, now)
            current_phase = detector.phases[phase_clock.index]
        if parser_name == "cargo":
            if line.lstrip().startswith("Building ["):
                current_counter = parsers.progress_counter(line)
                if crate and crate in line:
                    phase_clock.advance("crate", now)
                    current_phase = detector.phases[phase_clock.index]
            else:
                current_counter = parsers.progress_counter(line) or current_counter
        if parsed.chatter:
            last_message = line.strip()
            return
        _print_scrollback(line)

    def _draw(now: float) -> None:
        nonlocal spinner_index, last_frame, cursor_hidden, display_broken, passthrough
        if not is_live or passthrough or display_broken or now - last_frame < 0.1:
            return
        try:
            cols, _ = term.terminal_size(sys.stdout)
            frame = _make_frame(
                parser=parser_name,
                crate=crate,
                phase=current_phase,
                message=last_message,
                lock_wait=lock_wait,
                elapsed=now - run_started_mono,
                counter=current_counter,
                eta_text=learned.text,
                kind=kind,
                spinner_index=spinner_index,
                cols=cols,
            )
            sys.stdout.write("\r\033[K" + frame)
            sys.stdout.flush()
            cursor_hidden = True
            spinner_index += 1
            last_frame = now
        except Exception:
            display_broken = True
            passthrough = True
            if line_buffer:
                _print_raw(bytes(line_buffer))
                line_buffer.clear()
            if cursor_hidden:
                try:
                    sys.stdout.write("\033[?25h")
                    sys.stdout.flush()
                except Exception:
                    pass
                cursor_hidden = False

    try:
        while True:
            events = selector.select(0.1)
            for key_event, _ in events:
                try:
                    data = os.read(key_event.fd, 8192)
                except OSError:
                    data = b""
                if not data:
                    try:
                        selector.unregister(key_event.fileobj)
                    except Exception:
                        pass
                    continue
                if log_handle is not None:
                    try:
                        view = memoryview(data)
                        while view:
                            written = log_handle.write(view)
                            if not written:
                                break
                            view = view[written:]
                    except OSError:
                        pass
                if passthrough:
                    _print_raw(data)
                    continue
                for byte_index, byte in enumerate(data):
                    if byte in (10, 13):
                        if line_buffer:
                            _process_line(bytes(line_buffer))
                            line_buffer.clear()
                            if passthrough:
                                _print_raw(data[byte_index + 1 :])
                                break
                    else:
                        line_buffer.append(byte)
                _draw(clock())
            now = clock()
            _draw(now)
            if proc.poll() is not None and not selector.get_map():
                break
        if line_buffer:
            _process_line(bytes(line_buffer))
            line_buffer.clear()
        returncode = proc.wait()
    finally:
        try:
            selector.close()
        except Exception:
            pass
        try:
            if proc.poll() is None:
                proc.wait()
        except Exception:
            pass
        _restore_cursor()
        for sig, handler in original_handlers.items():
            try:
                signal.signal(sig, handler)
            except (ValueError, OSError):
                pass
        if log_handle is not None:
            try:
                log_handle.close()
            except OSError:
                pass

    if not is_live:
        return returncode if returncode >= 0 else 128 + (-returncode)

    finished = clock()
    durations, waits = phase_clock.finish(finished)
    signal_num = requested_signal or (-returncode if returncode < 0 else None)
    interrupted = signal_num in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
    outcome = "interrupted" if interrupted else ("ok" if returncode == 0 else "fail")
    wall_elapsed = wall_clock() - run_started_wall
    mono_elapsed = finished - run_started_mono
    slept = wall_elapsed - mono_elapsed > 30.0
    history.append_step_rows(
        step=step,
        phases=detector.phases,
        durations=durations,
        waits=waits,
        outcome=outcome,
        kind=kind,
        toolchain=toolchain,
        profile=profile,
        incremental=incremental,
        run=run_key,
        slept=slept,
    )

    try:
        cols, _ = term.terminal_size(sys.stdout)
    except Exception:
        cols, _ = 80, 24
    if interrupted:
        line = f"✗ interrupted after {term.format_clock(mono_elapsed)}"
        try:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()
            lines_in_scrollback += _rows_for_line(line, cols)
        except Exception:
            pass
    elif returncode == 0:
        line = f"✓ {term.sanitize(args.done or 'built')} · {term.format_duration(mono_elapsed)}"
        try:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()
            lines_in_scrollback += _rows_for_line(line, cols)
        except Exception:
            pass
    else:
        line = f"✗ build failed after {term.format_clock(mono_elapsed)} · exit {returncode}"
        try:
            sys.stdout.write(line + "\n")
            lines_in_scrollback += _rows_for_line(line, cols)
            excerpt = _failure_excerpt(log_path)
            for output_line in excerpt:
                rendered = _paint_error_line(output_line)
                sys.stdout.write("    " + rendered + "\n")
                lines_in_scrollback += _rows_for_line("    " + output_line, cols)
            if log_path:
                sys.stdout.write("    full log  " + _human_path(log_path) + "\n")
                sys.stdout.write("    rerun with  just install-all -v  to stream build output\n")
                lines_in_scrollback += 2
            sys.stdout.flush()
        except Exception:
            pass
    _write_rows_file(args.rows_file, lines_in_scrollback)

    if interrupted:
        signum = int(signal_num or signal.SIGINT)
        try:
            signal.signal(signum, signal.SIG_DFL)
            os.kill(os.getpid(), signum)
        except OSError:
            return 128 + signum
        return 128 + signum
    return returncode if returncode >= 0 else 128 + (-returncode)

from __future__ import annotations

import atexit
import os
import re
import selectors
import signal
import subprocess
import sys
import time
import traceback
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


def _rows_file_path(explicit: Optional[str], step: str) -> Optional[str]:
    if explicit:
        return explicit
    run = os.environ.get("BOB_INSTALL_RUN_ID")
    if not run:
        return None
    return str(state.state_dir() / "runs" / run / f"rows-{step}")


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
            verb = message.strip().split(None, 1)[0] if message.strip() else ""
            if verb in ("Updating", "Fetch"):
                return "updating crates.io index"
            if verb == "Locking":
                return "resolving dependencies"
            if verb in ("Downloading", "Downloaded"):
                return "downloading crates"
            return "preparing build"
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
    bar: str = "",
    bar_color: str = "35",
    eta_color: Optional[str] = None,
) -> str:
    label = _status_label(parser, phase, message, lock_wait, crate)
    if parser == "cargo" and phase == "deps" and counter:
        label += f" {counter[0]}/{counter[1]}"
    spinner = SPINNER[spinner_index % len(SPINNER)]
    clock = term.format_clock(elapsed)
    width = max(1, min(cols - 1, 100))
    show_bar = bool(bar) and width >= estimate.BAR_MIN_COLS
    if show_bar:
        body = f"{label}  {bar}  {clock}"
    else:
        body = f"{label} · {clock}"
    tail = []
    if eta_text:
        tail.append(eta_text)
    if kind == "deps":
        tail.append("Cargo.lock changed, expect a longer build")
    plain = "  " + " · ".join((f"{spinner} {body}", *tail))
    clipped = term.truncate_cells(plain, width)
    colorize = term.use_color(sys.stdout)
    if clipped.startswith(spinner) and colorize:
        clipped = term.paint("36", spinner) + clipped[len(spinner) :]
    if show_bar and bar in clipped and colorize:
        clipped = clipped.replace(bar, estimate.paint_bar(bar, bar_color, colorize=True), 1)
    if eta_color and eta_text and eta_text in clipped and colorize:
        clipped = clipped.replace(eta_text, term.paint(eta_color, eta_text), 1)
    return clipped


_REAL_ERROR_RE = re.compile(r"(?:^|[\s:])error(?:\[[^\]]*\])?:")


def _failure_excerpt(log_path: Optional[Path]) -> list[str]:
    if log_path is None:
        return []
    try:
        lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    start = next((i for i, line in enumerate(lines) if _REAL_ERROR_RE.search(line)), None)
    if start is not None:
        return lines[start : start + 40]
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


def _run_spawned(context: dict) -> int:
    args = context["args"]
    proc = context["proc"]
    original_handlers = context["original_handlers"]
    signal_state = context["signal_state"]
    is_live = context["is_live"]
    parser_name = context["parser_name"]
    clock = context["clock"]
    wall_clock = context["wall_clock"]
    repo = context["repo"]
    step = context["step"]
    profile = context["profile"]
    crate = context["crate"]
    run_key = context["run_key"]
    toolchain = context["toolchain"]
    kind = context["kind"]
    incremental = context["incremental"]
    host = context["host"]
    key = context["key"]
    detector = context["detector"]
    eta = context["eta"]
    eta_text = context["eta_text"]
    eta_fill = context["eta_fill"]
    eta_changed_at = context["eta_changed_at"]
    bar_color = context["bar_color"]
    child_env = context["child_env"]
    if signal_state["value"] is not None:
        try:
            proc.send_signal(signal_state["value"])
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
    eta_changed_at = run_started_mono
    phase_clock = _PhaseClock(detector.phases, run_started_mono)
    selector = selectors.DefaultSelector()
    assert proc.stdout is not None
    selector.register(proc.stdout, selectors.EVENT_READ)
    context["selector"] = selector

    log_path: Optional[Path] = None
    log_handle = None
    context["log_handle"] = log_handle
    if is_live:
        allocated = state.allocate_log(run_key, step)
        if allocated.ok and isinstance(allocated.value, Path):
            log_path = allocated.value
            try:
                log_handle = open(log_path, "ab", buffering=0)
                context["log_handle"] = log_handle
            except OSError:
                log_path = None

    passthrough = not is_live
    cursor_hidden = False
    lines_in_scrollback = 0
    line_buffer = bytearray()
    context["line_buffer"] = line_buffer
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

    def _refresh_eta(now: float) -> estimate.Estimate:
        nonlocal eta, eta_text, eta_fill, eta_changed_at
        phase_started = phase_clock.starts.get(current_phase, run_started_mono)
        refreshed = estimate.estimate(
            step,
            kind,
            key,
            now - run_started_mono,
            current_phase,
            phase_elapsed=now - phase_started,
            phases=detector.phases,
            prev_fill=eta_fill,
        )
        eta_fill = refreshed.fill
        if estimate.should_update_eta_text(eta_text, refreshed.text, eta_changed_at, now):
            eta_text = refreshed.text
            eta_changed_at = now
        eta = refreshed
        return refreshed

    def _draw(now: float) -> None:
        nonlocal spinner_index, last_frame, cursor_hidden, display_broken, passthrough
        if not is_live or passthrough or display_broken or now - last_frame < 0.1:
            return
        try:
            cols, _ = term.terminal_size(sys.stdout)
            _refresh_eta(now)
            bar = ""
            if eta_fill is not None and cols >= estimate.BAR_MIN_COLS:
                bar = estimate.render_bar(eta_fill)
            eta_color = "33" if eta.rung == "overtime" else None
            frame = _make_frame(
                parser=parser_name,
                crate=crate,
                phase=current_phase,
                message=last_message,
                lock_wait=lock_wait,
                elapsed=now - run_started_mono,
                counter=current_counter,
                eta_text=eta_text,
                kind=kind,
                spinner_index=spinner_index,
                cols=cols,
                bar=bar,
                bar_color=bar_color,
                eta_color=eta_color,
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
                            try:
                                _process_line(bytes(line_buffer))
                            except Exception:
                                # This chunk has already left the pipe. Preserve
                                # its unparsed tail before the outer recovery drains
                                # the child's remaining output.
                                passthrough = True
                                display_broken = True
                                _print_raw(bytes(line_buffer) + bytes((byte,)))
                                line_buffer.clear()
                                _print_raw(data[byte_index + 1 :])
                                raise
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
        _restore_cursor()
        if sys.exc_info()[0] is None:
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
    signal_num = signal_state["value"] or (-returncode if returncode < 0 else None)
    interrupted = signal_num in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
    successful = returncode == 0 or returncode in getattr(args, "success_code", [])
    outcome = "interrupted" if interrupted else ("ok" if successful else "fail")
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
        if os.environ.get("BOB_INSTALL_ALL_INTERRUPT_PARENT") != "1":
            line = f"  ✗ interrupted after {term.format_clock(mono_elapsed)}"
            try:
                sys.stdout.write(line + "\n")
                sys.stdout.flush()
                lines_in_scrollback += _rows_for_line(line, cols)
            except Exception:
                pass
    elif successful:
        done_est = estimate.estimate(
            step,
            kind,
            key,
            mono_elapsed,
            detector.phases[-1],
            phase_elapsed=durations.get(detector.phases[-1], 0.0),
            phases=detector.phases,
            prev_fill=eta_fill,
        )
        line = f"  ✓ {term.sanitize(args.done or 'built')} · {term.format_duration(mono_elapsed)}"
        suffix = estimate.final_suffix(mono_elapsed, done_est)
        if suffix:
            line += f" · {suffix}"
        try:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()
            lines_in_scrollback += _rows_for_line(line, cols)
        except Exception:
            pass
    else:
        line = f"  ✗ build failed after {term.format_clock(mono_elapsed)} · exit {returncode}"
        try:
            sys.stdout.write(line + "\n")
            lines_in_scrollback += _rows_for_line(line, cols)
            excerpt = _failure_excerpt(log_path)
            for output_line in excerpt:
                rendered = _paint_error_line(output_line)
                sys.stdout.write("      " + rendered + "\n")
                lines_in_scrollback += _rows_for_line("      " + output_line, cols)
            if log_path:
                sys.stdout.write("      full log  " + _human_path(log_path) + "\n")
                if os.environ.get("BOB_INSTALL_ALL") == "1":
                    hint = "      rerun with  just install-all -v  to stream build output\n"
                else:
                    hint = "      rerun with  BOB_INSTALL_VERBOSE=1 just install  to stream build output\n"
                sys.stdout.write(hint)
                lines_in_scrollback += 2
            sys.stdout.flush()
        except Exception:
            pass
    _write_rows_file(_rows_file_path(args.rows_file, step), lines_in_scrollback)

    if interrupted:
        signum = int(signal_num or signal.SIGINT)
        try:
            signal.signal(signum, signal.SIG_DFL)
            os.kill(os.getpid(), signum)
        except OSError:
            return 128 + signum
        return 128 + signum
    return returncode if returncode >= 0 else 128 + (-returncode)


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
    detector = parsers.PhaseDetector(parser_name, crate=crate)
    eta = estimate.estimate(
        step, kind, key, 0.0, detector.phases[0], phases=detector.phases
    )
    eta_text = eta.text
    eta_fill = eta.fill
    eta_changed_at = 0.0
    bar_color = estimate.STEP_COLORS.get(step, "35")
    child_env = os.environ.copy()
    if parser_name == "cargo":
        child_env["CARGO_TERM_PROGRESS_WHEN"] = "always"
        child_env["CARGO_TERM_PROGRESS_WIDTH"] = "80"
    is_live = bool(sys.stdout.isatty()) and os.environ.get("TERM") != "dumb"
    signal_state = {"value": None}
    original_handlers = {}
    proc: Optional[subprocess.Popen] = None

    def _handle_forward(signum, _frame) -> None:
        if signal_state["value"] is None:
            signal_state["value"] = signum
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

    context = locals().copy()
    try:
        return _run_spawned(context)
    except Exception:
        try:
            state.append_crash(traceback.format_exc())
        except Exception:
            pass

        # Stop drawing before passing the child's remaining bytes through. The
        # normal loop's finally handles this after mid-loop faults; this also
        # covers failures during post-spawn setup.
        if is_live:
            try:
                sys.stdout.write("\r\033[K\033[?25h")
                sys.stdout.flush()
            except Exception:
                pass

        pending = context.get("line_buffer")
        if isinstance(pending, bytearray) and pending:
            try:
                stream = getattr(sys.stdout, "buffer", None)
                if stream is not None:
                    stream.write(bytes(pending))
                    stream.flush()
                else:
                    sys.stdout.write(pending.decode("utf-8", errors="replace"))
                    sys.stdout.flush()
            except Exception:
                pass
            pending.clear()

        output = proc.stdout
        if output is not None:
            try:
                while True:
                    data = os.read(output.fileno(), 8192)
                    if not data:
                        break
                    stream = getattr(sys.stdout, "buffer", None)
                    if stream is not None:
                        stream.write(data)
                        stream.flush()
                    else:
                        sys.stdout.write(data.decode("utf-8", errors="replace"))
                        sys.stdout.flush()
            except Exception:
                pass

        try:
            returncode = proc.wait()
        except Exception:
            returncode = proc.poll()
        selector_value = context.get("selector")
        if selector_value is not None:
            try:
                selector_value.close()
            except Exception:
                pass
        log_value = context.get("log_handle")
        if log_value is not None:
            try:
                log_value.close()
            except Exception:
                pass
        if is_live:
            try:
                sys.stdout.write("\033[?25h")
                sys.stdout.flush()
            except Exception:
                pass
        for sig, handler in original_handlers.items():
            try:
                signal.signal(sig, handler)
            except (ValueError, OSError):
                pass

        signal_num = signal_state["value"] or (-returncode if returncode is not None and returncode < 0 else None)
        if signal_num in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            try:
                signal.signal(int(signal_num), signal.SIG_DFL)
                os.kill(os.getpid(), int(signal_num))
            except OSError:
                return 128 + int(signal_num)
            return 128 + int(signal_num)
        if returncode is None:
            return 1
        return returncode if returncode >= 0 else 128 + (-returncode)

from __future__ import annotations

import os
import re
import socket
import sys
import unicodedata
from typing import Optional, TextIO

BANNER_RULE = "────────────────────────────────────────────────"
_ELLIPSIS = "…"
_CSI_RE = re.compile(r"\033\[[0-?]*[ -/]*[@-~]")
_MONTHS = (
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
)


def use_color(stream: Optional[TextIO] = None) -> bool:
    """True when *stream* is a TTY, NO_COLOR is unset, and TERM is not dumb."""
    if "NO_COLOR" in os.environ:
        return False
    if os.environ.get("TERM") == "dumb":
        return False
    target = sys.stdout if stream is None else stream
    isatty = getattr(target, "isatty", None)
    try:
        return bool(isatty()) if callable(isatty) else False
    except Exception:
        return False


def paint(
    color: object,
    value: str,
    *,
    stream: Optional[TextIO] = None,
    colorize: Optional[bool] = None,
) -> str:
    """Wrap *value* in an ANSI SGR sequence when color is enabled."""
    if colorize is None:
        colorize = use_color(sys.stdout if stream is None else stream)
    if not colorize:
        return value
    return f"\033[{color}m{value}\033[0m"


def banner(
    color: object,
    icon: str,
    title: str,
    *,
    stream: Optional[TextIO] = None,
    colorize: Optional[bool] = None,
) -> str:
    """Return the install_all banner bytes; write them when *stream* is set."""
    if colorize is None:
        colorize = use_color(sys.stdout if stream is None else stream)
    if colorize:
        text = (
            f"\n{icon}  \033[1;{color}m{title}\033[0m\n"
            f"\033[{color}m{BANNER_RULE}\033[0m\n"
        )
    else:
        text = f"\n{icon}  {title}\n{BANNER_RULE}\n"
    if stream is not None:
        stream.write(text)
        stream.flush()
    return text


def strip_ansi(text: str) -> str:
    return _CSI_RE.sub("", text)


def _char_width(ch: str) -> int:
    if unicodedata.combining(ch):
        return 0
    if unicodedata.east_asian_width(ch) in ("W", "F"):
        return 2
    return 1


def cell_width(text: str) -> int:
    """Visible terminal cells: wide/fullwidth=2, combining marks=0, else 1."""
    return sum(_char_width(ch) for ch in strip_ansi(text))


def truncate_cells(text: str, width: int, ellipsis: str = _ELLIPSIS) -> str:
    """Cut *text* to *width* cells, appending *ellipsis* when truncated."""
    if width <= 0:
        return ""
    if cell_width(text) <= width:
        return text
    ellipsis_width = cell_width(ellipsis)
    if ellipsis_width > width:
        return ""
    budget = width - ellipsis_width
    out = []
    used = 0
    for ch in text:
        w = _char_width(ch)
        if used + w > budget:
            break
        out.append(ch)
        used += w
    return "".join(out) + ellipsis


def wrap_cells(text: str, width: int) -> list[str]:
    """Wrap *text* to lines of at most *width* cells."""
    if width <= 0:
        width = 1
    lines: list[str] = []
    for para in text.split("\n"):
        lines.extend(_wrap_paragraph(para, width))
    return lines


def _wrap_paragraph(para: str, width: int) -> list[str]:
    words = para.split()
    if not words:
        return [""]
    lines: list[str] = []
    current = ""
    for word in words:
        pieces = _split_cells(word, width)
        for i, piece in enumerate(pieces):
            if i > 0:
                if current:
                    lines.append(current)
                current = piece
                continue
            if not current:
                current = piece
                continue
            candidate = f"{current} {piece}"
            if cell_width(candidate) <= width:
                current = candidate
            else:
                lines.append(current)
                current = piece
    if current:
        lines.append(current)
    return lines


def _split_cells(word: str, width: int) -> list[str]:
    if cell_width(word) <= width:
        return [word]
    pieces: list[str] = []
    current: list[str] = []
    used = 0
    for ch in word:
        w = _char_width(ch)
        if current and used + w > width:
            pieces.append("".join(current))
            current = [ch]
            used = w
        else:
            current.append(ch)
            used += w
    if current:
        pieces.append("".join(current))
    return pieces or [""]


def sanitize(text: str) -> str:
    """Make *text* inert: tabs to spaces, C0/C1/DEL to caret notation."""
    out: list[str] = []
    for ch in text:
        code = ord(ch)
        if ch == "\t":
            out.append(" ")
        elif code == 0x7F:
            out.append("^?")
        elif code < 0x20:
            out.append("^" + chr(code + 64))
        elif 0x80 <= code <= 0x9F:
            out.append("^" + chr((code - 0x80) + 64))
        else:
            out.append(ch)
    return "".join(out)


def format_duration(seconds: float) -> str:
    """Match install_all's format_duration: ``45s``, ``8m 12s``."""
    total = int(seconds)
    if total < 0:
        total = 0
    if total < 60:
        return f"{total}s"
    return f"{total // 60}m {total % 60:02d}s"


def format_clock(seconds: float) -> str:
    """Elapsed clock: ``4:52`` or ``1:02:03``."""
    total = int(seconds)
    if total < 0:
        total = 0
    hours, rem = divmod(total, 3600)
    minutes, secs = divmod(rem, 60)
    if hours:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes}:{secs:02d}"


def format_relative(when: object, now: Optional[object] = None) -> str:
    """Relative time: ``51m ago``, ``3h ago``, ``yesterday 23:31``, ``Nov 2``."""
    from datetime import datetime, timedelta

    when_dt = _as_datetime(when)
    now_dt = datetime.now().astimezone() if now is None else _as_datetime(now)
    if when_dt.tzinfo is None:
        when_dt = when_dt.replace(tzinfo=now_dt.tzinfo)
    if now_dt.tzinfo is None:
        now_dt = now_dt.replace(tzinfo=when_dt.tzinfo)
    when_dt = when_dt.astimezone(now_dt.tzinfo)
    delta = now_dt - when_dt
    secs = int(delta.total_seconds())
    if secs < 0:
        secs = 0
    if secs < 60:
        return "just now"
    if secs < 3600:
        return f"{secs // 60}m ago"
    if when_dt.date() == now_dt.date():
        return f"{max(secs // 3600, 1)}h ago"
    if when_dt.date() == (now_dt.date() - timedelta(days=1)):
        return f"yesterday {when_dt.strftime('%H:%M')}"
    month = _MONTHS[when_dt.month - 1]
    if when_dt.year == now_dt.year:
        return f"{month} {when_dt.day}"
    return f"{month} {when_dt.day} {when_dt.year}"


def _as_datetime(value: object):
    from datetime import datetime

    if isinstance(value, datetime):
        return value
    raise TypeError("format_relative expected a datetime")


def terminal_size(stream: Optional[TextIO] = None) -> tuple[int, int]:
    """Columns and lines: the stream, then COLUMNS/LINES, then 80×24."""
    target = sys.stdout if stream is None else stream
    fileno = getattr(target, "fileno", None)
    if callable(fileno):
        try:
            size = os.get_terminal_size(fileno())
            cols, lines = size.columns, size.lines
            if cols > 0 and lines > 0:
                return cols, lines
        except (OSError, ValueError, AttributeError, TypeError):
            pass
    return (_env_positive("COLUMNS", 80), _env_positive("LINES", 24))


def _env_positive(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def host_label() -> str:
    """``this Mac`` on Darwin; otherwise the short hostname."""
    if sys.platform == "darwin":
        return "this Mac"
    return socket.gethostname().split(".")[0]

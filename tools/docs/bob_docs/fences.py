"""Static, accessible ``bob-task`` fence for documentation examples.

Supported headers (verified in Zensical and Material)::

    ```bob-task
    ```bob-task today

``today`` is an explicit illustration annotation for open task rows. It is
never inferred from dates or lanes.
"""

from __future__ import annotations

import html
import re

STATUS_MARKS: dict[str, tuple[str, str]] = {
    " ": ("○", "Ready"),
    "*": ("●", "Next"),
    "/": ("◑", "In Progress"),
    "?": ("?", "Blocked"),
    "x": ("✓", "Done"),
    "X": ("✓", "Done"),
    "-": ("⊘", "Cancelled"),
}

OPEN_STATUSES = {" ", "*", "/", "?"}

DATE_FIELDS = {
    "created": ("+", "Created"),
    "scheduled": ("⧗", "Scheduled"),
    "completion": ("✓", "Done"),
    "cancelled": ("⊘", "Cancelled"),
}

MONTHS = {
    1: "Jan",
    2: "Feb",
    3: "Mar",
    4: "Apr",
    5: "May",
    6: "Jun",
    7: "Jul",
    8: "Aug",
    9: "Sep",
    10: "Oct",
    11: "Nov",
    12: "Dec",
}

TASK_LINE_RE = re.compile(r"^(\s*)([-*+]|\d+[.)])\s+\[([ \*\/\?xX-])\](.*)$")
TAG_RE = re.compile(r"(?<!\w)#([A-Za-z][A-Za-z0-9_-]*)\b")
DATE_RE = re.compile(
    r"\[((created|scheduled|completion|cancelled))::\s*(\d{4})-(\d{2})-(\d{2})\s*\]"
    r"|\(((created|scheduled|completion|cancelled))::\s*(\d{4})-(\d{2})-(\d{2})\s*\)"
)
ID_RE = re.compile(r"\[(id|dependsOn)::\s*([^\]]*)\]")
INLINE_CODE_RE = re.compile(r"`[^`\n]+`")
FIELD_COUNT_RE = re.compile(r"(^|[^A-Za-z0-9_-])(created|scheduled|completion|cancelled)\s*::", re.IGNORECASE)


def _is_leap(year: int, month: int, day: int) -> bool:
    if month != 2 or day != 29:
        return True
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)


def _valid_date(year: int, month: int, day: int) -> bool:
    if month < 1 or month > 12 or day < 1:
        return False
    if month in (1, 3, 5, 7, 8, 10, 12):
        return day <= 31
    if month in (4, 6, 9, 11):
        return day <= 30
    return _is_leap(year, month, day) and day <= 29


def _date_label(year: int, month: int, day: int) -> str:
    return f"{MONTHS[month]} {day}, {year}"


def _escape(text: str) -> str:
    return html.escape(text, quote=True)


def _render_inline(text: str, *, today: bool, is_open: bool) -> str:
    """Render one task-body segment, escaping everything user-supplied."""

    parts: list[str] = []
    pos = 0
    for match in INLINE_CODE_RE.finditer(text):
        if match.start() > pos:
            parts.append(_render_text_segment(text[pos : match.start()], today=today, is_open=is_open))
        parts.append(f"<code>{_escape(match.group(0)[1:-1])}</code>")
        pos = match.end()
    if pos < len(text):
        parts.append(_render_text_segment(text[pos:], today=today, is_open=is_open))
    return "".join(parts)


def _render_text_segment(text: str, *, today: bool, is_open: bool) -> str:
    # Split out escaped \#task so it stays literal.
    tokens: list[str] = []
    pos = 0
    for match in re.finditer(r"\\#task\b|#task\b|#[A-Za-z][A-Za-z0-9_-]*\b", text):
        if match.start() > pos:
            tokens.append(("text", text[pos : match.start()]))
        tokens.append(("tag", match.group(0)))
        pos = match.end()
    if pos < len(text):
        tokens.append(("text", text[pos:]))
    if not tokens:
        tokens.append(("text", text))
    out: list[str] = []
    for kind, value in tokens:
        if kind == "text":
            out.append(_render_dates_and_ids(_escape(value)))
        else:
            out.append(_render_tag(value, today=today, is_open=is_open))
    return "".join(out)


def _render_tag(token: str, *, today: bool, is_open: bool) -> str:
    if token.startswith("\\"):
        return _escape(token)
    if token == "#task":
        # Teal identity hash; text stays for copy, CSS masks it.
        return (
            '<span class="bob-tag bob-tag-task" role="img" aria-label="tracked task">'
            f"{_escape(token)}</span>"
        )
    # #tasks, #Task and other case/plural variants stay literal text.
    if token.lower() in ("#task", "#tasks"):
        return _escape(token)
    name = token[1:]
    cls = "bob-tag bob-tag-prj" if name == "prj" else "bob-tag"
    return f'<span class="{cls}">{_escape(token)}</span>'


def _render_dates_and_ids(escaped: str) -> str:
    # Dates first, then id/dependsOn dimming. Input is already escaped, so
    # match against escaped-safe patterns (brackets and colons are unescaped).
    # Duplicate fields are never valid dates: count per-field occurrences
    # across the whole segment first.
    counts: dict[str, int] = {}
    for m in DATE_RE.finditer(escaped):
        field = (m.group(2) or m.group(7) or "").lower()
        counts[field] = counts.get(field, 0) + 1

    def date_repl(match: re.Match[str]) -> str:
        if match.group(2):
            field, year, month, day = match.group(2), int(match.group(3)), int(match.group(4)), int(match.group(5))
            open_br, close_br = "[", "]"
        else:
            field, year, month, day = match.group(7), int(match.group(8)), int(match.group(9)), int(match.group(10))
            open_br, close_br = "(", ")"
        raw = match.group(0)
        if counts.get(field.lower(), 0) > 1:
            return _escape(raw)
        if not _valid_date(year, month, day):
            return _escape(raw)
        glyph, verb = DATE_FIELDS[field]
        label = _date_label(year, month, day)
        iso = f"{year:04d}-{month:02d}-{day:02d}"
        return (
            f'<span class="bob-date bob-date-{field}" role="img" '
            f'aria-label="{verb} {label} ({iso})" data-field="{field}" data-date="{iso}">'
            f'<span class="bob-date-glyph" aria-hidden="true">{glyph}</span> '
            f'<span class="bob-date-label">{label}</span></span>'
        )

    rendered = DATE_RE.sub(date_repl, escaped)

    def id_repl(match: re.Match[str]) -> str:
        return f'<span class="bob-id">[{match.group(1)}:: {_escape(match.group(2))}]</span>'

    return ID_RE.sub(id_repl, rendered)


def _render_task_line(line: str, *, today: bool) -> str:
    match = TASK_LINE_RE.match(line)
    if not match:
        # Unknown/unparseable lines appear as raw escaped text.
        return f'<div class="bob-task-raw">{_escape(line)}</div>' if line.strip() else '<div class="bob-task-blank">&nbsp;</div>'
    indent, _bullet, status, rest = match.group(1), match.group(2), match.group(3), match.group(4)
    glyph, name = STATUS_MARKS.get(status, ("○", "Ready"))
    is_open = status in OPEN_STATUSES
    is_done = status in ("x", "X", "-")
    classes = f"bob-task-line bob-status-{name.lower().replace(' ', '-')}"
    if is_done:
        classes += " bob-task-closed"
    status_html = (
        f'<span class="bob-status" role="img" aria-label="{name}">{glyph}</span>'
    )
    today_html = ""
    if today and is_open:
        today_html = (
            ' <span class="bob-today" role="img" aria-label="Today">🍅</span>'
        )
    body_html = _render_inline(rest, today=today, is_open=is_open)
    pad = " " * len(indent) if indent else ""
    return (
        f'<div class="{classes}">{_escape(pad)}{status_html}{today_html}'
        f'<span class="bob-task-body">{body_html}</span></div>'
    )


def render_bob_tasks(source: str, *, today: bool = False) -> str:
    lines = source.splitlines()
    rendered = "".join(_render_task_line(line, today=today) for line in lines)
    today_attr = ' data-today="true"' if today else ""
    return f'<div class="bob-task"{today_attr}>{rendered}</div>'


def fence_validator(language: str, inputs: dict[str, str], options: dict[str, object], attrs: dict[str, str], md: object) -> bool:
    if language != "bob-task":
        return False
    for key, value in inputs.items():
        if key == "today" and value in ("today", "true", "True", "1"):
            options["today"] = True
            continue
        return False
    return True


def fence_format(
    source: str,
    language: str,
    class_name: str,
    options: dict[str, object],
    md: object,
    **kwargs: object,
) -> str:
    today = bool(options.get("today")) if isinstance(options, dict) else False
    return render_bob_tasks(source, today=today)


# Backwards-compatible alias used by the assembled-config test probe.
def fence_format_alias(*args: object, **kwargs: object) -> str:  # pragma: no cover
    return fence_format(*args, **kwargs)  # type: ignore[arg-type]

from __future__ import annotations

"""The ``install_ui digest`` renderer: levels, budget, layout, and records.

``commits.py`` resolves ranges and parses commits; this module decides how
much of each commit fits on screen and paints the section.
"""

import json
import os
import subprocess
from dataclasses import dataclass, field
from typing import Optional

from install_ui import commits, term

LEVELS = ("full", "lead", "subjects", "grouped")
_LEVEL_INDEX = {name: i for i, name in enumerate(LEVELS)}
_TYPE_COLORS = {"feat": "32", "fix": "33", "perf": "36", "revert": "35"}
# Hidden-type summary order at the grouped level: user-visible groups first,
# then the folded types (each counted separately), then non-conventional.
_HIDDEN_TYPE_ORDER = (
    "feat",
    "fix",
    "perf",
    "revert",
    "docs",
    "test",
    "refactor",
    "chore",
    "ci",
    "style",
    "build",
    "other",
)
_NON_TTY_CAP = 60
_BANNER_ICON = "📰"
_BANNER_TITLE = "WHAT'S NEW"
_BANNER_RULE = "────────────────────────────────────────────────"


def _banner_lines(width: int, colorize: bool) -> list[str]:
    """Section banner; the rule shrinks below 48 columns to fit the measure."""
    rule = _BANNER_RULE
    while term.cell_width(rule) > width and len(rule) > 1:
        rule = rule[:-1]
    if colorize:
        return [
            "",
            f"{_BANNER_ICON}  \033[1;1m{_BANNER_TITLE}\033[0m",
            f"\033[1m{rule}\033[0m",
        ]
    return ["", f"{_BANNER_ICON}  {_BANNER_TITLE}", rule]


@dataclass
class RenderRepo:
    """One repo's digest input, ready to lay out."""

    name: str
    directory: str
    color_code: str = "1"
    head: str = ""
    base: str = ""
    label: str = ""
    notes: list[str] = field(default_factory=list)
    commit_list: list[commits.CommitInfo] = field(default_factory=list)
    kind: str = "none"


def budget(lines: int, steps: int) -> int:
    """Row budget: ``clamp(LINES - reserve, 8, floor(0.6 * LINES))``."""
    steps = max(int(steps), 1)
    lines = max(int(lines), 1)
    reserve = 8 + 2 + 4 * steps + (6 + steps) + 2
    raw = lines - reserve
    capped = (lines * 6) // 10
    return max(8, min(raw, capped))


def measure(cols: int) -> int:
    """Usable width: ``min(COLUMNS - 1, 100)`` floored at 20."""
    return max(20, min(max(int(cols), 1) - 1, 100))


def _pad_cells(text: str, width: int) -> str:
    pad = max(width - term.cell_width(text), 0)
    return text + (" " * pad)


def _widest_type(all_commits: list[commits.CommitInfo]) -> int:
    return max((len(commits.type_string(c)) for c in all_commits), default=0)


def _widest_scope(repo_commits: list[commits.CommitInfo]) -> int:
    widest = 0
    for commit in repo_commits:
        if commit.scope:
            widest = max(widest, term.cell_width(commit.scope[:64]))
    return min(widest, 16)


def _prefix_width(type_width: int, scope_width: int) -> int:
    return 2 + type_width + 2 + scope_width + 2


def _paint_subject_row(
    commit: commits.CommitInfo,
    type_width: int,
    scope_width: int,
    width: int,
    colorize: bool,
    show_sha: bool,
    fold_scope: bool,
) -> str:
    badge = "⚠ " if commit.breaking else "  "
    type_str = commits.type_string(commit)
    scope = commit.scope or ""
    subject = term.sanitize(commit.subject or commit.raw_subject)
    if fold_scope and scope:
        subject = term.sanitize(f"{scope}: {commit.subject or commit.raw_subject}")
        scope = ""
    scope_width_eff = 0 if fold_scope else scope_width
    scope_cell = term.truncate_cells(scope, 16) if scope else ""
    scope_cell = _pad_cells(scope_cell, scope_width_eff)
    type_cell = _pad_cells(type_str, type_width)
    prefix = badge + type_cell + "  " + scope_cell + "  "
    sha = (commit.sha or "")[:8]
    if show_sha:
        avail = max(width - _prefix_width(type_width, scope_width_eff) - 8 - 1, 1)
        short = term.truncate_cells(subject, avail)
        gap = " " * max(avail - term.cell_width(short), 0)
        if colorize:
            badge_out = term.paint("1;31", badge, colorize=True) if commit.breaking else badge
            if type_str:
                color = "1;31" if commit.breaking else _TYPE_COLORS.get(
                    commit.type, "2"
                )
                type_out = term.paint(color, type_cell, colorize=True)
            else:
                type_out = type_cell
            scope_out = term.paint("2", scope_cell, colorize=True)
            sha_out = term.paint("2", sha, colorize=True)
            return f"{badge_out}{type_out}  {scope_out}  {short}{gap} {sha_out}"
        return f"{prefix}{short}{gap} {sha}"
    avail = max(width - _prefix_width(type_width, scope_width_eff), 1)
    short = term.truncate_cells(subject, avail)
    if colorize:
        badge_out = term.paint("1;31", badge, colorize=True) if commit.breaking else badge
        if type_str:
            color = "1;31" if commit.breaking else _TYPE_COLORS.get(commit.type, "2")
            type_out = term.paint(color, type_cell, colorize=True)
        else:
            type_out = type_cell
        scope_out = term.paint("2", scope_cell, colorize=True)
        return f"{badge_out}{type_out}  {scope_out}  {short}"
    return f"{prefix}{short}"


def _sanitize_keep_breaks(text: str) -> str:
    """Sanitize like :func:`term.sanitize` but preserve ``\\n`` breaks.

    A raw carriage return or vertical tab must never become a forged row, so
    every control character except newline is folded before wrapping.
    """
    out: list[str] = []
    for char in text:
        if char == "\n":
            out.append(char)
        else:
            out.append(term.sanitize(char))
    return "".join(out)


def _cut_last_line(line: str, width: int) -> str:
    visible = term.strip_ansi(line)
    if visible.endswith("…"):
        return line
    return term.truncate_cells(visible, max(width - 1, 1)) + "…"


def _wrap_body(
    text: str, indent: int, width: int, max_rows: Optional[int] = None
) -> list[str]:
    wrap_at = min(80, width - indent)
    if wrap_at < 10:
        wrap_at = 10
    lines: list[str] = []
    for paragraph in commits.split_paragraphs(_sanitize_keep_breaks(text)):
        for chunk in paragraph.split("\n") or [""]:
            wrapped = term.wrap_cells(chunk, wrap_at) or [""]
            lines.extend((" " * indent + row) for row in wrapped)
    if max_rows is not None and len(lines) > max_rows:
        lines = lines[:max_rows]
        lines[-1] = _cut_last_line(lines[-1], width)
    return lines


def _fold_verified_row(
    paragraph: str, indent: int, width: int, colorize: bool
) -> str:
    # A folded paragraph is always one dim row ending in an ellipsis: the
    # marker proves the provenance text is one `git log` away.
    one_line = " ".join(paragraph.split())
    one_line = term.sanitize(one_line)
    avail = max(width - indent, 1)
    short = term.truncate_cells(one_line, max(avail - 1, 1))
    if not short.endswith("…"):
        short = short + "…"
    row = " " * indent + short
    return term.paint("2", row, colorize=colorize) if colorize else row


def _body_paragraphs(commit: commits.CommitInfo) -> list[str]:
    return [
        p
        for p in commits.split_paragraphs(commit.stripped_body)
        if p.strip() and p.strip() not in [v.strip() for v in commit.verified]
    ]


def _render_commit_full(
    commit: commits.CommitInfo,
    type_width: int,
    scope_width: int,
    width: int,
    colorize: bool,
    show_sha: bool,
    fold_scope: bool,
    indent: int,
) -> list[str]:
    lines = [
        _paint_subject_row(
            commit, type_width, scope_width, width, colorize, show_sha, fold_scope
        )
    ]
    for paragraph in _body_paragraphs(commit):
        for row in _wrap_body(paragraph, indent, width):
            lines.append(row)
    for verified in commit.verified:
        lines.append(_fold_verified_row(verified, indent, width, colorize))
    return lines


def _render_commit_lead(
    commit: commits.CommitInfo,
    type_width: int,
    scope_width: int,
    width: int,
    colorize: bool,
    show_sha: bool,
    fold_scope: bool,
    indent: int,
) -> list[str]:
    lines = [
        _paint_subject_row(
            commit, type_width, scope_width, width, colorize, show_sha, fold_scope
        )
    ]
    focus = commit.breaking_footer or commit.lead
    if focus:
        for row in _wrap_body(focus, indent, width, max_rows=4):
            lines.append(row)
    for verified in commit.verified:
        lines.append(_fold_verified_row(verified, indent, width, colorize))
    return lines


def _header_lines(
    repo: RenderRepo,
    plugin_ids: Optional[list[str]],
    colorize: bool,
    width: int,
) -> list[str]:
    label = repo.label
    if plugin_ids:
        label = f"{label} · will update {', '.join(plugin_ids)}" if label else (
            f"will update {', '.join(plugin_ids)}"
        )
    visible = f"{repo.name} · {label}" if label else repo.name
    if colorize:
        code = (repo.color_code or "1").strip() or "1"
        bold = code if code.startswith("1;") else f"1;{code}"
        head = term.paint(bold, repo.name, colorize=True)
        tail = term.paint("2", f" · {label}" if label else "", colorize=True)
        single = [head + tail]
    else:
        single = [visible]
    if term.cell_width(visible) <= width:
        lines = single
    else:
        # Narrow panes get the range label on its own wrapped rows instead
        # of a truncated header that would hide the commit count.
        if colorize:
            code = (repo.color_code or "1").strip() or "1"
            bold = code if code.startswith("1;") else f"1;{code}"
            lines = [term.paint(bold, repo.name, colorize=True)]
        else:
            lines = [repo.name]
        for row in term.wrap_cells(label, max(width - 2, 10)) or [""]:
            text = f"  {row}"
            lines.append(term.paint("2", text, colorize=True) if colorize else text)
    for note in repo.notes:
        for row in term.wrap_cells(note, max(width - 2, 10)) or [""]:
            text = f"  {row}"
            lines.append(term.paint("2", text, colorize=True) if colorize else text)
    return lines


def _render_block(
    repo: RenderRepo,
    type_width: int,
    width: int,
    colorize: bool,
    show_sha: bool,
    fold_scope: bool,
    level: str,
    plugin_ids: Optional[list[str]],
) -> list[str]:
    scope_width = 0 if fold_scope else _widest_scope(repo.commit_list)
    indent = _prefix_width(type_width, scope_width)
    lines = _header_lines(repo, plugin_ids, colorize, width)
    ordered = commits.sort_commits(repo.commit_list)
    if repo.kind == "cloned":
        short = (repo.head or "")[:8]
        row = f"  initial checkout @ {short}"
        lines.append(term.paint("2", row, colorize=True) if colorize else row)
        return lines
    if level == "subjects":
        for commit in ordered:
            lines.append(
                _paint_subject_row(
                    commit,
                    type_width,
                    scope_width,
                    width,
                    colorize,
                    show_sha,
                    fold_scope,
                )
            )
        return lines
    for index, commit in enumerate(ordered):
        if level == "full":
            block = _render_commit_full(
                commit, type_width, scope_width, width, colorize,
                show_sha, fold_scope, indent,
            )
        else:
            block = _render_commit_lead(
                commit, type_width, scope_width, width, colorize,
                show_sha, fold_scope, indent,
            )
        lines.extend(block)
        has_body = len(block) > 1
        if has_body and index < len(ordered) - 1:
            lines.append("")
    return lines


def _hidden_summary(hidden: list[commits.CommitInfo]) -> str:
    by_type: dict[str, list[commits.CommitInfo]] = {}
    for commit in hidden:
        key = commit.type if commit.type else "other"
        by_type.setdefault(key, []).append(commit)
    parts = [f"+ {len(hidden)} more"]
    for key in _HIDDEN_TYPE_ORDER:
        group = by_type.get(key, [])
        if not group:
            continue
        scopes: dict[str, int] = {}
        for commit in group:
            if commit.scope:
                scopes[commit.scope] = scopes.get(commit.scope, 0) + 1
        if scopes and key != "other":
            ranked = sorted(scopes.items(), key=lambda kv: (-kv[1], kv[0]))
            shown = ranked[:2]
            rest = sum(count for _, count in ranked[2:])
            inner = " · ".join(f"{name} {count}" for name, count in shown)
            if rest:
                inner += f" · {rest} other" if inner else f"{rest} other"
            parts.append(f"{key} {len(group)} ({inner})")
        else:
            parts.append(f"{key} {len(group)}")
    return " · ".join(parts)


def _render_grouped(
    repos: list[RenderRepo],
    type_width: int,
    width: int,
    colorize: bool,
    show_sha: bool,
    fold_scope: bool,
    rows_budget: int,
    plugin_ids_by_repo: dict[str, Optional[list[str]]],
) -> list[str]:
    lines = _banner_lines(width, colorize)
    # Mandatory output: headers plus every breaking commit in full.
    pending: list[tuple] = []
    out = list(lines)
    for repo in repos:
        scope_width = 0 if fold_scope else _widest_scope(repo.commit_list)
        indent = _prefix_width(type_width, scope_width)
        head_lines = _header_lines(
            repo, plugin_ids_by_repo.get(repo.name), colorize, width
        )
        if repo.kind == "cloned":
            short = (repo.head or "")[:8]
            row = f"  initial checkout @ {short}"
            head_lines.append(
                term.paint("2", row, colorize=True) if colorize else row
            )
            out.extend(head_lines)
            continue
        ordered = commits.sort_commits(repo.commit_list)
        breaking = [c for c in ordered if c.breaking]
        rest = [c for c in ordered if not c.breaking]
        breaking_lines: list[str] = []
        foldable = set(commits._FOLD_TYPES)
        for commit in breaking:
            breaking_lines.append(
                _paint_subject_row(
                    commit, type_width, scope_width, width, colorize,
                    show_sha, fold_scope,
                )
            )
            focus = commit.breaking_footer or commit.lead
            if focus:
                breaking_lines.extend(_wrap_body(focus, indent, width))
            else:
                note = "breaking · the commit has no migration note"
                breaking_lines.append(" " * indent + note)
            for verified in commit.verified:
                breaking_lines.append(
                    _fold_verified_row(verified, indent, width, colorize)
                )
        # Folded types never get subject rows at the grouped level.
        candidates = [c for c in rest if c.type not in foldable]
        folded_only = [c for c in rest if c.type in foldable]
        out.extend(head_lines)
        out.extend(breaking_lines)
        pending.append(
            (candidates, folded_only, repo, scope_width, indent)
        )
    # Greedy subject fill in run order while rows remain.
    for candidates, folded_only, repo, scope_width, indent in pending:
        shown: list[commits.CommitInfo] = []
        hidden: list[commits.CommitInfo] = []
        for commit in candidates:
            probe = _paint_subject_row(
                commit, type_width, scope_width, width, colorize,
                show_sha, fold_scope,
            )
            # Reserve two rows per repo that still hides commits (the +N
            # more line and the copyable git log line).
            hidden_after = len(candidates) - (len(shown) + 1) + len(folded_only)
            reserve = 2 if hidden_after > 0 else 0
            if len(out) + 1 + reserve <= rows_budget:
                out.append(probe)
                shown.append(commit)
            else:
                hidden.append(commit)
        hidden.extend(folded_only)
        if hidden:
            summary = _hidden_summary(hidden)
            for row in term.wrap_cells(summary, max(width - 2, 10)) or [""]:
                text = f"  {row}"
                out.append(term.paint("2", text, colorize=True) if colorize else text)
            git_line = term.truncate_cells(
                f"    git -C {repo.directory} log --oneline "
                f"{(repo.base or '')[:8]}..{(repo.head or '')[:8]}",
                width,
            )
            out.append(term.paint("2", git_line, colorize=True) if colorize else git_line)
    return out


def render_section(
    repos: list[RenderRepo],
    cols: int,
    lines: int,
    steps: int,
    colorize: bool,
    tty: bool,
    plugin_ids_by_repo: Optional[dict[str, Optional[list[str]]]] = None,
) -> tuple[str, dict]:
    """Render the digest section; returns (text, record)."""
    plugin_ids_by_repo = plugin_ids_by_repo or {}
    width = measure(cols)
    show_sha = cols >= 60
    fold_scope = cols < 45
    live = [r for r in repos if r.commit_list or r.kind == "cloned"]
    if not live:
        record = {"v": 1, "rows": 0, "level": "subjects", "repos": []}
        return "", record
    if not tty:
        return _render_plain(live, width)
    rows_budget = budget(lines, steps)
    all_commits = [c for r in live for c in r.commit_list]
    type_width = _widest_type(all_commits)
    banner_rows = len(_banner_lines(width, colorize))
    counts = {}
    for level in ("full", "lead", "subjects"):
        total = banner_rows
        for repo in live:
            total += len(
                _render_block(
                    repo, type_width, width, colorize, show_sha,
                    fold_scope, level, plugin_ids_by_repo.get(repo.name),
                )
            )
        counts[level] = total
    chosen = "grouped"
    for level in ("full", "lead", "subjects"):
        if counts[level] <= rows_budget:
            chosen = level
            break
    if chosen == "grouped":
        rendered = _render_grouped(
            live, type_width, width, colorize, show_sha, fold_scope,
            rows_budget, plugin_ids_by_repo,
        )
    else:
        rendered = _banner_lines(width, colorize)
        for repo in live:
            rendered.extend(
                _render_block(
                    repo, type_width, width, colorize, show_sha,
                    fold_scope, chosen, plugin_ids_by_repo.get(repo.name),
                )
            )
    text = "\n".join(rendered) + "\n"
    record = {
        "v": 1,
        "rows": len(rendered),
        "level": chosen,
        "repos": [
            {
                "name": repo.name,
                "count": len(repo.commit_list) if repo.kind != "cloned" else 1,
                "label": repo.label,
                "base": repo.base,
                "head": repo.head,
                "breaking": sum(1 for c in repo.commit_list if c.breaking),
                "subjects": [
                    term.sanitize(c.subject or c.raw_subject)
                    for c in commits.sort_commits(repo.commit_list)[:3]
                ]
                if repo.kind != "cloned"
                else [f"initial checkout @ {(repo.head or '')[:8]}"],
            }
            for repo in live
        ],
    }
    return text, record


def _render_plain(
    repos: list[RenderRepo], width: int
) -> tuple[str, dict]:
    out = _banner_lines(width, False)
    shown_total = 0
    record_repos = []
    for repo in repos:
        header = f"{repo.name} · {repo.label}" if repo.label else repo.name
        out.append(header)
        for note in repo.notes:
            out.append(f"  {note}")
        if repo.kind == "cloned":
            out.append(f"  initial checkout @ {(repo.head or '')[:8]}")
            record_repos.append(
                {
                    "name": repo.name, "count": 1, "label": repo.label,
                    "base": repo.base, "head": repo.head, "breaking": 0,
                    "subjects": [f"initial checkout @ {(repo.head or '')[:8]}"],
                }
            )
            continue
        ordered = commits.sort_commits(repo.commit_list)
        subjects = []
        for commit in ordered:
            if shown_total >= _NON_TTY_CAP:
                continue
            line = term.sanitize(commit.raw_subject)
            out.append(term.truncate_cells(f"  {line}", width))
            shown_total += 1
            subjects.append(line)
        record_repos.append(
            {
                "name": repo.name, "count": len(ordered), "label": repo.label,
                "base": repo.base, "head": repo.head,
                "breaking": sum(1 for c in ordered if c.breaking),
                "subjects": subjects[:3],
            }
        )
    # Recompute hidden as total minus shown for one honest +N more line.
    total = sum(
        len(r.commit_list) for r in repos if r.kind != "cloned"
    )
    if total > shown_total:
        out.append(f"  + {total - shown_total} more")
    text = "\n".join(out) + "\n"
    record = {"v": 1, "rows": len(out), "level": "subjects", "repos": record_repos}
    return text, record


def plugin_updates(
    bob_bin: str, repo_dir: str, timeout: float = 10.0
) -> Optional[list[str]]:
    """List plugin IDs with pending file copies; None when unknown."""
    if not bob_bin or not repo_dir:
        return None
    try:
        proc = subprocess.run(
            [
                bob_bin, "plugins", "sync", "--dry-run", "--format", "json",
                "--no-pull", "--repo", repo_dir,
            ],
            check=False,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    try:
        data = json.loads(proc.stdout or "")
    except (json.JSONDecodeError, ValueError):
        return None
    plugins = data.get("plugins") if isinstance(data, dict) else None
    if not isinstance(plugins, list):
        return None
    pending: list[str] = []
    for plugin in plugins:
        if not isinstance(plugin, dict):
            continue
        plugin_id = plugin.get("id")
        files = plugin.get("files")
        if not isinstance(plugin_id, str) or not isinstance(files, list):
            continue
        for entry in files:
            if not isinstance(entry, dict):
                continue
            action = entry.get("action")
            if isinstance(action, str) and action.lower() in {
                "created", "updated", "forced",
            }:
                pending.append(plugin_id)
                break
    return pending


def write_record(path: str, record: dict) -> bool:
    """Best-effort record write; True on success."""
    try:
        directory = os.path.dirname(path)
        if directory:
            os.makedirs(directory, mode=0o700, exist_ok=True)
        payload = json.dumps(record, indent=2, sort_keys=True) + "\n"
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(payload)
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass
        return True
    except OSError:
        return False

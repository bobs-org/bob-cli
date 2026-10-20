"""Execute the published Get started Bash fences in an isolated Bob sandbox."""

from __future__ import annotations

import json
import os
import re
import shlex
import signal
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping

import yaml

from .errors import DocsError
from .markdown import fence_language
from .sandbox import DEFAULT_NOW, BobSandbox, bob_sandbox, resolve_bob_binary


_OPEN_FENCE = re.compile(r"^( {0,3})(`{3,}|~{3,})([^\n]*)$")
_SKIP_MARKER = re.compile(r"<!--[ \t]*tutorial-test:[ \t]*skip[ \t]+(.+?)[ \t]*-->")
_CHECKPOINT_MARKER = re.compile(
    r"<!--[ \t]*tutorial-test:[ \t]*checkpoint[ \t]+([A-Za-z0-9][A-Za-z0-9_-]*)[ \t]*-->"
)
_TASK_LINE = re.compile(r"^(?P<indent>[ \t]*)[-*+]\s+\[(?P<status>[ xX/*?\-])\](?P<body>.*)$")
_DATE_PREFIX = re.compile(r"^(\d{4}-\d{2}-\d{2})")


@dataclass(frozen=True)
class MarkdownFence:
    """A Markdown-parsed fence with its exact source line positions."""

    info: str
    body: str
    start_line: int
    end_line: int


@dataclass(frozen=True)
class TutorialFence:
    """A Bash fence plus its adjacent test marker, if present."""

    page: str
    body: str
    line: int
    skip: str | None = None
    checkpoint: str | None = None


@dataclass(frozen=True)
class Marker:
    kind: str
    value: str
    line: int


_REQUIRED_CHECKPOINTS = frozenset(
    {
        "fixture-copy",
        "inbox-captured",
        "area-captured",
        "initial-plan",
        "daily-note",
        "pomodoro-started",
        "task-linked",
        "task-completed",
        "review-listed",
    }
)


def _markdown_fences(text: str, *, source: str) -> tuple[list[MarkdownFence], list[tuple[int, str]]]:
    """Parse actual fenced code blocks and standalone outside-code comments.

    Markdown fences inside HTML comments are not code. Likewise comments inside
    a code fence are just code and cannot register tutorial markers.
    """

    lines = text.splitlines(keepends=True)
    fences: list[MarkdownFence] = []
    comments: list[tuple[int, str]] = []
    index = 0
    while index < len(lines):
        line = lines[index].rstrip("\r\n")
        opening = _OPEN_FENCE.match(line)
        if opening and not (
            opening.group(2).startswith("`") and "`" in opening.group(3)
        ):
            indent, marker, info = opening.groups()
            char = marker[0]
            count = len(marker)
            close_re = re.compile(rf"^ {{0,3}}{re.escape(char)}{{{count},}}[ \t]*$")
            body_start = index + 1
            cursor = body_start
            while cursor < len(lines) and not close_re.match(lines[cursor].rstrip("\r\n")):
                cursor += 1
            if cursor >= len(lines):
                raise DocsError(f"{source}:{index + 1}: unclosed Markdown fence")
            body = "".join(lines[body_start:cursor])
            fences.append(MarkdownFence(info=info.strip(), body=body, start_line=index + 1, end_line=cursor + 1))
            index = cursor + 1
            continue

        # Only comments whose opening delimiter is outside a fence are parsed.
        # A multiline comment is one comment, so marker-like example text inside
        # it cannot be mistaken for a standalone marker.
        comment_start = line.find("<!--")
        if comment_start >= 0:
            first_line = index + 1
            collected = [line[comment_start:]]
            if "-->" not in collected[0]:
                index += 1
                while index < len(lines):
                    piece = lines[index].rstrip("\r\n")
                    collected.append(piece)
                    if "-->" in piece:
                        break
                    index += 1
            comments.append((first_line, "\n".join(collected)))
        index += 1
    return fences, comments


def parse_tutorial_fences(text: str, *, source: str) -> list[TutorialFence]:
    """Extract ordered Bash fences and validate all tutorial marker semantics."""

    fences, comments = _markdown_fences(text, source=source)
    markers: list[Marker] = []
    for line, raw in comments:
        # A nested illustrative comment inside another HTML comment remains
        # inert. Only a single standalone comment is a marker candidate.
        if "\n" in raw:
            continue
        marker_text = raw.strip()
        skip = _SKIP_MARKER.fullmatch(marker_text)
        checkpoint = _CHECKPOINT_MARKER.fullmatch(marker_text)
        if skip:
            reason = skip.group(1).strip()
            if not reason:
                raise DocsError(f"{source}:{line}: tutorial skip reason must be nonempty")
            markers.append(Marker("skip", reason, line))
        elif checkpoint:
            name = checkpoint.group(1)
            if name not in _REQUIRED_CHECKPOINTS:
                raise DocsError(f"{source}:{line}: unknown tutorial checkpoint {name!r}")
            markers.append(Marker("checkpoint", name, line))
        elif "tutorial-test" in marker_text:
            raise DocsError(f"{source}:{line}: malformed tutorial-test marker")

    used: set[int] = set()
    parsed: list[TutorialFence] = []
    for fence in fences:
        if fence_language(fence.info) != "bash":
            continue
        before = [m for m in markers if m.line + 1 == fence.start_line]
        after = [m for m in markers if m.line == fence.end_line + 1]
        if len(before) > 1 or len(after) > 1:
            raise DocsError(f"{source}:{fence.start_line}: conflicting tutorial markers")
        skip = before[0] if before else None
        checkpoint = after[0] if after else None
        if skip and skip.kind != "skip":
            raise DocsError(f"{source}:{skip.line}: checkpoint marker must follow a Bash fence")
        if checkpoint and checkpoint.kind != "checkpoint":
            raise DocsError(f"{source}:{checkpoint.line}: skip marker must precede a Bash fence")
        if skip and checkpoint:
            raise DocsError(f"{source}:{fence.start_line}: a skipped Bash fence cannot have a checkpoint")
        if skip:
            used.add(skip.line)
        if checkpoint:
            used.add(checkpoint.line)
        parsed.append(
            TutorialFence(
                page=source,
                body=fence.body,
                line=fence.start_line,
                skip=skip.value if skip else None,
                checkpoint=checkpoint.value if checkpoint else None,
            )
        )

    unused = [marker for marker in markers if marker.line not in used]
    if unused:
        marker = unused[0]
        raise DocsError(f"{source}:{marker.line}: unconsumed tutorial-test {marker.kind} marker")
    return parsed


def _nav_pages(root: Path) -> list[Path]:
    config = root / "mkdocs.yml"
    try:
        # Only navigation structure and scalar page paths matter here. BaseLoader
        # keeps those values as strings while tolerating MkDocs' Python-name tags
        # elsewhere in the config (SafeLoader rejects those tags outright).
        data = yaml.load(config.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
    except (OSError, yaml.YAMLError) as error:
        raise DocsError(f"could not read Get started navigation from {config}: {error}") from error
    nav = data.get("nav") if isinstance(data, dict) else None
    if not isinstance(nav, list):
        raise DocsError("mkdocs.yml: nav must be a list before tutorial fences can be run")
    section: object | None = None
    for item in nav:
        if isinstance(item, dict) and "Get started" in item:
            section = item["Get started"]
            break
    if not isinstance(section, list):
        raise DocsError("mkdocs.yml: add a populated 'Get started' nav before running the tutorial")

    page_names: list[str] = []

    def walk(entries: Iterable[object]) -> None:
        for entry in entries:
            if isinstance(entry, str):
                if entry.startswith("start/") and entry.endswith(".md"):
                    page_names.append(entry)
            elif isinstance(entry, dict):
                for value in entry.values():
                    if isinstance(value, str) and value.startswith("start/") and value.endswith(".md"):
                        page_names.append(value)
                    elif isinstance(value, list):
                        walk(value)

    walk(section)
    start_dir = root / "docs" / "start"
    files = sorted(path.relative_to(root / "docs").as_posix() for path in start_dir.rglob("*.md")) if start_dir.exists() else []
    duplicate = sorted({name for name in page_names if page_names.count(name) > 1})
    missing = sorted(set(files) - set(page_names))
    nonexistent = sorted(set(page_names) - set(files))
    if duplicate or missing or nonexistent:
        parts = []
        if duplicate:
            parts.append("duplicate Get started nav pages: " + ", ".join(duplicate))
        if missing:
            parts.append("start pages missing from Get started nav: " + ", ".join(missing))
        if nonexistent:
            parts.append("Get started nav points to missing pages: " + ", ".join(nonexistent))
        raise DocsError("; ".join(parts))
    if not page_names:
        raise DocsError("Get started nav has no pages to test")
    return [root / "docs" / name for name in page_names]


def _load_plan(sandbox: BobSandbox) -> object:
    return _json_command(sandbox, ["plan", "-f", "json"])


def _load_freshness(sandbox: BobSandbox) -> object:
    return _json_command(sandbox, ["freshness", "list", "-f", "json"])


def _json_command(sandbox: BobSandbox, args: list[str]) -> object:
    try:
        completed = subprocess.run(
            [str(sandbox.bob_bin), *args],
            cwd=sandbox.workdir,
            env=sandbox.env,
            check=False,
            capture_output=True,
            text=True,
            timeout=40,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise DocsError(f"checkpoint could not run `bob {' '.join(args)}`: {error}") from error
    if completed.returncode:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise DocsError(
            f"checkpoint command `bob {' '.join(args)}` failed with exit {completed.returncode}: {detail}"
        )
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise DocsError(
            f"checkpoint command `bob {' '.join(args)}` did not return JSON: {error}"
        ) from error


def _date_for(sandbox: BobSandbox) -> str:
    match = _DATE_PREFIX.match(sandbox.now)
    if not match:
        raise DocsError(f"BOB_NOW is not an ISO date/time: {sandbox.now!r}")
    return match.group(1)


def _vault_path(sandbox: BobSandbox) -> Path:
    raw = sandbox.env.get("BOB_DIR", "")
    if not raw:
        raise DocsError("tutorial did not select BOB_DIR")
    return Path(raw).resolve()


def _tasks(vault: Path, block_id: str) -> list[tuple[Path, str, str]]:
    found: list[tuple[Path, str, str]] = []
    for note in vault.rglob("*.md"):
        if ".obsidian" in note.parts:
            continue
        try:
            lines = note.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeError):
            continue
        for line in lines:
            match = _TASK_LINE.match(line)
            if match and re.search(rf"\^{re.escape(block_id)}(?:\s|$)", match.group("body")):
                found.append((note, match.group("status"), line))
    return found


def _require_task(vault: Path, block_id: str, note_name: str, *, status: str | None = None) -> tuple[Path, str, str]:
    matches = _tasks(vault, block_id)
    if len(matches) != 1:
        raise DocsError(f"expected exactly one task with ^{block_id}; found {len(matches)}")
    note, actual_status, line = matches[0]
    if note.name != note_name:
        raise DocsError(f"task ^{block_id} landed in {note.relative_to(vault)}, expected {note_name}")
    if status is not None and actual_status != status:
        raise DocsError(f"task ^{block_id} has status [{actual_status}], expected [{status}]")
    return matches[0]


def _walk_dicts(value: object) -> Iterable[dict[str, object]]:
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from _walk_dicts(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk_dicts(child)


def _records_for_id(value: object, block_id: str) -> list[dict[str, object]]:
    keys = ("id", "task_id", "block_id", "blockId", "taskId")
    out = []
    for item in _walk_dicts(value):
        if any(item.get(key) in (block_id, f"^{block_id}") for key in keys):
            out.append(item)
    return out


def _checkpoint_fixture(sandbox: BobSandbox) -> None:
    vault = _vault_path(sandbox)
    if not vault.is_relative_to(sandbox.root) or vault == sandbox.root / "not-selected-vault":
        raise DocsError("tutorial must select a fresh vault copy inside its sandbox")
    if not (vault / "mac_inbox.md").is_file() or not (vault / ".obsidian/plugins/obsidian-tasks-plugin/data.json").is_file():
        raise DocsError("copied demo vault is missing its inbox or Tasks settings")
    settings = json.loads((vault / ".obsidian/plugins/obsidian-tasks-plugin/data.json").read_text())
    if settings.get("taskFormat") != "dataview" or settings.get("globalFilter") != "#task":
        raise DocsError("demo Tasks settings must use Dataview format and the #task filter")


def _checkpoint_initial_plan(sandbox: BobSandbox) -> None:
    plan = _load_plan(sandbox)
    if not isinstance(plan, dict) or plan.get("schema_version") != 2:
        raise DocsError("bob plan JSON did not match schema_version 2")
    if plan.get("date") != _date_for(sandbox) or plan.get("status") != "ok":
        raise DocsError("bob plan JSON does not report today's successful plan")
    if (plan.get("today") or {}).get("count") != 0:
        raise DocsError("Today should be empty before the tutorial starts a Pomodoro")
    if plan.get("today_tasks") != []:
        raise DocsError("today_tasks should be empty before the tutorial starts a Pomodoro")


def _checkpoint_daily_note(sandbox: BobSandbox) -> None:
    vault = _vault_path(sandbox)
    day = _date_for(sandbox)
    daily = vault / day[:4] / f"{day.replace('-', '')}.md"
    if sandbox.env.get("BOB_DAY_FILE") != str(daily):
        raise DocsError(f"tutorial must select today's daily file explicitly: {daily}")
    try:
        contents = daily.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"tutorial did not create {daily.relative_to(vault)}: {error}") from error
    if contents.count("## Pomodoros") != 1:
        raise DocsError("daily-note setup must create one explicit empty ## Pomodoros section")
    section = contents.split("## Pomodoros", 1)[1].split("\n## ", 1)[0]
    if any(line.lstrip().startswith("-") for line in section.splitlines()):
        raise DocsError("daily-note setup must leave the Pomodoros section empty")


def _checkpoint_pomodoro_started(sandbox: BobSandbox) -> None:
    vault = _vault_path(sandbox)
    _require_task(vault, "seedlings", "garden.md", status=" ")
    day = _date_for(sandbox)
    daily = vault / day[:4] / f"{day.replace('-', '')}.md"
    text = daily.read_text(encoding="utf-8")
    if not re.search(r"(?m)^- \[ \] \(.*\) — FOCUS\s*$", text):
        raise DocsError("bob capture '=#focus' did not leave an open named FOCUS Pomodoro")
    plan = _load_plan(sandbox)
    if (plan.get("today") or {}).get("count") != 0 or plan.get("today_tasks") != []:
        raise DocsError("starting a Pomodoro alone must not add a Today Task Link")


def _checkpoint_linked(sandbox: BobSandbox) -> None:
    vault = _vault_path(sandbox)
    _require_task(vault, "seedlings", "garden.md", status="*")
    day = _date_for(sandbox)
    daily = vault / day[:4] / f"{day.replace('-', '')}.md"
    lines = daily.read_text(encoding="utf-8").splitlines()
    parent = next((i for i, line in enumerate(lines) if re.match(r"^- \[ \] \(.*\) — FOCUS\s*$", line)), None)
    if parent is None:
        raise DocsError("linked task has no open FOCUS Pomodoro")
    target = "[[garden#^seedlings]]"
    if parent + 1 >= len(lines) or not re.fullmatch(
        r"(?:\t| {2,})- " + re.escape(target), lines[parent + 1]
    ):
        raise DocsError("the Task Link must be a direct child of the open FOCUS Pomodoro")
    plan = _load_plan(sandbox)
    if not isinstance(plan, dict) or plan.get("schema_version") != 2:
        raise DocsError("bob plan JSON did not match schema_version 2")
    today_tasks = plan.get("today_tasks")
    if not isinstance(today_tasks, list) or plan.get("today", {}).get("count") != 1:
        raise DocsError("bob plan JSON should report one Today Task Link")
    task_record = next(
        (item for item in today_tasks if isinstance(item, dict) and item.get("block_id") == "seedlings"),
        None,
    )
    if task_record is None:
        raise DocsError("bob plan -f json does not expose the linked ^seedlings task")
    if task_record.get("path") != "garden.md" or task_record.get("status_symbol") != "*":
        raise DocsError("bob plan JSON should report garden.md's Next task in Today")


def _checkpoint_completed(sandbox: BobSandbox) -> None:
    vault = _vault_path(sandbox)
    day = _date_for(sandbox)
    _require_task(vault, "seedlings", "garden.md", status="x")
    completed_line = _require_task(vault, "seedlings", "garden.md")[2]
    if f"[completion::{day}]" not in completed_line and f"[completion:: {day}]" not in completed_line:
        raise DocsError("completed task line is missing today's completion date")
    _require_task(vault, "seed-packet", "mac_inbox.md", status=" ")
    daily = vault / day[:4] / f"{day.replace('-', '')}.md"
    daily_text = daily.read_text(encoding="utf-8")
    close = next((line for line in daily_text.splitlines() if "FOCUS" in line and line.startswith("- [x]")), None)
    if close is None:
        raise DocsError("closing with x! did not close the FOCUS ledger entry")
    if "[t:: 0m]" not in close:
        raise DocsError("the fixed-clock immediate close should record an actual zero-minute session")
    plan = _load_plan(sandbox)
    if (plan.get("today") or {}).get("count") != 0 or plan.get("today_tasks") != []:
        raise DocsError("completed ^seedlings still appears in today's plan JSON")


def _checkpoint_review_listed(sandbox: BobSandbox) -> None:
    freshness = _load_freshness(sandbox)
    records = _records_for_id(freshness, "seed-packet")
    if not records:
        raise DocsError("bob freshness list -f json omitted the unfinished inbox task ^seed-packet")
    if not isinstance(freshness, dict) or freshness.get("schema_version") != 14:
        raise DocsError("bob freshness list JSON did not match schema_version 14")
    inbox_record = records[0]
    if inbox_record.get("path") != "mac_inbox.md" or inbox_record.get("tier") != "new":
        raise DocsError("freshness JSON should place the unfinished inbox task in NEW")
    if _records_for_id(freshness, "seedlings"):
        raise DocsError("bob freshness list still includes the completed task ^seedlings")


_CHECKPOINTS = {
    "fixture-copy": _checkpoint_fixture,
    "inbox-captured": lambda sandbox: _require_task(
        _vault_path(sandbox), "seed-packet", "mac_inbox.md", status=" "
    ),
    "area-captured": lambda sandbox: _require_task(
        _vault_path(sandbox), "seedlings", "garden.md", status=" "
    ),
    "initial-plan": _checkpoint_initial_plan,
    "daily-note": _checkpoint_daily_note,
    "pomodoro-started": _checkpoint_pomodoro_started,
    "task-linked": _checkpoint_linked,
    "task-completed": _checkpoint_completed,
    "review-listed": _checkpoint_review_listed,
}


def assert_checkpoint(checkpoint: str, root: str, bob_bin: str, now: str) -> None:
    """Re-open a sandbox checkpoint from the persistent shell's environment."""

    path = Path(root).resolve()
    env = dict(os.environ)
    env["BOB_DIR"] = str(_vault_path_from_env(env))
    sandbox = BobSandbox(
        root=path,
        workdir=Path.cwd(),
        home=Path(env["HOME"]),
        vault=Path(env["BOB_DIR"]),
        demo_source=path / "work" / "examples" / "demo-vault",
        env=env,
        bob_bin=Path(bob_bin),
        now=now,
    )
    check = _CHECKPOINTS.get(checkpoint)
    if check is None:
        raise DocsError(f"unknown tutorial checkpoint {checkpoint!r}")
    check(sandbox)


def _vault_path_from_env(env: Mapping[str, str]) -> Path:
    raw = env.get("BOB_DIR", "")
    if not raw:
        raise DocsError("tutorial did not export BOB_DIR")
    return Path(raw).resolve()


def _runner_script(fences: list[TutorialFence], *, sandbox: BobSandbox, source_root: Path) -> tuple[str, list[str]]:
    script = ["set -Eeuo pipefail", "trap 'rc=$?; printf \\\"\\nTUTORIAL_FAILURE status=%s line=%s\\n\\\" \\\"$rc\\\" \\\"$LINENO\\\" >&2' ERR"]
    skips: list[str] = []
    checkpoint_code = (
        "from bob_docs.tutorial import assert_checkpoint; import sys; "
        "assert_checkpoint(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])"
    )
    for number, fence in enumerate(fences, start=1):
        if fence.skip is not None:
            skips.append(f"skipped: {fence.page}:{fence.line}: {fence.skip}")
            continue
        label = f"{fence.page}:{fence.line}"
        script.append(f"printf '\\nTUTORIAL_FENCE {number} {shlex.quote(label)}\\n'")
        script.append(fence.body.rstrip("\n"))
        if fence.checkpoint:
            command = [
                sys.executable,
                "-c",
                checkpoint_code,
                fence.checkpoint,
                "${BOB_DOCS_SANDBOX_ROOT}",
                str(sandbox.bob_bin),
                sandbox.now,
            ]
            quoted = " ".join(shlex.quote(arg) if not arg.startswith("${") else f'"{arg}"' for arg in command)
            script.append(f"{quoted}")
    return "\n".join(script) + "\n", skips


def _run_shell(script: str, *, sandbox: BobSandbox, timeout: float) -> str:
    env = dict(sandbox.env)
    env["BOB_DOCS_SANDBOX_ROOT"] = str(sandbox.root)
    try:
        process = subprocess.Popen(
            ["bash", "--noprofile", "--norc", "-c", script],
            cwd=sandbox.workdir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
        )
    except OSError as error:
        raise DocsError(f"could not start the tutorial Bash runner: {error}") from error
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            output, _ = process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            output, _ = process.communicate()
        tail = "\n".join(output.splitlines()[-60:])
        raise DocsError(f"tutorial timed out after {timeout:g}s; last output:\n{tail}") from error
    if process.returncode:
        tail = "\n".join(output.splitlines()[-80:])
        active = next((line for line in reversed(output.splitlines()) if line.startswith("TUTORIAL_FENCE ")), "unknown fence")
        raise DocsError(f"tutorial failed at {active} (exit {process.returncode}); output:\n{tail}")
    return output


def run_tutorial(
    root: Path,
    *,
    bob_bin: Path | None = None,
    now: str = DEFAULT_NOW,
    source_env: Mapping[str, str] | None = None,
    timeout: float = 120,
) -> int:
    """Run all executable start-page Bash fences and their state checkpoints."""

    repo_root = Path(root).resolve()
    pages = _nav_pages(repo_root)
    all_fences: list[TutorialFence] = []
    seen_checkpoints: set[str] = set()
    for page in pages:
        rel = page.relative_to(repo_root).as_posix()
        fences = parse_tutorial_fences(page.read_text(encoding="utf-8"), source=rel)
        for fence in fences:
            if fence.checkpoint:
                if fence.checkpoint in seen_checkpoints:
                    raise DocsError(f"duplicate tutorial checkpoint {fence.checkpoint!r}")
                seen_checkpoints.add(fence.checkpoint)
        all_fences.extend(fences)
    missing = sorted(_REQUIRED_CHECKPOINTS - seen_checkpoints)
    if missing:
        raise DocsError("missing required tutorial checkpoints: " + ", ".join(missing))
    if bob_bin is None:
        bob_bin = resolve_bob_binary(repo_root, source_env=source_env)
    else:
        bob_bin = Path(bob_bin).resolve()
        if not bob_bin.is_file() or not os.access(bob_bin, os.X_OK):
            raise DocsError(f"prepared Bob binary is not executable: {bob_bin}")

    with bob_sandbox(repo_root, bob_bin, now=now, source_env=source_env) as sandbox:
        script, skips = _runner_script(all_fences, sandbox=sandbox, source_root=repo_root)
        output = _run_shell(script, sandbox=sandbox, timeout=timeout)
        if output.strip():
            print(output, end="" if output.endswith("\n") else "\n")
        for item in skips:
            print(item)
    print("tutorial: passed")
    return 0

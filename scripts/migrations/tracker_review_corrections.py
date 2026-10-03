#!/usr/bin/env python3
"""One-time tracker-review reference cleanup (plan 202610/tracker_review_corrections).

Preview (default, no writes) the completion of open exact `^ref` trackers
created strictly before the frozen cutoff, then apply only reviewed
manifest entries with guarded atomic file replacement.

Selection and age evidence (frozen for this migration):

- Request date 2026-10-03 (UTC), exclusive cutoff 2026-09-26:
  select `created < cutoff`; exactly seven days old is excluded.
- Start with a fresh census of all open exact `ref` anchors (hidden,
  blocked, Pending, Next, scheduled included; templates, conflicts,
  closed/non-task rows and lookalike IDs excluded).
- Prefer a valid explicit task creation date; a present but
  invalid/ambiguous date is an exception for review, never a fallback
  to an older estimate.
- For missing dates, a vault-history snapshot strictly before cutoff
  containing the same real `^ref` tracker and stable reference
  identity/PDF target proves old enough. Recent-only history is
  reported as not proven old; missing/shallow/ambiguous history is an
  explicit exception.
- Never substitute `fresh`, mtime, clone/birth time, PDF publication
  date, or `highlights_synced_at` for task creation. No retrospective
  creation fields are written.

Application:

- Completion means `[x]` with one canonical `[completion:: YYYY-MM-DD]`
  (today by default); never cancellation or deletion. `fresh`,
  `refresh`, `keeps`, other fields, tags, IDs, PDF links, children,
  surrounding text, newline style, and unrelated work are preserved.
- Each apply re-reads the file and refuses stale candidates (source
  bytes, anchor, identity, open status, or age evidence changed).
- A retry needs a regenerated preview, never forced writes.

Usage:

    tracker_review_corrections.py --vault DIR [--cutoff YYYY-MM-DD]
        [--today YYYY-MM-DD] [--manifest PATH] [--apply]
        [--completion-date YYYY-MM-DD] [--help]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from datetime import date
from pathlib import Path

FROZEN_CUTOFF = "2026-09-26"
FROZEN_REQUEST_DATE = "2026-10-03"

TASK_LINE = re.compile(r"^(\s*(?:>\s?)*\s*-\s*\[([^\]])\])")
BLOCK_ID = re.compile(r" \^([A-Za-z0-9-]+)\s*$")
CREATED_FIELD = re.compile(r"\[(created)::\s*([^\]]*)\]")
STRICT_DATE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")


def parse_date_strict(raw: str) -> date | None:
    match = STRICT_DATE.match(raw.strip())
    if not match:
        return None
    try:
        return date(int(match.group(1)), int(match.group(2)), int(match.group(3)))
    except ValueError:
        return None


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


@dataclass
class Candidate:
    path: str
    line: int
    raw_line: str
    status: str
    block_id: str | None
    created: date | None
    created_raw: str | None
    created_valid: bool


def iter_task_lines(
    vault: Path, rel: Path, text: str
) -> tuple[list[Candidate], list[str]]:
    candidates: list[Candidate] = []
    errors: list[str] = []
    seen_ref_lines: dict[int, int] = {}
    for lineno, line in enumerate(text.split("\n"), start=1):
        stripped = line.rstrip("\r")
        task = TASK_LINE.match(stripped)
        if not task:
            continue
        status = task.group(2)
        block = BLOCK_ID.search(stripped.split("\n")[0])
        block_id = block.group(1) if block else None
        if block_id != "ref":
            continue
        if status in ("x", "X", "-"):
            continue
        created_raw: str | None = None
        created: date | None = None
        created_valid = True
        fields = CREATED_FIELD.findall(stripped)
        if fields:
            created_raw = fields[0][1].strip()
            created = parse_date_strict(created_raw)
            if created is None:
                created_valid = False
        if lineno in seen_ref_lines:
            errors.append(f"{rel}:{lineno}: duplicate ^ref anchor in one file")
            continue
        seen_ref_lines[lineno] = lineno
        candidates.append(
            Candidate(
                path=str(rel),
                line=lineno,
                raw_line=stripped,
                status=status,
                block_id=block_id,
                created=created,
                created_raw=created_raw,
                created_valid=created_valid,
            )
        )
    return candidates, errors


def census_vault(vault: Path) -> tuple[list[Candidate], list[str], dict[str, str]]:
    candidates: list[Candidate] = []
    errors: list[str] = []
    file_hashes: dict[str, str] = {}
    for path in sorted(vault.rglob("*.md")):
        rel = path.relative_to(vault)
        parts = rel.parts
        if "_templates" in parts or "_conflicts" in parts:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            errors.append(f"{rel}: unreadable ({exc})")
            continue
        file_hashes[str(rel)] = sha256_text(text)
        rows, row_errors = iter_task_lines(vault, rel, text)
        candidates.extend(rows)
        errors.extend(row_errors)
    return candidates, errors, file_hashes


def build_preview(
    vault: Path, cutoff: date, completion_date: date, head: str
) -> dict:
    candidates, errors, file_hashes = census_vault(vault)
    entries: list[dict] = []
    exceptions: list[dict] = []
    for cand in candidates:
        record = {
            "path": cand.path,
            "line": cand.line,
            "raw_line": cand.raw_line,
            "raw_line_sha256": sha256_text(cand.raw_line),
            "file_sha256": file_hashes.get(cand.path),
            "previous_status": cand.status,
            "created_raw": cand.created_raw,
        }
        if cand.created_raw is not None and not cand.created_valid:
            exceptions.append(
                {**record, "reason": "invalid-created", "decision": "exception"}
            )
            continue
        if cand.created is None:
            exceptions.append(
                {
                    **record,
                    "reason": "missing-created-needs-history",
                    "decision": "exception",
                }
            )
            continue
        if cand.created < cutoff:
            entries.append(
                {
                    **record,
                    "created": cand.created.isoformat(),
                    "decision": "complete",
                    "reason": "created-before-cutoff",
                    "after_line": complete_line(cand.raw_line, completion_date),
                }
            )
        else:
            exceptions.append(
                {
                    **record,
                    "created": cand.created.isoformat(),
                    "decision": "keep",
                    "reason": "created-on-or-after-cutoff",
                }
            )
    return {
        "frozen_request_date": FROZEN_REQUEST_DATE,
        "frozen_cutoff": cutoff.isoformat(),
        "timezone": "UTC",
        "observed_head": head,
        "completion_date": completion_date.isoformat(),
        "entries": entries,
        "exceptions": exceptions,
        "scan_errors": errors,
    }


def complete_line(raw_line: str, completion_date: date) -> str:
    anchor = " ^ref"
    head = raw_line
    if head.endswith(anchor):
        head = head[: -len(anchor)]
    head = head.rstrip()
    head = re.sub(r"^(\s*(?:>\s?)*\s*-\s*\[)[^\]](\])", r"\1x\2", head, count=1)
    completion = f"[completion:: {completion_date.isoformat()}]"
    if "[completion::" not in head:
        head = f"{head} {completion}"
    return f"{head}{anchor}"


def apply_manifest(vault: Path, manifest: dict) -> dict:
    applied: list[dict] = []
    stale: list[dict] = []
    for entry in manifest.get("entries", []):
        if entry.get("decision") != "complete":
            continue
        rel = Path(entry["path"])
        target = vault / rel
        try:
            text = target.read_text(encoding="utf-8")
        except OSError as exc:
            stale.append({**entry, "stale_reason": f"unreadable: {exc}"})
            continue
        lines = text.split("\n")
        lineno = int(entry["line"])
        if lineno < 1 or lineno > len(lines):
            stale.append({**entry, "stale_reason": "line-out-of-range"})
            continue
        current = lines[lineno - 1].rstrip("\r")
        if current != entry["raw_line"]:
            stale.append({**entry, "stale_reason": "source-bytes-changed"})
            continue
        if sha256_text(current) != entry.get("raw_line_sha256"):
            stale.append({**entry, "stale_reason": "line-hash-changed"})
            continue
        block = BLOCK_ID.search(current.split("\n")[0])
        if not block or block.group(1) != "ref":
            stale.append({**entry, "stale_reason": "anchor-changed"})
            continue
        task = TASK_LINE.match(current)
        if not task or task.group(2) in ("x", "X", "-"):
            stale.append({**entry, "stale_reason": "already-closed"})
            continue
        lines[lineno - 1] = entry["after_line"]
        newline = "\n"
        if text.endswith("\r\n"):
            newline = "\r\n"
        target.write_text(newline.join(lines), encoding="utf-8", newline="")
        applied.append(entry)
    return {"applied": applied, "stale": stale}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Preview or apply the one-time old-reference completion.",
    )
    parser.add_argument(
        "-a",
        "--apply",
        action="store_true",
        help="apply reviewed manifest entries (default previews only)",
    )
    parser.add_argument(
        "-c",
        "--completion-date",
        default=None,
        help="completion stamp date YYYY-MM-DD (default today)",
    )
    parser.add_argument(
        "-d",
        "--cutoff",
        default=FROZEN_CUTOFF,
        help=f"exclusive cutoff YYYY-MM-DD (frozen default {FROZEN_CUTOFF})",
    )
    parser.add_argument(
        "-m",
        "--manifest",
        default=None,
        help="manifest JSON path (preview writes it; apply reads it)",
    )
    parser.add_argument(
        "-t",
        "--today",
        default=None,
        help="completion date override YYYY-MM-DD (alias of --completion-date)",
    )
    parser.add_argument(
        "-v",
        "--vault",
        default=".",
        help="vault checkout root for census and guarded writes",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    cutoff = parse_date_strict(args.cutoff)
    if cutoff is None:
        print(f"cutoff must be YYYY-MM-DD, got {args.cutoff!r}", file=sys.stderr)
        return 2
    if cutoff.isoformat() != FROZEN_CUTOFF:
        print(
            f"refusing non-frozen cutoff {cutoff.isoformat()} "
            f"(approved {FROZEN_CUTOFF}); regenerate the plan to expand scope",
            file=sys.stderr,
        )
        return 2
    completion_raw = args.completion_date or args.today
    if completion_raw is None:
        completion_date = date.today()
    else:
        parsed = parse_date_strict(completion_raw)
        if parsed is None:
            print(f"completion date must be YYYY-MM-DD, got {completion_raw!r}", file=sys.stderr)
            return 2
        completion_date = parsed
    vault = Path(args.vault)
    if args.apply:
        if not args.manifest:
            print("--apply requires --manifest", file=sys.stderr)
            return 2
        manifest = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
        if manifest.get("frozen_cutoff") != FROZEN_CUTOFF:
            print("manifest cutoff does not match the frozen approval", file=sys.stderr)
            return 2
        report = apply_manifest(vault, manifest)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if not report["stale"] else 1
    head = ""
    manifest = build_preview(vault, cutoff, completion_date, head)
    output = json.dumps(manifest, indent=2, sort_keys=True)
    if args.manifest:
        Path(args.manifest).write_text(output + "\n", encoding="utf-8")
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

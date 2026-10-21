#!/usr/bin/env python3
"""Decide whether the scheduled release train should ship today.

One serialized workflow makes every release decision in order: guard, CI on
the exact SHA, `release-plz update`, fast-forward push, `release-plz release`,
optional binaries. This guard is step one. It is stdlib-only (Python >= 3.11
for `tomllib` and `zoneinfo`) and every behavior that needs testing takes
injectable inputs, so the unit tests are offline and deterministic.

Mode comes from `release-plz.toml`: `git_only = true` is `rehearsal`
(tags + GitHub Releases only), anything else is `live` (crates.io sparse
index). The train is armed only when the repository variable
`RELEASES_PAUSED` is exactly `false`; unset or any other value means paused
and fails closed.

Known gap (documented in the plan): a rehearsal run that created a tag but
not its GitHub Release is not counted as a release, because the rehearsal
data source is the newest Release `createdAt`, not tags.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_TOML = ROOT / "release-plz.toml"
DEFAULT_CARGO_TOML = ROOT / "Cargo.toml"

PAUSE_DOC_URL = "https://bobs-org.github.io/develop/releases/pause-and-resume-releases/"
RECOVER_DOC_URL = "https://bobs-org.github.io/develop/releases/recover-a-failed-release/"
GUARD_REF_URL = "https://bobs-org.github.io/develop/releases/reference/#daily-guard"

NY_TZ = ZoneInfo("America/New_York")
USER_AGENT = "bob-cli-release-train/1.0 (+https://github.com/bobs-org/bob-cli)"


# ---------------------------------------------------------------------------
# Pure helpers (all unit-tested with injected inputs)
# ---------------------------------------------------------------------------

def read_mode(toml_path: Path | str | None = None) -> str:
    """Return `rehearsal` when `release-plz.toml` sets `git_only = true`.

    Anything else -- `git_only = false`, a missing key, a missing file, or
    an unparseable file -- is `live`, matching release-plz's own default
    (`git_only` defaults to false, i.e. crates.io publishing).
    """
    path = Path(toml_path) if toml_path is not None else DEFAULT_TOML
    try:
        import tomllib
    except ImportError:  # Python < 3.11 should not happen in CI; fail closed.
        return "live"
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, ValueError):
        return "live"
    workspace = data.get("workspace", {})
    if isinstance(workspace, dict) and workspace.get("git_only") is True:
        return "rehearsal"
    return "live"


def read_crate_name(cargo_toml: Path | str | None = None) -> str:
    """Return the `[package] name` from Cargo.toml (default `bob-obsidian`)."""
    path = Path(cargo_toml) if cargo_toml is not None else DEFAULT_CARGO_TOML
    try:
        import tomllib

        data = tomllib.loads(path.read_text())
        name = data.get("package", {}).get("name")
        if isinstance(name, str) and name:
            return name
    except (OSError, ValueError, ImportError):
        pass
    return "bob-obsidian"


def is_armed(paused_value: str | None) -> bool:
    """Return True only when `RELEASES_PAUSED` is exactly `"false"`.

    Unset (None) or any other value means paused and fails closed, so the
    train never runs until someone explicitly arms it with
    `gh variable set RELEASES_PAUSED --body false`.
    """
    return paused_value == "false"


def sparse_index_path(crate_name: str) -> str:
    """Return the Cargo sparse-index path for a crate name.

    General Cargo rules: 1-char names live under `1/`, 2-char under `2/`,
    3-char under `3/<first-char>/`, and longer names under
    `<first-two>/<next-two>/`. For example `bob-obsidian` maps to
    `bo/b-/bob-obsidian`.
    """
    name = crate_name.lower()
    if len(name) == 1:
        return f"1/{name}"
    if len(name) == 2:
        return f"2/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[0:2]}/{name[2:4]}/{name}"


def parse_index_pubtimes(index_text: str) -> _dt.datetime | None:
    """Return the maximum `pubtime` over every line of a sparse index entry.

    Yanked versions are included: yanking never resets the daily guard. Each
    line is a JSON object; lines without a parseable `pubtime` are ignored.
    Returns None when no line carries a timestamp (e.g. a fresh crate).
    """
    latest: _dt.datetime | None = None
    for line in index_text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            record = json.loads(line)
        except ValueError:
            continue
        raw = record.get("pubtime")
        if not isinstance(raw, str) or not raw:
            continue
        try:
            stamp = _dt.datetime.fromisoformat(raw.replace("Z", "+00:00"))
        except ValueError:
            continue
        if stamp.tzinfo is None:
            stamp = stamp.replace(tzinfo=_dt.timezone.utc)
        if latest is None or stamp > latest:
            latest = stamp
    return latest


def parse_gh_releases_json(payload: str) -> _dt.datetime | None:
    """Return the newest `createdAt` from `gh release list --json` output.

    Drafts and prereleases are included (the CLI returns them unless asked
    to exclude them). Returns None when the list is empty.
    """
    try:
        releases = json.loads(payload)
    except ValueError:
        return None
    if not isinstance(releases, list):
        return None
    latest: _dt.datetime | None = None
    for item in releases:
        if not isinstance(item, dict):
            continue
        raw = item.get("createdAt") or item.get("created_at")
        if not isinstance(raw, str) or not raw:
            continue
        try:
            stamp = _dt.datetime.fromisoformat(raw.replace("Z", "+00:00"))
        except ValueError:
            continue
        if stamp.tzinfo is None:
            stamp = stamp.replace(tzinfo=_dt.timezone.utc)
        if latest is None or stamp > latest:
            latest = stamp
    return latest


def new_york_date(moment: _dt.datetime) -> _dt.date:
    """Return the America/New_York calendar date of an aware datetime."""
    if moment.tzinfo is None:
        moment = moment.replace(tzinfo=_dt.timezone.utc)
    return moment.astimezone(NY_TZ).date()


def decide(
    *,
    mode: str,
    paused_value: str | None,
    ref: str,
    event_name: str,
    dry_run_input: bool,
    override_reason: str,
    sha_tagged: bool,
    last_release_at: _dt.datetime | None,
    lookup_error: str | None,
    now: _dt.datetime,
) -> dict:
    """Decide `go` and a human-readable `reason` for the train.

    Returns a dict with keys `go` (bool), `reason` (str), `mode` (str),
    `dry_run` (bool, push events are always dry runs) and `hard_fail`
    (bool, a lookup failure that must fail the job closed).
    """
    dry_run = bool(dry_run_input) or event_name == "push"
    override = (override_reason or "").strip()
    base = {"mode": mode, "dry_run": dry_run, "hard_fail": False}

    if not is_armed(paused_value):
        if paused_value is None or paused_value == "":
            reason = (
                "unarmed: RELEASES_PAUSED is unset, so the train stays paused. "
                f"See {PAUSE_DOC_URL}"
            )
        else:
            reason = (
                f"paused: RELEASES_PAUSED is {paused_value!r} (expected exactly "
                f"'false' to arm). See {PAUSE_DOC_URL}"
            )
        return {**base, "go": False, "reason": reason}

    if not dry_run and ref != "refs/heads/master":
        return {
            **base,
            "go": False,
            "reason": (
                f"refused: non-dry-run release from {ref!r}; only "
                f"refs/heads/master may ship. See {RECOVER_DOC_URL}"
            ),
        }

    if sha_tagged:
        return {
            **base,
            "go": False,
            "reason": "skip: this SHA already carries a v* tag, so it shipped already.",
        }

    if lookup_error:
        return {
            **base,
            "go": False,
            "hard_fail": True,
            "reason": (
                f"lookup failed ({mode} source): {lookup_error}. Failing closed; "
                f"see {RECOVER_DOC_URL} and {GUARD_REF_URL}"
            ),
        }

    if last_release_at is None:
        if dry_run:
            reason = (
                f"dry run: no previous {'GitHub Release' if mode == 'rehearsal' else 'crates.io publish'} "
                f"found; the next scheduled run would ship. See {GUARD_REF_URL}"
            )
        else:
            reason = (
                f"go: no previous {'GitHub Release' if mode == 'rehearsal' else 'crates.io publish'} "
                f"found; this is the first release. See {GUARD_REF_URL}"
            )
        return {**base, "go": True, "reason": reason}

    last_day = new_york_date(last_release_at)
    today = new_york_date(now)
    if last_day < today:
        if dry_run:
            reason = (
                f"dry run: last release was {last_day.isoformat()} (New York), "
                f"today is {today.isoformat()}; the next scheduled run would ship. "
                f"See {GUARD_REF_URL}"
            )
        else:
            reason = (
                f"go: last release was {last_day.isoformat()} (New York), "
                f"today is {today.isoformat()}. See {GUARD_REF_URL}"
            )
        return {**base, "go": True, "reason": reason}
    if last_day > today:
        return {
            **base,
            "go": False,
            "hard_fail": True,
            "reason": (
                f"lookup failed: last release date {last_day.isoformat()} is after "
                f"today {today.isoformat()} (clock skew?). Failing closed; "
                f"see {RECOVER_DOC_URL}"
            ),
        }
    # Same New York date.
    if override:
        return {
            **base,
            "go": True,
            "reason": (
                f"go (hotfix override): already released {today.isoformat()} "
                f"(New York) but proceeding because an override reason was given: "
                f"{override}. See {GUARD_REF_URL}"
            ),
        }
    return {
        **base,
        "go": False,
        "reason": (
            f"skip: already released {today.isoformat()} (New York); at most one "
            f"release per day without a hotfix override. See {GUARD_REF_URL}"
        ),
    }


def verdict_markdown(*, go: bool, dry_run: bool, reason: str, sha: str, mode: str) -> str:
    """Render the short Markdown verdict written to $GITHUB_STEP_SUMMARY."""
    if dry_run:
        headline = "### Train guard: DRY RUN"
    elif go:
        headline = "### Train guard: GO"
    else:
        headline = "### Train guard: SKIP"
    lines = [
        headline,
        "",
        f"- mode: `{mode}`",
        f"- sha: `{sha}`",
        f"- verdict: {reason}",
        "",
        f"Guard reference: {GUARD_REF_URL}",
        f"Pause/resume: {PAUSE_DOC_URL}",
    ]
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Live data fetching (impure; wrapped so --explain degrades gracefully)
# ---------------------------------------------------------------------------

def fetch_crates_pubtime(crate_name: str, timeout: float = 20.0) -> tuple[_dt.datetime | None, str | None]:
    """Fetch the newest pubtime from the crates.io sparse index.

    Returns `(timestamp_or_None, error_or_None)`. A 404 or any network /
    parse error returns an error string so the caller can fail closed.
    """
    url = f"https://index.crates.io/{sparse_index_path(crate_name)}"
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read().decode("utf-8", errors="replace")
    except Exception as error:  # noqa: BLE001 -- any lookup failure fails closed
        return None, f"could not fetch {url}: {error}"
    stamp = parse_index_pubtimes(body)
    if stamp is None and not body.strip():
        return None, f"index entry for {crate_name!r} is empty (HTTP 200 with no lines)"
    return stamp, None


def _run_gh(args: list[str], timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, check=False, text=True, capture_output=True, timeout=timeout)


def fetch_rehearsal_last_release(repo: str | None = None) -> tuple[_dt.datetime | None, str | None]:
    """Fetch the newest GitHub Release `createdAt` (drafts included).

    Returns `(timestamp_or_None, error_or_None)`. An empty release list is
    not an error (it means "no previous release"). Anything else that goes
    wrong returns an error string; the `--explain` path degrades gracefully
    while the real `plan` job surfaces the message.
    """
    command = ["gh", "release", "list", "--limit", "100", "--json", "createdAt,isDraft,isPrerelease"]
    if repo:
        command.extend(["--repo", repo])
    try:
        result = _run_gh(command)
    except FileNotFoundError:
        return None, "gh CLI is not installed"
    except Exception as error:  # noqa: BLE001
        return None, f"gh release list failed: {error}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "unknown error").strip()
        return None, f"gh release list failed: {detail}"
    return parse_gh_releases_json(result.stdout or "[]"), None


def sha_has_version_tag(sha: str) -> bool:
    """Return True when `sha` already carries a `v*` tag."""
    try:
        result = subprocess.run(
            ["git", "tag", "--points-at", sha, "--list", "v*"],
            check=False,
            text=True,
            capture_output=True,
        )
    except Exception:  # noqa: BLE001
        return False
    return bool(result.stdout.strip())


def append_output(name: str, value: str) -> None:
    """Append one `name=value` line to $GITHUB_OUTPUT (or stdout locally)."""
    path = os.environ.get("GITHUB_OUTPUT")
    line = f"{name}={value}\n"
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(line)
    else:
        sys.stdout.write(line)


def append_summary(markdown: str) -> None:
    """Append Markdown to $GITHUB_STEP_SUMMARY (or stdout locally)."""
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(markdown)
    else:
        sys.stdout.write(markdown)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", default=os.environ.get("GITHUB_REF", ""),
                        help="git ref being released (default $GITHUB_REF)")
    parser.add_argument("--sha", default=os.environ.get("GITHUB_SHA", ""),
                        help="exact commit SHA the train would ship (default $GITHUB_SHA)")
    parser.add_argument("--event-name", default=os.environ.get("GITHUB_EVENT_NAME", "schedule"),
                        help="GitHub event name (default $GITHUB_EVENT_NAME)")
    parser.add_argument("--dry-run", action="store_true",
                        help="force dry-run verdict (push events are always dry runs)")
    parser.add_argument("--no-dry-run", dest="dry_run", action="store_false",
                        help="clear --dry-run")
    parser.add_argument("--override-reason", default=os.environ.get("OVERRIDE_REASON", ""),
                        help="hotfix reason that bypasses the same-day ceiling")
    parser.add_argument("--paused-value", default=None,
                        help="value of the RELEASES_PAUSED repo variable "
                             "(default $RELEASES_PAUSED; unset means unarmed)")
    parser.add_argument("--release-plz-toml", default=str(DEFAULT_TOML),
                        help="path to release-plz.toml (default ./release-plz.toml)")
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""),
                        help="owner/repo for rehearsal Release lookup")
    parser.add_argument("--explain", action="store_true",
                        help="explain-only mode for `just release-preview`: fetch live "
                             "data, print the verdict, never touch git tags or releases")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.paused_value is None:
        args.paused_value = os.environ.get("RELEASES_PAUSED")
    # workflow_dispatch booleans arrive as "true"/"false" strings.
    if isinstance(args.dry_run, str):
        args.dry_run = args.dry_run.lower() == "true"
    if not args.dry_run:
        for env_key in ("DRY_RUN", "INPUT_DRY_RUN"):
            raw = os.environ.get(env_key, "")
            if raw.lower() == "true":
                args.dry_run = True
                break
    if not args.override_reason:
        for env_key in ("OVERRIDE_REASON", "INPUT_OVERRIDE_REASON"):
            if os.environ.get(env_key):
                args.override_reason = os.environ[env_key]
                break

    mode = read_mode(args.release_plz_toml)
    now = _dt.datetime.now(tz=_dt.timezone.utc)

    sha = args.sha or "HEAD"
    if args.explain:
        # Explain mode never fails closed on missing data: it shows what the
        # next scheduled run would decide and degrades without gh auth.
        if mode == "rehearsal":
            last, error = fetch_rehearsal_last_release(args.repo or None)
            if error:
                last, error_for_decide = None, None
                degraded = f" (Release lookup degraded: {error})"
            else:
                error_for_decide, degraded = None, ""
        else:
            crate = read_crate_name()
            last, error = fetch_crates_pubtime(crate)
            if error:
                last, error_for_decide = None, None
                degraded = f" (index lookup degraded: {error})"
            else:
                error_for_decide, degraded = None, ""
        tagged = False
        verdict = decide(
            mode=mode,
            paused_value=args.paused_value,
            ref=args.ref or "refs/heads/master",
            event_name=args.event_name,
            dry_run_input=True,
            override_reason=args.override_reason,
            sha_tagged=tagged,
            last_release_at=last,
            lookup_error=error_for_decide,
            now=now,
        )
        verdict["reason"] += degraded
        sys.stdout.write(verdict_markdown(go=verdict["go"], dry_run=True,
                                          reason=verdict["reason"], sha=sha, mode=mode))
        sys.stdout.write(f"override_reason={args.override_reason!r}\n")
        return 0

    # Real plan-job path: resolve the SHA, check tags, fetch the data source.
    if not args.sha:
        try:
            resolved = subprocess.run(["git", "rev-parse", "HEAD"], check=False,
                                      text=True, capture_output=True)
            sha = resolved.stdout.strip() or "HEAD"
        except Exception:  # noqa: BLE001
            sha = "HEAD"
    tagged = sha_has_version_tag(sha) if sha != "HEAD" else False

    lookup_error: str | None = None
    last: _dt.datetime | None = None
    if mode == "rehearsal":
        last, lookup_error = fetch_rehearsal_last_release(args.repo or None)
        # An empty release list is "no previous release", not an error.
        # fetch_rehearsal_last_release only returns an error string on failure.
    else:
        last, lookup_error = fetch_crates_pubtime(read_crate_name())

    verdict = decide(
        mode=mode,
        paused_value=args.paused_value,
        ref=args.ref,
        event_name=args.event_name,
        dry_run_input=bool(args.dry_run),
        override_reason=args.override_reason,
        sha_tagged=tagged,
        last_release_at=last,
        lookup_error=lookup_error,
        now=now,
    )

    append_output("sha", sha)
    append_output("go", "true" if verdict["go"] else "false")
    append_output("mode", verdict["mode"])
    append_output("dry_run", "true" if verdict["dry_run"] else "false")
    markdown = verdict_markdown(go=verdict["go"], dry_run=verdict["dry_run"],
                                reason=verdict["reason"], sha=sha, mode=verdict["mode"])
    append_summary(markdown)
    if verdict["go"] and verdict["dry_run"] is False and (args.override_reason or "").strip():
        warning = f"hotfix override: {(args.override_reason or '').strip()}"
        gh_warning = f"::warning::{warning}\n"
        if os.environ.get("GITHUB_ACTIONS"):
            sys.stdout.write(gh_warning)
        else:
            sys.stdout.write(f"WARNING: {warning}\n")
    if verdict.get("hard_fail"):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

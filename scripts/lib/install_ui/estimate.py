from __future__ import annotations

import json
import math
import os
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from statistics import median
from typing import Optional, Sequence

from install_ui import history, state, term


BAR_WIDTH = 24
BAR_MIN_COLS = 70
DISPERSION = 1.6
LONG_PHASE_SECS = 10.0
RANGE_SAMPLES = 5
SAMPLE_WINDOW_DAYS = 60
SAMPLE_LIMIT = 10
QUICK_FRACTION = 3.0
ETA_TEXT_INTERVAL = 10.0
FILL_CAP = 0.97

STEP_PHASES = {
    "bob-cli": ("index", "deps", "crate", "finish"),
    "bob-mac-capture": ("app", "helper", "install"),
    "bob-plugins": ("sync",),
    "obsidian": ("restart",),
}

STEP_COLORS = {
    "bob-cli": "35",
    "bob-plugins": "34",
    "bob-mac-capture": "32",
}


@dataclass(frozen=True)
class Estimate:
    text: str
    low: Optional[float] = None
    high: Optional[float] = None
    fill: Optional[float] = None
    usual_low: Optional[float] = None
    usual_high: Optional[float] = None
    rung: str = "learn"


def _timestamp(value: object) -> Optional[datetime]:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def _history_rows() -> list[dict]:
    result = state.read_history()
    if not result.ok:
        return []
    return list(result.value)


def _row_matches_key(row: dict, step: str, kind: str, key: dict) -> bool:
    if row.get("step") != step or row.get("kind") != kind:
        return False
    if row.get("outcome") != "ok" or row.get("slept"):
        return False
    if row.get("host") != key.get("host"):
        return False
    if row.get("toolchain") != key.get("toolchain"):
        return False
    if row.get("profile") != key.get("profile"):
        return False
    if row.get("incremental") != key.get("incremental"):
        return False
    return True


def _within_window(stamp: Optional[datetime], now: datetime) -> bool:
    return stamp is not None and now - stamp <= timedelta(days=SAMPLE_WINDOW_DAYS)


def _apply_quick_filter(samples: list[tuple[datetime, float]]) -> list[tuple[datetime, float]]:
    if not samples:
        return samples
    cutoff = median([value for _, value in samples]) / QUICK_FRACTION
    return [sample for sample in samples if sample[1] >= cutoff]


def _matching_runs(step: str, kind: str, key: dict) -> list[tuple[datetime, float]]:
    now = datetime.now(timezone.utc)
    grouped: dict[str, list[dict]] = {}
    dates: dict[str, datetime] = {}
    for row in _history_rows():
        if not _row_matches_key(row, step, kind, key):
            continue
        stamp = _timestamp(row.get("ts"))
        if not _within_window(stamp, now):
            continue
        run = str(row.get("run", ""))
        if not run:
            continue
        grouped.setdefault(run, []).append(row)
        dates[run] = max(stamp, dates.get(run, stamp))
    samples = []
    for run, rows in grouped.items():
        try:
            total = sum(max(0.0, float(row.get("secs", 0))) for row in rows)
        except (TypeError, ValueError):
            continue
        samples.append((dates[run], total))
    samples.sort(key=lambda item: item[0])
    samples = samples[-SAMPLE_LIMIT:]
    if kind == "rebuild":
        samples = _apply_quick_filter(samples)
    return samples


def _matching_phase_samples(
    step: str, kind: str, key: dict, phase: str
) -> list[float]:
    now = datetime.now(timezone.utc)
    samples: list[tuple[datetime, float]] = []
    for row in _history_rows():
        if not _row_matches_key(row, step, kind, key):
            continue
        if row.get("phase") != phase:
            continue
        stamp = _timestamp(row.get("ts"))
        if not _within_window(stamp, now):
            continue
        try:
            secs = max(0.0, float(row.get("secs", 0)))
        except (TypeError, ValueError):
            continue
        samples.append((stamp, secs))
    samples.sort(key=lambda item: item[0])
    samples = samples[-SAMPLE_LIMIT:]
    if kind == "rebuild":
        samples = _apply_quick_filter(samples)
    return [value for _, value in samples]


def percentile(values: Sequence[float], p: float) -> float:
    """Linear-interpolated percentile; *p* is 0–100."""
    ordered = sorted(float(v) for v in values)
    if not ordered:
        raise ValueError("percentile of empty sample")
    if len(ordered) == 1:
        return ordered[0]
    rank = (len(ordered) - 1) * (p / 100.0)
    lo = int(math.floor(rank))
    hi = int(math.ceil(rank))
    if lo == hi:
        return ordered[lo]
    frac = rank - lo
    return ordered[lo] * (1.0 - frac) + ordered[hi] * frac


def _phase_bounds(samples: Sequence[float]) -> Optional[tuple[float, float]]:
    if not samples:
        return None
    p20 = percentile(samples, 20)
    p80 = percentile(samples, 80)
    if p80 >= LONG_PHASE_SECS:
        if len(samples) < RANGE_SAMPLES:
            return None
        if p20 <= 0:
            return None
        if p80 / p20 > DISPERSION:
            return None
    elif len(samples) < 1:
        return None
    return (max(0.0, p20), max(p20, p80))


def _step_phases(step: str, phase: str, phases: Optional[Sequence[str]]) -> tuple[str, ...]:
    if phases:
        return tuple(phases)
    known = STEP_PHASES.get(step)
    if known:
        return known
    if phase:
        return (phase,)
    return ("crate",)


def _range_for_step(
    step: str,
    kind: str,
    key: dict,
    phases: Sequence[str],
) -> Optional[dict]:
    bounds = {}
    for name in phases:
        sample = _matching_phase_samples(step, kind, key, name)
        pair = _phase_bounds(sample)
        if pair is None:
            return None
        bounds[name] = pair
    usual_low = sum(pair[0] for pair in bounds.values())
    usual_high = sum(pair[1] for pair in bounds.values())
    return {"phases": bounds, "low": usual_low, "high": usual_high}


def format_remaining(low: float, high: float) -> str:
    """Remaining-time wording; never 0 or negative."""
    high = max(0.0, high)
    low = max(0.0, min(low, high))
    if high < 60:
        return "under a minute"
    if high < 120:
        return "about 1 min left"
    hi_m = max(2, int(round(high / 60.0)))
    if low < 90:
        return f"about {hi_m} min left"
    lo_m = max(2, int(round(low / 60.0)))
    if lo_m > hi_m:
        lo_m = hi_m
    if lo_m == hi_m:
        return f"about {lo_m} min left"
    return f"about {lo_m}–{hi_m} min left"


def format_minute_span(low: float, high: float) -> str:
    """Whole-minute span for plan and overtime labels."""
    high = max(0.0, high)
    low = max(0.0, min(low, high))
    if high < 60:
        return term.format_duration(high)
    if high < 120:
        return "1 min"
    hi_m = max(2, int(round(high / 60.0)))
    lo_m = max(1, int(round(low / 60.0)))
    if low < 90:
        lo_m = min(lo_m, hi_m)
        if lo_m < 2:
            return f"{hi_m} min"
    if lo_m > hi_m:
        lo_m = hi_m
    if lo_m == hi_m:
        return f"{lo_m} min"
    return f"{lo_m}–{hi_m} min"


def render_bar(fill: float) -> str:
    """24-cell bar: ━ body, ╸ tip, ─ remainder."""
    fill = max(0.0, min(FILL_CAP, fill))
    filled = int(round(fill * BAR_WIDTH))
    if filled <= 0:
        return "─" * BAR_WIDTH
    if filled >= BAR_WIDTH:
        filled = BAR_WIDTH - 1
    return "━" * (filled - 1) + "╸" + "─" * (BAR_WIDTH - filled)


def paint_bar(bar: str, color: str, *, colorize: Optional[bool] = None) -> str:
    if not bar:
        return bar
    tip_at = bar.find("╸")
    if tip_at == -1:
        empty = bar.find("─")
        if empty == -1:
            return term.paint(color, bar, colorize=colorize)
        solid, rest = bar[:empty], bar[empty:]
        return term.paint(color, solid, colorize=colorize) + term.paint(
            "2", rest, colorize=colorize
        )
    solid = bar[: tip_at + 1]
    rest = bar[tip_at + 1 :]
    return term.paint(color, solid, colorize=colorize) + term.paint(
        "2", rest, colorize=colorize
    )


def _clamp_fill(value: Optional[float], prev: Optional[float], overtime: bool) -> Optional[float]:
    if overtime:
        if prev is not None:
            return min(FILL_CAP, prev)
        if value is not None:
            return min(FILL_CAP, value)
        return FILL_CAP
    if value is None:
        return prev
    capped = max(0.0, min(FILL_CAP, value))
    if prev is None:
        return capped
    return max(prev, capped)


def estimate(
    step: str,
    kind: str,
    key: dict,
    elapsed: float,
    phase: str,
    *,
    phase_elapsed: Optional[float] = None,
    phases: Optional[Sequence[str]] = None,
    prev_fill: Optional[float] = None,
) -> Estimate:
    """Return the live ETA: learning, last-similar, range, or overtime."""
    elapsed = max(0.0, float(elapsed))
    samples = _matching_runs(step, kind, key)
    if not samples:
        noun = history.kind_noun(kind)
        return Estimate(f"first timed {noun} on {term.host_label()}", rung="learn")

    last_text = f"last similar build {term.format_duration(samples[-1][1])}"
    if len(samples) < RANGE_SAMPLES:
        return Estimate(last_text, rung="last")
    phase_names = _step_phases(step, phase, phases)
    ranged = _range_for_step(step, kind, key, phase_names)
    if ranged is None:
        return Estimate(last_text, rung="last")

    usual_low = float(ranged["low"])
    usual_high = float(ranged["high"])
    overtime = usual_high > 0 and elapsed > usual_high
    fill = None
    if usual_high > 0:
        fill = elapsed / usual_high
    fill = _clamp_fill(fill, prev_fill, overtime)
    if overtime:
        span = format_minute_span(usual_low, usual_high)
        return Estimate(
            f"taking longer than usual ({span})",
            low=0.0,
            high=0.0,
            fill=fill,
            usual_low=usual_low,
            usual_high=usual_high,
            rung="overtime",
        )

    current = phase if phase in phase_names else phase_names[0]
    current_index = phase_names.index(current)
    current_elapsed = elapsed if phase_elapsed is None else max(0.0, float(phase_elapsed))
    remain_low = 0.0
    remain_high = 0.0
    bounds = ranged["phases"]
    for index, name in enumerate(phase_names):
        p20, p80 = bounds[name]
        if index < current_index:
            continue
        if index == current_index:
            remain_low += max(0.0, p20 - current_elapsed)
            remain_high += max(0.0, p80 - current_elapsed)
        else:
            remain_low += p20
            remain_high += p80
    if remain_high <= 0:
        text = "under a minute"
    else:
        text = format_remaining(remain_low, remain_high)
    return Estimate(
        text,
        low=remain_low,
        high=remain_high,
        fill=fill,
        usual_low=usual_low,
        usual_high=usual_high,
        rung="range",
    )


def final_suffix(elapsed: float, est: Estimate) -> str:
    """Success-row comparison; only when the range rung applied."""
    if est.rung not in ("range", "overtime"):
        return ""
    if est.usual_low is None or est.usual_high is None:
        return ""
    return comparison_word(elapsed, est.usual_low, est.usual_high)


def comparison_word(elapsed: float, low: float, high: float) -> str:
    if elapsed < low:
        return "faster than usual"
    if elapsed > high:
        return "slower than usual"
    return "about usual"


def should_update_eta_text(
    previous: Optional[str],
    new: str,
    last_change: float,
    now: float,
    *,
    interval: float = ETA_TEXT_INTERVAL,
) -> bool:
    if previous is None or previous == "":
        return True
    if previous == new:
        return False
    if new.startswith("taking longer than usual"):
        return True
    return now - last_change >= interval


def _aux_median(step: str, phase: str) -> Optional[float]:
    now = datetime.now(timezone.utc)
    host = history.host_name()
    samples: list[tuple[datetime, float]] = []
    for row in _history_rows():
        if row.get("step") != step or row.get("phase") != phase:
            continue
        if row.get("outcome") != "ok" or row.get("slept"):
            continue
        if row.get("host") != host:
            continue
        stamp = _timestamp(row.get("ts"))
        if not _within_window(stamp, now):
            continue
        try:
            secs = max(0.0, float(row.get("secs", 0)))
        except (TypeError, ValueError):
            continue
        samples.append((stamp, secs))
    samples.sort(key=lambda item: item[0])
    values = [value for _, value in samples[-SAMPLE_LIMIT:]]
    if not values:
        return None
    return float(median(values))


def current_key(step: str, repo: str, profile: str) -> tuple[str, dict]:
    toolchain = history.probe_toolchain(step, repo)
    kind = history.predict_kind(step, repo, profile, toolchain=toolchain)
    key = {
        "host": history.host_name(),
        "toolchain": toolchain,
        "profile": profile,
        "incremental": history.incremental_for(step),
    }
    return kind, key


def plan_estimate(
    steps: Sequence[dict],
    aux: Sequence[tuple[str, str]] = (),
) -> Optional[dict]:
    """Return the whole-run range when every required step has one."""
    total_low = 0.0
    total_high = 0.0
    if not steps:
        return None
    for spec in steps:
        name = spec["name"]
        directory = spec["directory"]
        profile = spec.get("profile") or (
            "local-install" if name == "bob-cli" else "release"
        )
        parser = spec.get("parser") or (
            "cargo" if name == "bob-cli" else "swiftpm"
        )
        kind, key = current_key(name, directory, profile)
        if parser == "cargo":
            phases = STEP_PHASES["bob-cli"]
        elif parser == "swiftpm":
            phases = STEP_PHASES["bob-mac-capture"]
        else:
            phases = STEP_PHASES.get(name, ("crate",))
        ranged = _range_for_step(name, kind, key, phases)
        if ranged is None:
            return None
        total_low += float(ranged["low"])
        total_high += float(ranged["high"])
    for step_name, phase in aux:
        extra = _aux_median(step_name, phase)
        if extra is not None:
            total_low += extra
            total_high += extra
    return {"low": total_low, "high": total_high}


def plan_line_text(
    bounds: dict,
    *,
    now: Optional[datetime] = None,
    host: Optional[str] = None,
) -> str:
    span = format_minute_span(bounds["low"], bounds["high"])
    when = now if now is not None else datetime.now().astimezone()
    finish = when + timedelta(seconds=max(0.0, float(bounds["high"])))
    clock = finish.strftime("%H:%M")
    label = host if host is not None else term.host_label()
    return f"  ⏱  usually {span} on {label} · done by about {clock}"


def write_json(path: str, payload: dict) -> bool:
    try:
        directory = os.path.dirname(path)
        if directory:
            os.makedirs(directory, mode=0o700, exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass
        return True
    except OSError:
        return False


def load_json(path: str) -> Optional[dict]:
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, json.JSONDecodeError, ValueError, TypeError):
        return None
    return data if isinstance(data, dict) else None


def summary_clause(elapsed: float, record: dict) -> str:
    if not record.get("shown"):
        return ""
    try:
        low = float(record["low"])
        high = float(record["high"])
    except (KeyError, TypeError, ValueError):
        return ""
    word = comparison_word(elapsed, low, high)
    label = str(record.get("label") or format_minute_span(low, high))
    return f" · {word} ({label})"


def replay_needed(digest_rows: int, rows_after: int, summary_rows: int, lines: int) -> bool:
    return digest_rows + rows_after + summary_rows > lines


def replay_lines(record: dict, width: int) -> list[str]:
    rows = []
    content_width = max(width - 2, 1)
    for repo in record.get("repos") or []:
        if not isinstance(repo, dict):
            continue
        name = str(repo.get("name") or "")
        if not name:
            continue
        try:
            count = int(repo.get("count") or 0)
        except (TypeError, ValueError):
            count = 0
        try:
            breaking = int(repo.get("breaking") or 0)
        except (TypeError, ValueError):
            breaking = 0
        subjects = repo.get("subjects") or []
        head = f"what's new  {name} {count}"
        if subjects:
            subject = term.sanitize(str(subjects[0]))
            if breaking:
                subject = "⚠ " + subject
            extra = f", +{count - 1} more" if count > 1 else ""
            available = max(8, content_width - term.cell_width(head + " · ") - term.cell_width(extra))
            subject = term.truncate_cells(subject, available)
            line = f"{head} · {subject}{extra}"
        else:
            line = head
        rows.append("  " + term.truncate_cells(line, content_width))
    return rows

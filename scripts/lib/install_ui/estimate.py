from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from statistics import median
from typing import Optional

from install_ui import history, state, term


@dataclass(frozen=True)
class Estimate:
    text: str
    low: Optional[float] = None
    high: Optional[float] = None
    fill: Optional[float] = None


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


def _matching_runs(step: str, kind: str, key: dict) -> list[tuple[datetime, float]]:
    result = state.read_history()
    if not result.ok:
        return []
    now = datetime.now(timezone.utc)
    grouped: dict[str, list[dict]] = {}
    dates: dict[str, datetime] = {}
    for row in result.value:
        if row.get("step") != step or row.get("kind") != kind:
            continue
        if row.get("outcome") != "ok" or row.get("slept"):
            continue
        if row.get("host") != key.get("host"):
            continue
        if row.get("toolchain") != key.get("toolchain"):
            continue
        if row.get("profile") != key.get("profile"):
            continue
        if row.get("incremental") != key.get("incremental"):
            continue
        stamp = _timestamp(row.get("ts"))
        if stamp is None or now - stamp > timedelta(days=60):
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
    return samples[-10:]


def estimate(
    step: str,
    kind: str,
    key: dict,
    elapsed: float,
    phase: str,
) -> Estimate:
    """Return the live-runner learning rung; earned-eta adds range estimates later."""
    del elapsed, phase  # Reserved for the range estimator in the later earned-eta phase.
    samples = _matching_runs(step, kind, key)
    if kind == "rebuild" and samples:
        cutoff = median([value for _, value in samples]) / 3.0
        samples = [sample for sample in samples if sample[1] >= cutoff]
    if not samples:
        noun = history.kind_noun(kind)
        return Estimate(f"first timed {noun} on {term.host_label()}")
    return Estimate(f"last similar build {term.format_duration(samples[-1][1])}")

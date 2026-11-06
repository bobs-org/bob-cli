from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Optional


@dataclass(frozen=True)
class Pattern:
    name: str
    parser: str
    regex: re.Pattern
    phase: Optional[str] = None


@dataclass(frozen=True)
class ParsedLine:
    chatter: bool
    phase: Optional[str] = None
    lock_wait: bool = False
    app_path: bool = False


# Keep the chatter grammar in one table so captured Cargo and SwiftPM output can
# be reviewed and extended without scattering renderer heuristics.
CHATTER_PATTERNS = (
    Pattern("cargo-lock-wait", "cargo", re.compile(r"^Blocking waiting for file lock\b"), None),
    Pattern("cargo-index", "cargo", re.compile(r"^(?:Updating\b|Locking\b|Downloading\b|Downloaded\b)"), "index"),
    Pattern("cargo-building-frame", "cargo", re.compile(r"^Building\s*\["), "deps"),
    Pattern("cargo-compiling", "cargo", re.compile(r"^Compiling\s+([^\s]+)"), "deps"),
    Pattern("cargo-fresh", "cargo", re.compile(r"^Fresh\s+"), "deps"),
    Pattern("cargo-adding", "cargo", re.compile(r"^Adding\s+"), "deps"),
    Pattern("cargo-finished", "cargo", re.compile(r"^Finished\b"), "finish"),
    Pattern("cargo-installing", "cargo", re.compile(r"^(?:Installing|Replacing|Installed)\b"), "finish"),
    Pattern("swiftpm-progress", "swiftpm", re.compile(r"^\[\s*\d+\s*/\s*\d+\s*\]"), None),
    Pattern("swiftpm-build-start", "swiftpm", re.compile(r"^Building for production\b"), None),
    Pattern("swiftpm-build-complete", "swiftpm", re.compile(r"^Build complete!"), None),
    Pattern(
        "swiftpm-status",
        "swiftpm",
        re.compile(r"^(?:Compiling|Linking|Write|Emitting module|Planning build|Fetching|Cloning|Resolving|Computing|Applying)\b"),
        None,
    ),
    Pattern("mac-app-path", "swiftpm", re.compile(r"^/.+\.app/?$"), "install"),
)

_CARGO_COMPILE = re.compile(r"^Compiling\s+([^\s]+)")
_COUNTER = re.compile(r"(?:^|\s)(\d+)/(\d+)(?=\s|[:)]|$)")


class PhaseDetector:
    def __init__(self, parser: str, crate: str = "bob-obsidian") -> None:
        if parser not in ("cargo", "swiftpm"):
            raise ValueError("parser must be cargo or swiftpm")
        self.parser = parser
        self.crate = crate
        self.swift_builds = 0

    @property
    def phases(self) -> tuple[str, ...]:
        if self.parser == "cargo":
            return ("index", "deps", "crate", "finish")
        return ("app", "helper", "install")

    def parse(self, line: str) -> ParsedLine:
        text = line.strip()
        if self.parser == "cargo":
            if text.startswith("Blocking waiting for file lock"):
                return ParsedLine(True, lock_wait=True)
            match = _CARGO_COMPILE.match(text)
            if match:
                crate_name = match.group(1)
                phase = "crate" if crate_name == self.crate else "deps"
                return ParsedLine(True, phase)
            if text.startswith("Building ["):
                phase = "crate" if self.crate in text else "deps"
                return ParsedLine(True, phase)
            for pattern in CHATTER_PATTERNS:
                if pattern.parser != "cargo" or pattern.name == "cargo-lock-wait" or pattern.name == "cargo-compiling":
                    continue
                if pattern.regex.search(text):
                    return ParsedLine(True, pattern.phase)
            return ParsedLine(False)

        if text.startswith("Building for production"):
            self.swift_builds += 1
            return ParsedLine(True, "app" if self.swift_builds == 1 else "helper")
        if text.startswith("Build complete!"):
            phase = "app" if self.swift_builds <= 1 else "install"
            return ParsedLine(True, phase)
        for pattern in CHATTER_PATTERNS:
            if pattern.parser != "swiftpm" or pattern.name == "swiftpm-build-start" or pattern.name == "swiftpm-build-complete":
                continue
            if pattern.regex.search(text):
                return ParsedLine(True, pattern.phase, app_path=pattern.name == "mac-app-path")
        return ParsedLine(False)


def progress_counter(line: str) -> Optional[tuple[int, int]]:
    match = _COUNTER.search(line)
    if not match:
        return None
    return int(match.group(1)), int(match.group(2))

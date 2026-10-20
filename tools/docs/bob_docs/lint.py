"""Enforce Bob's documentation contract.

Ported from sase-org/sase ``tools/check_docs`` (MIT License, Copyright 2026
Bryan Bugyi) and retargeted to Bob's directory model, nav completeness,
portability markup, media provenance, and privacy baseline.
"""

from __future__ import annotations

import fnmatch
import json
import math
import re
from dataclasses import dataclass
from datetime import date as date_type
from pathlib import Path, PurePosixPath
from typing import Any, Iterable

from .context import DocsContext, context_from_root
from .contracts import ContractRegistry, load_contract_registry
from .errors import DocsError
from .environment import check_environment_coverage
from .ignore import path_ignored as _path_ignored
from .markdown import (
    blank_link_destinations,
    count_words,
    fence_language,
    is_whole_file_generated,
    iter_fences,
    line_at,
    mask_for_hygiene,
    parse_front_matter,
    split_front_matter,
)
from .mounts import Mount, load_mounts
from .yaml_tools import dump_yaml, load_yaml

TYPED_DIRS = ("start", "guides", "concepts", "reference", "develop")
GUIDE_DIRS = ("start", "guides", "concepts")
SUPPORT_PREFIXES = ("_templates/", "_meta/", "overrides/")
EXCLUDED_NAMES = frozenset({"README.md"})
ALLOWED_FRONTMATTER = frozenset(
    {"title", "description", "hide", "search", "tags", "template", "icon", "status"}
)
PLUGIN_ALLOWLIST = frozenset({"search", "redirects", "tags", "llmstxt"})
DESCRIPTION_LIMIT = 160
GUIDES_TARGET = 1500
GUIDES_HARD = 2500
HUB_CAP = 400
HOMEPAGE_CAP = 400
PAGE_BUDGETS: dict[str, tuple[int | None, int | None, str]] = {
    "start": (2500, 10000, "error"),
    "guides": (GUIDES_HARD, None, "error"),
    "concepts": (3000, None, "error"),
    "reference": (6000, None, "error"),
    "develop": (8000, None, "warning"),
}
IMAGE_SUFFIXES = frozenset({".png", ".svg", ".jpg", ".jpeg", ".gif", ".webp", ".avif"})
SHELL_LANGS = frozenset({"bash", "sh", "shell", "zsh", "console"})
TEST_PIN_RE = re.compile(r"docs[\\/](start|guides|concepts)[\\/]")
TEST_PIN_EXEMPTIONS = frozenset({"tests/cli/docs_tutorial.rs"})
JS_TEST_SUFFIXES = (
    ".test.js",
    ".test.mjs",
    ".test.cjs",
    ".test.ts",
    ".spec.js",
    ".spec.mjs",
    ".spec.cjs",
    ".spec.ts",
)
SKIP_DIR_NAMES = frozenset({".git", "node_modules", "dist", "coverage", ".venv", "target"})
BEAD_ID_RE = re.compile(
    r"\b(?:bob-cli|bob-plugins|bob-mac-capture)-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*\b"
)
SRC_PATH_RE = re.compile(r"(?<![\w/])src/[\w\-.]+")
PHASE_LABEL_RE = re.compile(
    r"\bE\d+\b|\b(?:epic|phase)\s+(?:bob-cli|bob-plugins|bob-mac-capture)-",
    re.IGNORECASE,
)
HISTORY_RE = re.compile(r"\b(formerly|no longer|legacy|renamed)\b", re.IGNORECASE)
ADMONITION_RE = re.compile(r"(?m)^[ \t]*!!![ \t]+\S")
TAB_RE = re.compile(r"(?m)^[ \t]*===[ \t]+\S")
KEY_RE = re.compile(r"\+\+[A-Za-z][A-Za-z0-9_+-]*\+\+")
PROMPT_RE = re.compile(r"(?m)^[ \t]*\$[ \t]+\S")

GUIDE_HYGIENE = (
    ("bead-id", BEAD_ID_RE, "reader pages must not name bead IDs ({hit})"),
    ("src-path", SRC_PATH_RE, "reader pages must not cite src/ paths ({hit})"),
    ("phase-label", PHASE_LABEL_RE, "reader pages must not carry epic or phase labels ({hit})"),
    ("history-phrase", HISTORY_RE, "reader pages must not carry history phrases ({hit})"),
)
REFERENCE_HYGIENE = (
    ("bead-id", BEAD_ID_RE, "reference pages must not name bead IDs ({hit})"),
)


@dataclass(frozen=True)
class Finding:
    """One contract violation or advisory."""

    path: str
    line: int
    rule: str
    explanation: str
    severity: str = "error"

    def format(self) -> str:
        return f"{self.path}:{self.line}: {self.rule}: {self.explanation}"


@dataclass(frozen=True)
class PrivacyPattern:
    ident: str
    description: str
    regex: re.Pattern[str]
    allow: tuple[re.Pattern[str], ...]
    placeholder: str | None


@dataclass(frozen=True)
class MediaEntry:
    path: str
    source: str
    date: str


@dataclass
class LintContext:
    root: Path
    docs: Path
    config_path: Path
    config: dict[str, Any]
    registry: ContractRegistry
    mounts: list[Mount]
    ceilings: dict[str, int]
    budgets_path: Path
    exclude_globs: tuple[str, ...]
    privacy_patterns: tuple[PrivacyPattern, ...]
    baseline_path: Path
    baseline: dict[tuple[str, str, str], int]
    media: dict[str, MediaEntry]
    media_paths: tuple[Path, ...]
    source_dirs: dict[str, Path]
    source_stamps: dict[str, str]
    assembled: bool


def _finding(path: str, line: int, rule: str, explanation: str, *, severity: str = "error") -> Finding:
    return Finding(path, line, rule, explanation, severity)


def load_exclude_globs(config: dict[str, Any]) -> tuple[str, ...]:
    globs = config.get("exclude_docs", [])
    if isinstance(globs, str):
        globs = [line.strip() for line in globs.splitlines() if line.strip()]
    if not isinstance(globs, list):
        return ()
    return tuple(str(glob) for glob in globs if isinstance(glob, str))


def _is_support(rel: str) -> bool:
    return rel in EXCLUDED_NAMES or any(rel.startswith(prefix) for prefix in SUPPORT_PREFIXES)


def _is_excluded(rel: str, globs: tuple[str, ...]) -> bool:
    return any(fnmatch.fnmatch(rel, glob) or fnmatch.fnmatch("/" + rel, glob) for glob in globs)


def _tier_of(rel: str) -> str | None:
    top = rel.split("/", 1)[0]
    return top if top in TYPED_DIRS else None


def _is_hub(rel: str) -> bool:
    return rel.rsplit("/", 1)[-1] == "index.md" and "/" in rel


def _is_homepage(rel: str) -> bool:
    return rel == "index.md"


def _docs_path(rel: str) -> str:
    return f"docs/{rel}"


def _contract_stems(registry: ContractRegistry) -> set[str]:
    return {entry.stem for entry in registry.entries}


def _is_contract(rel: str, stems: set[str]) -> bool:
    return "/" not in rel and rel.endswith(".md") and rel[:-3] in stems


def _under_mount(rel: str, mounts: list[Mount]) -> Mount | None:
    posix = PurePosixPath(rel)
    for mount in mounts:
        target = mount.target
        if posix == target or posix == target / "index.md":
            return mount
        try:
            posix.relative_to(target)
        except ValueError:
            continue
        else:
            return mount
    return None


def _is_mount_landing(rel: str, mounts: list[Mount]) -> bool:
    posix = PurePosixPath(rel)
    return any(posix == mount.target / "index.md" for mount in mounts)


def _is_verified_copy(
    rel: str,
    source: object,
    mounts: list[Mount],
    source_stamps: dict[str, str],
) -> bool:
    if not isinstance(source, str):
        return False
    if _is_mount_landing(rel, mounts):
        return False
    mount = _under_mount(rel, mounts)
    if mount is None:
        return False
    expected = source_stamps.get(rel)
    return expected is not None and source == expected and source.partition(":")[0] == mount.repo


def publishable_markdown(docs: Path, exclude_globs: tuple[str, ...]) -> list[str]:
    pages: list[str] = []
    if not docs.is_dir():
        return pages
    for path in sorted(docs.rglob("*.md")):
        if path.is_symlink() or not path.is_file():
            continue
        rel = path.relative_to(docs).as_posix()
        if _is_support(rel) or _is_excluded(rel, exclude_globs):
            continue
        pages.append(rel)
    return pages


def _read(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return None


def load_budgets(path: Path) -> dict[str, int]:
    if not path.is_file():
        return {}
    raw = load_yaml(path)
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise DocsError(f"{path}: budgets.yml must be a mapping")
    ceilings = raw.get("ceilings", {})
    if ceilings is None:
        return {}
    if not isinstance(ceilings, dict):
        raise DocsError(f"{path}: ceilings must be a mapping of path to integer")
    parsed: dict[str, int] = {}
    for key, value in ceilings.items():
        if not isinstance(key, str) or not key:
            raise DocsError(f"{path}: ceiling keys must be nonempty path strings")
        if isinstance(value, bool) or not isinstance(value, int) or value < 1:
            raise DocsError(f"{path}: ceiling {key!r} must be a positive integer")
        parsed[key] = value
    return parsed


def load_media(path: Path) -> dict[str, MediaEntry]:
    if not path.is_file():
        return {}
    raw = load_yaml(path)
    if raw is None:
        return {}
    if not isinstance(raw, dict) or not all(isinstance(key, str) for key in raw):
        raise DocsError(f"{path}: media.yml must map asset paths to provenance")
    entries: dict[str, MediaEntry] = {}
    for asset, value in raw.items():
        where = f"{path}: {asset}"
        if not isinstance(value, dict):
            raise DocsError(f"{where}: expected a mapping with source and date")
        source = value.get("source")
        date = value.get("date")
        extra = set(value) - {"source", "date"}
        if extra:
            raise DocsError(f"{where}: unknown keys: {', '.join(sorted(extra))}")
        if not isinstance(source, str) or not source.strip():
            raise DocsError(f"{where}: source must be a nonempty string")
        if isinstance(date, date_type):
            date = date.isoformat()
        if not isinstance(date, str) or not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
            raise DocsError(f"{where}: date must be an ISO date (YYYY-MM-DD)")
        entries[asset] = MediaEntry(asset, source.strip(), date)
    return entries


def load_privacy_patterns(path: Path) -> tuple[PrivacyPattern, ...]:
    raw = load_yaml(path)
    if not isinstance(raw, dict):
        raise DocsError(f"{path}: privacy.yml must be a mapping")
    placeholders = raw.get("placeholders", {})
    if placeholders is None:
        placeholders = {}
    if not isinstance(placeholders, dict) or not all(isinstance(key, str) for key in placeholders):
        raise DocsError(f"{path}: placeholders must be a string mapping")
    patterns_raw = raw.get("patterns")
    if not isinstance(patterns_raw, list) or not patterns_raw:
        raise DocsError(f"{path}: patterns must be a nonempty list")
    patterns: list[PrivacyPattern] = []
    seen: set[str] = set()
    for index, item in enumerate(patterns_raw, start=1):
        where = f"{path}: patterns[{index}]"
        if not isinstance(item, dict):
            raise DocsError(f"{where}: expected a mapping")
        ident = item.get("id")
        description = item.get("description")
        regex = item.get("regex")
        if not isinstance(ident, str) or not ident.strip():
            raise DocsError(f"{where}: id must be a nonempty string")
        if ident in seen:
            raise DocsError(f"{where}: duplicate pattern id {ident!r}")
        seen.add(ident)
        if not isinstance(description, str) or not description.strip():
            raise DocsError(f"{where}: description must be a nonempty string")
        if not isinstance(regex, str) or not regex:
            raise DocsError(f"{where}: regex must be a nonempty string")
        try:
            compiled = re.compile(regex)
        except re.error as error:
            raise DocsError(f"{where}: invalid regex: {error}") from error
        allow_raw = item.get("allow", [])
        if allow_raw is None:
            allow_raw = []
        if not isinstance(allow_raw, list) or not all(isinstance(entry, str) for entry in allow_raw):
            raise DocsError(f"{where}: allow must be a list of regex strings")
        allow: list[re.Pattern[str]] = []
        for allow_index, allow_pat in enumerate(allow_raw, start=1):
            try:
                allow.append(re.compile(allow_pat))
            except re.error as error:
                raise DocsError(f"{where}: allow[{allow_index}] invalid regex: {error}") from error
        placeholder = item.get("placeholder", placeholders.get(ident))
        if placeholder is not None and (not isinstance(placeholder, str) or not placeholder.strip()):
            raise DocsError(f"{where}: placeholder must be a nonempty string")
        extra = set(item) - {"id", "description", "regex", "allow", "placeholder"}
        if extra:
            raise DocsError(f"{where}: unknown keys: {', '.join(sorted(extra))}")
        patterns.append(
            PrivacyPattern(
                ident.strip(),
                description.strip(),
                compiled,
                tuple(allow),
                placeholder.strip() if isinstance(placeholder, str) else None,
            )
        )
    unused = set(raw) - {"placeholders", "patterns"}
    if unused:
        raise DocsError(f"{path}: unknown keys: {', '.join(sorted(unused))}")
    return tuple(patterns)


def load_privacy_baseline(path: Path) -> dict[tuple[str, str, str], int]:
    if not path.is_file():
        return {}
    text = path.read_text(encoding="utf-8")
    rows: dict[tuple[str, str, str], int] = {}
    for lineno, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) != 4:
            raise DocsError(f"{path}:{lineno}: expected path, rule, count, matched columns")
        rel, rule, count_text, matched = parts
        if not rel or not rule:
            raise DocsError(f"{path}:{lineno}: path and rule must be nonempty")
        try:
            count = int(count_text)
        except ValueError as error:
            raise DocsError(f"{path}:{lineno}: count must be an integer") from error
        if count < 1:
            raise DocsError(f"{path}:{lineno}: count must be positive")
        key = (rel, rule, matched)
        if key in rows:
            raise DocsError(f"{path}:{lineno}: duplicate baseline row for {rel} {rule}")
        rows[key] = count
    return rows


def dump_privacy_baseline(rows: dict[tuple[str, str, str], int]) -> str:
    lines = [
        "# Contract privacy baseline. Columns: path, rule, count, matched.",
        "# Counts ignore line movement. A second copy of the same text is a new hit.",
        "# Normal lint never adds rows. bob-docs lint --fix only removes obsolete hits",
        "# or lowers a count that shrank.",
    ]
    for rel, rule, matched in sorted(rows):
        lines.append(f"{rel}\t{rule}\t{rows[(rel, rule, matched)]}\t{matched}")
    return "\n".join(lines) + "\n"


def _plugin_entries(config: dict[str, Any]) -> list[tuple[str, Any]]:
    plugins = config.get("plugins", [])
    entries: list[tuple[str, Any]] = []
    if not isinstance(plugins, list):
        return entries
    for item in plugins:
        if isinstance(item, str):
            entries.append((item, None))
        elif isinstance(item, dict) and item:
            name, options = next(iter(item.items()))
            if isinstance(name, str):
                entries.append((name, options))
    return entries


def _redirect_sources(config: dict[str, Any]) -> set[str]:
    sources: set[str] = set()
    for name, options in _plugin_entries(config):
        if name != "redirects" or not isinstance(options, dict):
            continue
        mapping = options.get("redirect_maps", {})
        if isinstance(mapping, dict):
            for src in mapping:
                if isinstance(src, str) and src.endswith(".md"):
                    sources.add(src)
    return sources


def _nav_paths(nav: Any) -> list[str]:
    paths: list[str] = []
    if isinstance(nav, list):
        for item in nav:
            paths.extend(_nav_paths(item))
    elif isinstance(nav, dict):
        for value in nav.values():
            if isinstance(value, str):
                paths.append(value)
            else:
                paths.extend(_nav_paths(value))
    elif isinstance(nav, str):
        paths.append(nav)
    return paths


def _nav_sections(nav: Any) -> list[tuple[str, list[str]]]:
    if not isinstance(nav, list):
        return []
    sections: list[tuple[str, list[str]]] = []
    for item in nav:
        if isinstance(item, str):
            sections.append(("Untitled", [item]))
        elif isinstance(item, dict) and item:
            title, value = next(iter(item.items()))
            title_text = title if isinstance(title, str) else "Untitled"
            if isinstance(value, str):
                sections.append((title_text, [value]))
            else:
                sections.append((title_text, _nav_paths(value)))
    return sections


def build_lint_context(root: Path, *, docs_context: DocsContext | None = None) -> LintContext:
    docs_context = docs_context or context_from_root(root)
    registry = load_contract_registry(docs_context.repo_root)
    mounts = load_mounts(docs_context.repo_root)
    docs = docs_context.docs_root
    config = docs_context.config
    source_stamps: dict[str, str] = {}
    if docs_context.assembled:
        docs = docs_context.output_dir / "docs"
        assembled_config = load_yaml(docs_context.assembled_config)
        if not isinstance(assembled_config, dict):
            raise DocsError(f"{docs_context.assembled_config}: assembled config must be a mapping")
        config = assembled_config
        if not docs.is_dir():
            raise DocsError(f"assembled docs tree is missing: {docs}")
    if docs_context.assembled:
        manifest_path = docs_context.output_dir / "mount-manifest.json"
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except OSError as error:
            raise DocsError(f"cannot read assembled mount manifest {manifest_path}: {error}") from error
        except ValueError as error:
            raise DocsError(f"assembled mount manifest {manifest_path} is invalid JSON: {error}") from error
        raw_stamps = manifest.get("sources") if isinstance(manifest, dict) else None
        if not isinstance(raw_stamps, dict) or not all(
            isinstance(key, str) and isinstance(value, str) for key, value in raw_stamps.items()
        ):
            raise DocsError(
                f"assembled mount manifest {manifest_path} must map copied paths to source stamps"
            )
        source_stamps = raw_stamps
    meta = docs_context.docs_root / "_meta"
    budgets_path = meta / "budgets.yml"
    privacy_path = meta / "privacy.yml"
    baseline_path = meta / "privacy_baseline.txt"
    media_path = meta / "media.yml"
    media = load_media(media_path)
    media_paths = [media_path]
    for mount in mounts:
        repo_dir = docs_context.source_dirs.get(mount.repo)
        if repo_dir is None:
            continue
        mounted_media = repo_dir / Path(*mount.source.parts) / "media.yml"
        if mounted_media.is_file():
            for key, entry in load_media(mounted_media).items():
                media[f"{mount.target.as_posix()}/{key}"] = entry
            media_paths.append(mounted_media)
    return LintContext(
        root=docs_context.repo_root,
        docs=docs,
        config_path=docs_context.authored_config,
        config=config,
        registry=registry,
        mounts=mounts,
        ceilings=load_budgets(budgets_path),
        budgets_path=budgets_path,
        exclude_globs=load_exclude_globs(config),
        privacy_patterns=load_privacy_patterns(privacy_path),
        baseline_path=baseline_path,
        baseline=load_privacy_baseline(baseline_path),
        media=media,
        media_paths=tuple(media_paths),
        source_dirs=docs_context.source_dirs,
        source_stamps=source_stamps,
        assembled=docs_context.assembled,
    )


def check_registry(ctx: LintContext) -> list[Finding]:
    findings: list[Finding] = []
    stems = _contract_stems(ctx.registry)
    typed = set(TYPED_DIRS)
    mount_roots = {mount.target.parts[0] for mount in ctx.mounts if mount.target.parts}
    path = "docs/_meta/contracts.yml"
    for stem in sorted(stems):
        if stem in typed:
            findings.append(
                _finding(path, 1, "registry", f"contract stem {stem!r} collides with a typed directory")
            )
        if stem in mount_roots:
            findings.append(
                _finding(path, 1, "registry", f"contract stem {stem!r} collides with a mount root")
            )
        page = ctx.docs / f"{stem}.md"
        if not page.is_file():
            findings.append(
                _finding(path, 1, "registry", f"registered contract page docs/{stem}.md is missing")
            )
    return findings


def check_mount_gitignores(ctx: LintContext) -> list[Finding]:
    findings: list[Finding] = []
    for mount in ctx.mounts:
        probe = f"docs/{mount.target.as_posix()}/.__bob_docs_mount_probe.md"
        landing = f"docs/{mount.target.as_posix()}/index.md"
        if not _path_ignored(ctx.root, probe):
            findings.append(
                _finding(
                    "docs/_meta/mounts.yml",
                    1,
                    "gitignore",
                    f"mount target {mount.target.as_posix()!r} is not gitignored; "
                    "copied pages must be ignored apart from index.md",
                )
            )
        if _path_ignored(ctx.root, landing):
            findings.append(
                _finding(
                    "docs/_meta/mounts.yml",
                    1,
                    "gitignore",
                    f"mount landing page {landing} must stay tracked (not gitignored)",
                )
            )
    return findings


def check_typing(rel: str, ctx: LintContext) -> Finding | None:
    stems = _contract_stems(ctx.registry)
    if _is_homepage(rel) or _is_contract(rel, stems) or _tier_of(rel) is not None:
        return None
    if rel in _redirect_sources(ctx.config):
        return None
    mount = _under_mount(rel, ctx.mounts)
    if mount is not None:
        return None
    return _finding(
        _docs_path(rel),
        1,
        "typing",
        "untyped location; only docs/index.md and registered contracts may live at "
        f"the docs root, and typed pages belong under {', '.join(d + '/' for d in TYPED_DIRS)}",
    )


def check_front_matter(
    rel: str,
    text: str,
    *,
    ctx: LintContext,
    contract: bool,
) -> list[Finding]:
    findings: list[Finding] = []
    label = _docs_path(rel)
    head, _body, error = parse_front_matter(text, source=label)
    if error:
        findings.append(_finding(label, 1, "yaml", f"malformed front matter: {error}"))
        return findings
    if head is None:
        head = {}
        if not contract:
            findings.append(_finding(label, 1, "front-matter", "missing front matter key 'title'"))
            findings.append(_finding(label, 1, "front-matter", "missing front matter key 'description'"))
            return findings
        return findings
    source = head.get("source")
    if "source" in head:
        if not _is_verified_copy(rel, source, ctx.mounts, ctx.source_stamps):
            findings.append(
                _finding(
                    label,
                    1,
                    "source",
                    "source is legal only on verified assembled mount copies; "
                    "this stamp is forged or unauthenticated",
                )
            )
    for key in sorted(set(head) - {"source"}):
        if key not in ALLOWED_FRONTMATTER:
            findings.append(
                _finding(
                    label,
                    1,
                    "front-matter",
                    f"unknown front matter key '{key}'. Allowed keys: "
                    f"{', '.join(sorted(ALLOWED_FRONTMATTER))}",
                )
            )
    if contract:
        return findings
    title = head.get("title")
    if not isinstance(title, str) or not title.strip():
        findings.append(
            _finding(label, 1, "front-matter", "missing front matter key 'title' (must be a string)")
        )
    description = head.get("description")
    if not isinstance(description, str) or not description.strip():
        findings.append(
            _finding(
                label,
                1,
                "front-matter",
                "missing front matter key 'description' (must be a string)",
            )
        )
    elif "\n" in description or len(description) > DESCRIPTION_LIMIT:
        findings.append(
            _finding(
                label,
                1,
                "front-matter",
                f"'description' must be one line of at most {DESCRIPTION_LIMIT} characters "
                f"(it is {len(description)})",
            )
        )
    return findings


def check_budget(
    rel: str,
    words: int,
    *,
    tier: str | None,
    contract: bool,
    generated: bool,
    ceilings: dict[str, int],
    tier_totals: dict[str, int],
) -> list[Finding]:
    if contract or generated:
        return []
    findings: list[Finding] = []
    label = _docs_path(rel)
    ceiling = ceilings.get(rel)
    if _is_homepage(rel) and words > HOMEPAGE_CAP:
        findings.append(
            _finding(label, 1, "budget", f"homepage is over its {HOMEPAGE_CAP}-word cap (it is {words})")
        )
    if _is_hub(rel) and words > HUB_CAP:
        findings.append(
            _finding(label, 1, "budget", f"hub pages hold at most {HUB_CAP} words (it is {words})")
        )
    if tier is not None:
        page_cap, _tier_cap, severity = PAGE_BUDGETS[tier]
        if page_cap is not None and words > page_cap:
            findings.append(
                _finding(
                    label,
                    1,
                    "budget",
                    f"over budget at {words} words (cap {page_cap} for {tier}/)",
                    severity=severity,
                )
            )
        elif tier == "guides" and words > GUIDES_TARGET:
            findings.append(
                _finding(
                    label,
                    1,
                    "budget",
                    f"{words} words is past the {GUIDES_TARGET}-word guides target; "
                    f"split before it hits the {GUIDES_HARD}-word cap",
                    severity="warning",
                )
            )
        tier_totals[tier] = tier_totals.get(tier, 0) + words
    if ceiling is not None and words > ceiling:
        findings.append(
            _finding(label, 1, "budget", f"over budget at {words} words (ceiling {ceiling})")
        )
    return findings


def _hygiene_hits(text: str, patterns: Iterable[tuple[str, re.Pattern[str], str]]) -> list[tuple[int, str]]:
    masked = blank_link_destinations(mask_for_hygiene(text))
    hits: list[tuple[int, str]] = []
    for lineno, line in enumerate(masked.splitlines(), start=1):
        for _kind, pattern, template in patterns:
            match = pattern.search(line)
            if match:
                hits.append((lineno, template.format(hit=match.group(0))))
    return hits


def check_hygiene(rel: str, text: str, *, tier: str | None) -> list[Finding]:
    if tier in GUIDE_DIRS:
        patterns = GUIDE_HYGIENE
    elif tier == "reference":
        patterns = REFERENCE_HYGIENE
    else:
        return []
    return [
        _finding(_docs_path(rel), lineno, "hygiene", explanation)
        for lineno, explanation in _hygiene_hits(text, patterns)
    ]


def check_portability_markup(rel: str, text: str) -> list[Finding]:
    findings: list[Finding] = []
    label = _docs_path(rel)
    _, body, body_start = split_front_matter(text)
    masked = mask_for_hygiene(text)
    for match in ADMONITION_RE.finditer(masked):
        findings.append(
            _finding(
                label,
                line_at(masked, match.start()),
                "portability",
                "rendered !!! admonitions are forbidden; use GitHub > [!NOTE] callouts",
            )
        )
    for match in TAB_RE.finditer(masked):
        findings.append(
            _finding(
                label,
                line_at(masked, match.start()),
                "portability",
                "rendered === content tabs are forbidden",
            )
        )
    for match in KEY_RE.finditer(masked):
        findings.append(
            _finding(
                label,
                line_at(masked, match.start()),
                "portability",
                f"++key++ markup is forbidden ({match.group(0)}); use <kbd>",
            )
        )
    for fence in iter_fences(body):
        if fence_language(fence.info) not in SHELL_LANGS:
            continue
        for match in PROMPT_RE.finditer(fence.body):
            findings.append(
                _finding(
                    label,
                    line_at(text, body_start + fence.body_start + match.start()),
                    "portability",
                    "shell fences must not start commands with a $ prompt",
                )
            )
    return findings


def check_portability_config(ctx: LintContext) -> list[Finding]:
    findings: list[Finding] = []
    label = ctx.config_path.name
    if "hooks" in ctx.config:
        findings.append(
            _finding(
                label,
                1,
                "portability",
                "'hooks:' is forbidden (Zensical portability); generate in tools/ before the build",
            )
        )
    for name, _options in _plugin_entries(ctx.config):
        if name not in PLUGIN_ALLOWLIST:
            findings.append(
                _finding(
                    label,
                    1,
                    "portability",
                    f"plugin {name!r} is outside the allowlist "
                    f"({', '.join(sorted(PLUGIN_ALLOWLIST))})",
                )
            )
    return findings


def check_nav(ctx: LintContext, pages: list[str]) -> list[Finding]:
    findings: list[Finding] = []
    nav = ctx.config.get("nav")
    label = ctx.config_path.name
    if not isinstance(nav, list) or not nav:
        findings.append(_finding(label, 1, "nav", "mkdocs.yml nav must be a nonempty list"))
        return findings
    leaves = _nav_paths(nav)
    redirects = _redirect_sources(ctx.config)
    mount_placeholders = {f"{mount.target.as_posix()}/index.md" for mount in ctx.mounts}
    seen: dict[str, int] = {}
    for leaf in leaves:
        seen[leaf] = seen.get(leaf, 0) + 1
        page = ctx.docs / Path(*PurePosixPath(leaf).parts)
        if leaf in mount_placeholders:
            if not page.is_file():
                findings.append(
                    _finding(label, 1, "nav", f"mount placeholder {leaf!r} has no committed landing page")
                )
            continue
        if not page.is_file():
            if leaf.endswith("/index.md"):
                findings.append(
                    _finding(
                        label,
                        1,
                        "nav",
                        f"unresolved mount placeholder {leaf!r}; add a mounts.yml entry or fix the path",
                    )
                )
            else:
                findings.append(_finding(label, 1, "nav", f"nav target {leaf!r} does not exist"))
    for leaf, count in seen.items():
        if count > 1:
            findings.append(_finding(label, 1, "nav", f"page {leaf!r} appears {count} times in nav"))
    nav_set = set(leaves)
    for rel in pages:
        if rel in redirects:
            continue
        if rel not in nav_set:
            mount = _under_mount(rel, ctx.mounts)
            if mount is not None and not _is_mount_landing(rel, ctx.mounts):
                findings.append(
                    _finding(
                        _docs_path(rel),
                        1,
                        "nav",
                        f"copied page is missing from expanded nav; add it to {mount.repo}'s nav.yml",
                    )
                )
                continue
            findings.append(
                _finding(_docs_path(rel), 1, "nav", "page is missing from nav (Zensical does not report this)")
            )
    for mount in ctx.mounts:
        placeholder = f"{mount.target.as_posix()}/index.md"
        if placeholder not in nav_set:
            findings.append(
                _finding(
                    label,
                    1,
                    "nav",
                    f"no nav placeholder for mount target {mount.target.as_posix()!r}",
                )
            )
    for title, section_pages in _nav_sections(nav):
        non_hubs = [page for page in section_pages if page != "index.md" and not page.endswith("/index.md")]
        prefixes = {page.split("/", 1)[0] for page in section_pages if "/" in page}
        typed = prefixes & set(TYPED_DIRS)
        if not typed:
            continue
        if not non_hubs:
            findings.append(
                _finding(
                    label,
                    1,
                    "tab",
                    f"typed tab {title!r} has no non-hub page; Home is special and "
                    "contracts count under Develop",
                )
            )
    return findings


def collect_privacy_hits(rel: str, text: str, patterns: tuple[PrivacyPattern, ...]) -> dict[tuple[str, str], int]:
    counts: dict[tuple[str, str], int] = {}
    for pattern in patterns:
        for match in pattern.regex.finditer(text):
            hit = match.group(0).replace("\t", " ")
            if any(allow.search(hit) for allow in pattern.allow):
                continue
            key = (pattern.ident, hit)
            counts[key] = counts.get(key, 0) + 1
    return counts


def check_privacy(rel: str, text: str, ctx: LintContext, *, contract: bool) -> list[Finding]:
    findings: list[Finding] = []
    label = _docs_path(rel)
    hits = collect_privacy_hits(rel, text, ctx.privacy_patterns)
    first_lines: dict[tuple[str, str], int] = {}
    for pattern in ctx.privacy_patterns:
        for match in pattern.regex.finditer(text):
            matched = match.group(0).replace("\t", " ")
            if any(allow.search(matched) for allow in pattern.allow):
                continue
            first_lines.setdefault((pattern.ident, matched), line_at(text, match.start()))
    for (rule, matched), count in sorted(hits.items()):
        line = first_lines[(rule, matched)]
        if not contract:
            placeholder = next((p.placeholder for p in ctx.privacy_patterns if p.ident == rule), None)
            hint = f"; use placeholder {placeholder}" if placeholder else ""
            findings.append(
                _finding(
                    label,
                    line,
                    "privacy",
                    f"{rule} hit {matched!r} occurs {count} time(s) on a reader page or mount{hint}",
                )
            )
            continue
        allowed = ctx.baseline.get((rel, rule, matched), 0)
        if allowed == 0:
            findings.append(
                _finding(
                    label,
                    line,
                    "privacy",
                    f"new {rule} hit {matched!r} is not in privacy_baseline.txt",
                )
            )
        elif count > allowed:
            findings.append(
                _finding(
                    label,
                    line,
                    "privacy",
                    f"duplicate {rule} hit {matched!r}: {count} occurrences exceed baseline {allowed}",
                )
            )
    return findings


def check_test_pins(ctx: LintContext) -> list[Finding]:
    findings: list[Finding] = []
    roots = [ctx.root / "src", ctx.root / "tests"]
    files: list[tuple[Path, str | None]] = []
    for base in roots:
        if base.is_dir():
            files.extend((path, None) for path in base.rglob("*.rs") if path.is_file())
    for mount in ctx.mounts:
        repo_dir = ctx.source_dirs.get(mount.repo)
        if repo_dir is None:
            if ctx.assembled:
                raise DocsError(
                    f"assembled source context is missing the resolved repo {mount.repo!r}"
                )
            continue
        files.extend(
            (path, f"{mount.repo}/{path.relative_to(repo_dir).as_posix()}")
            for path in _js_test_files(repo_dir)
        )
    for path, mounted_rel in sorted(files, key=lambda item: item[0].as_posix()):
        if mounted_rel is not None:
            rel = mounted_rel
        else:
            try:
                rel = path.relative_to(ctx.root).as_posix()
            except ValueError:
                rel = path.as_posix()
        if rel in TEST_PIN_EXEMPTIONS:
            continue
        text = _read(path)
        if text is None:
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            match = TEST_PIN_RE.search(line)
            if match:
                findings.append(
                    _finding(
                        rel,
                        lineno,
                        "test-pin",
                        "tests must not read docs/{start,guides,concepts}/; "
                        "only tests/cli/docs_tutorial.rs and this lint's tests may",
                    )
                )
    return findings


def _js_test_files(repo_dir: Path) -> list[Path]:
    files: list[Path] = []
    for path in repo_dir.rglob("*"):
        if not path.is_file():
            continue
        if any(part in SKIP_DIR_NAMES for part in path.parts):
            continue
        rel = path.relative_to(repo_dir).as_posix()
        name = path.name
        if path.suffix not in {".js", ".mjs", ".cjs", ".ts", ".tsx"}:
            continue
        if name.endswith(JS_TEST_SUFFIXES) or any(
            part in {"test", "tests", "__tests__"} for part in path.relative_to(repo_dir).parts
        ):
            files.append(path)
    return files


def check_media(ctx: LintContext) -> list[Finding]:
    findings: list[Finding] = []
    published: set[str] = set()
    for path in ctx.docs.rglob("*"):
        if not path.is_file() or path.suffix.lower() not in IMAGE_SUFFIXES:
            continue
        rel = path.relative_to(ctx.docs).as_posix()
        if _is_support(rel) or _is_excluded(rel, ctx.exclude_globs):
            continue
        published.add(rel)
    for rel in sorted(published):
        if rel not in ctx.media:
            findings.append(
                _finding(
                    _docs_path(rel),
                    1,
                    "media",
                    "published image is missing from the media manifest (source and ISO date required)",
                )
            )
    for asset in sorted(ctx.media):
        file_path = ctx.docs / Path(*PurePosixPath(asset).parts)
        if not file_path.is_file():
            findings.append(
                _finding(
                    "docs/_meta/media.yml",
                    1,
                    "media",
                    f"manifest path {asset!r} does not exist as a published asset",
                )
            )
    return findings


def _stale_ceilings(ctx: LintContext, pages: set[str]) -> dict[str, str]:
    stale: dict[str, str] = {}
    stems = _contract_stems(ctx.registry)
    for rel in sorted(ctx.ceilings):
        path = ctx.docs / rel
        if rel not in pages or not path.is_file():
            stale[rel] = f"stale ceiling for missing or excluded page {rel}"
            continue
        text = _read(path)
        if text is None:
            continue
        if _is_contract(rel, stems) or is_whole_file_generated(text):
            stale[rel] = f"stale ceiling for budget-exempt page {rel}"
    return stale


def check_stale_ceilings(ctx: LintContext, pages: set[str]) -> list[Finding]:
    return [
        _finding("docs/_meta/budgets.yml", 1, "budget", explanation)
        for _rel, explanation in sorted(_stale_ceilings(ctx, pages).items())
    ]


def check_tree(ctx: LintContext) -> list[Finding]:
    findings: list[Finding] = []
    findings.extend(check_registry(ctx))
    findings.extend(check_mount_gitignores(ctx))
    findings.extend(check_portability_config(ctx))
    findings.extend(
        _finding(path, line, rule, explanation)
        for path, line, rule, explanation in check_environment_coverage(ctx.root, ctx.docs)
    )
    pages = publishable_markdown(ctx.docs, ctx.exclude_globs)
    findings.extend(check_nav(ctx, pages))
    findings.extend(check_stale_ceilings(ctx, set(pages)))
    stems = _contract_stems(ctx.registry)
    tier_totals: dict[str, int] = {}
    for rel in pages:
        path = ctx.docs / rel
        text = _read(path)
        if text is None:
            continue
        contract = _is_contract(rel, stems)
        generated = is_whole_file_generated(text)
        typing = check_typing(rel, ctx)
        if typing is not None:
            findings.append(typing)
            continue
        findings.extend(check_front_matter(rel, text, ctx=ctx, contract=contract))
        findings.extend(
            check_budget(
                rel,
                count_words(text),
                tier=_tier_of(rel),
                contract=contract,
                generated=generated,
                ceilings=ctx.ceilings,
                tier_totals=tier_totals,
            )
        )
        findings.extend(check_hygiene(rel, text, tier=_tier_of(rel)))
        findings.extend(check_portability_markup(rel, text))
        findings.extend(
            check_privacy(
                rel,
                text,
                ctx,
                contract=contract and _under_mount(rel, ctx.mounts) is None,
            )
        )
    for tier, (_page_cap, tier_cap, _severity) in PAGE_BUDGETS.items():
        if tier_cap is not None and tier_totals.get(tier, 0) > tier_cap:
            findings.append(
                _finding(
                    f"docs/{tier}",
                    1,
                    "budget",
                    f"{tier}/ tier total is {tier_totals[tier]} words (cap {tier_cap})",
                )
            )
    findings.extend(check_test_pins(ctx))
    findings.extend(check_media(ctx))
    return findings


def ratchet_ceilings(ctx: LintContext) -> list[str]:
    ceilings = dict(ctx.ceilings)
    changed: list[str] = []
    pages = set(publishable_markdown(ctx.docs, ctx.exclude_globs))
    stale = _stale_ceilings(ctx, pages)
    for rel in sorted(list(ceilings)):
        if rel in stale:
            del ceilings[rel]
            changed.append(f"removed stale ceiling: {rel}")
            continue
        path = ctx.docs / rel
        text = _read(path)
        if text is None:
            continue
        shrunk_to = max(1, math.ceil(count_words(text) * 1.01))
        if shrunk_to < ceilings[rel]:
            ceilings[rel] = shrunk_to
            changed.append(f"{rel}: {ceilings[rel]}")
    if changed:
        payload: dict[str, Any] = {"ceilings": dict(sorted(ceilings.items())) if ceilings else {}}
        ctx.budgets_path.parent.mkdir(parents=True, exist_ok=True)
        header = (
            "# Optional per-page word ceilings. bob-docs lint fails a page that grows\n"
            "# past its ceiling. bob-docs lint --fix lowers a ceiling when a page shrinks\n"
            "# and never raises one. Missing, generated, and contract pages are removed.\n"
        )
        ctx.budgets_path.write_text(header + dump_yaml(payload), encoding="utf-8")
        ctx.ceilings = ceilings
    return changed


def ratchet_privacy_baseline(ctx: LintContext) -> list[str]:
    stems = _contract_stems(ctx.registry)
    current: dict[tuple[str, str, str], int] = {}
    for rel in publishable_markdown(ctx.docs, ctx.exclude_globs):
        if not _is_contract(rel, stems) or _under_mount(rel, ctx.mounts) is not None:
            continue
        text = _read(ctx.docs / rel)
        if text is None:
            continue
        for (rule, matched), count in collect_privacy_hits(rel, text, ctx.privacy_patterns).items():
            current[(rel, rule, matched)] = count
    changed: list[str] = []
    updated = dict(ctx.baseline)
    for key in sorted(ctx.baseline):
        rel, rule, matched = key
        new_count = current.get(key, 0)
        if new_count == 0:
            del updated[key]
            changed.append(f"removed obsolete {rel} {rule} {matched!r}")
        elif new_count < ctx.baseline[key]:
            updated[key] = new_count
            changed.append(f"lowered {rel} {rule} {matched!r}: {new_count}")
    if changed:
        ctx.baseline_path.write_text(dump_privacy_baseline(updated), encoding="utf-8")
        ctx.baseline = updated
    return changed


def run_lint(root: Path, *, fix: bool = False, docs_context: DocsContext | None = None) -> int:
    ctx = build_lint_context(root, docs_context=docs_context)
    if fix:
        for line in ratchet_ceilings(ctx):
            print(f"lowered ceiling: {line}")
        for line in ratchet_privacy_baseline(ctx):
            print(f"privacy baseline: {line}")
        return 0
    findings = check_tree(ctx)
    for finding in findings:
        prefix = "" if finding.severity == "error" else "warning: "
        print(f"{prefix}{finding.format()}")
    errors = sum(1 for finding in findings if finding.severity == "error")
    if errors:
        print(f"bob-docs lint: {errors} error(s)")
        return 1
    if findings:
        print(f"bob-docs lint: {len(findings)} warning(s)")
    else:
        print("bob-docs lint: ok")
    return 0

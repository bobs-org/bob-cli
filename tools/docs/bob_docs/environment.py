"""Check that production Rust environment names are covered by the reference."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .errors import DocsError
from .yaml_tools import load_yaml

ENV_NAME = re.compile(r"(?<![A-Za-z0-9_])BOB_[A-Z0-9]+(?:_[A-Z0-9]+)*(?![A-Za-z0-9_])")
DOC_NAME = re.compile(r"(?<![A-Za-z0-9_])BOB_[A-Z0-9]+(?:_[A-Z0-9]+)*(?![A-Za-z0-9_])")
TEST_FILES = frozenset({"test.rs", "tests.rs"})


@dataclass(frozen=True)
class RustToken:
    kind: str
    value: str
    line: int


@dataclass(frozen=True)
class EnvironmentName:
    name: str
    path: str
    line: int


def _quoted_end(source: str, start: int, quote: str) -> int:
    """Return the end of an escaped Rust string or character literal."""

    index = start + 1
    while index < len(source):
        char = source[index]
        if char == "\\":
            index += 2
            continue
        if char == quote:
            return index + 1
        index += 1
    return len(source)


def _raw_string(source: str, start: int) -> tuple[int, int] | None:
    """Return (content start, token end) for r#"..."# and br#"..."# forms."""

    prefix_start = start
    if source.startswith("br", start):
        prefix_start += 1
    if prefix_start >= len(source) or source[prefix_start] != "r":
        return None
    cursor = prefix_start + 1
    while cursor < len(source) and source[cursor] == "#":
        cursor += 1
    if cursor >= len(source) or source[cursor] != '"':
        return None
    hashes = cursor - prefix_start - 1
    content_start = cursor + 1
    terminator = '"' + ("#" * hashes)
    close = source.find(terminator, content_start)
    if close < 0:
        return content_start, len(source)
    return content_start, close + len(terminator)


def rust_tokens(source: str) -> list[RustToken]:
    """Tokenize enough Rust syntax to isolate literals and cfg(test) items.

    Comments are discarded. String contents are retained as one token so braces,
    quotes, and comment markers inside literals cannot affect item boundaries.
    """

    tokens: list[RustToken] = []
    index = 0
    line = 1
    length = len(source)
    while index < length:
        char = source[index]
        if char.isspace():
            if char == "\n":
                line += 1
            index += 1
            continue
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            if end < 0:
                break
            index = end
            continue
        if source.startswith("/*", index):
            depth = 1
            cursor = index + 2
            while cursor < length and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            line += source[index:cursor].count("\n")
            index = cursor
            continue

        raw = _raw_string(source, index)
        if raw is not None:
            content_start, end = raw
            content_end = end
            terminator_len = 1
            cursor = index + (1 if source.startswith("br", index) else 0) + 1
            while cursor < length and source[cursor] == "#":
                cursor += 1
                terminator_len += 1
            content_end = max(content_start, end - terminator_len)
            value = source[content_start:content_end]
            tokens.append(RustToken("string", value, line + source[index:content_start].count("\n")))
            line += source[index:end].count("\n")
            index = end
            continue

        if char == '"' or (char == "b" and index + 1 < length and source[index + 1] == '"'):
            quote_at = index if char == '"' else index + 1
            end = _quoted_end(source, quote_at, '"')
            value = source[quote_at + 1 : max(quote_at + 1, end - 1)]
            tokens.append(RustToken("string", value, line + source[index:quote_at + 1].count("\n")))
            line += source[index:end].count("\n")
            index = end
            continue

        if char == "'":
            # Distinguish a lifetime such as 'static from a character literal.
            # Unescaped chars close immediately; escaped forms may use Rust's
            # longer Unicode escape syntax.
            char_start = index + 1
            if char_start < length and source[char_start] == "\\":
                end = _quoted_end(source, index, "'")
                is_char = end > char_start + 1 and end <= length and source[end - 1] == "'"
            else:
                is_char = index + 2 < length and source[index + 2] == "'"
                end = index + 3 if is_char else index + 1
            if is_char:
                tokens.append(RustToken("char", source[index:end], line))
                line += source[index:end].count("\n")
                index = end
                continue

        if char.isalpha() or char == "_":
            end = index + 1
            while end < length and (source[end].isalnum() or source[end] == "_"):
                end += 1
            tokens.append(RustToken("ident", source[index:end], line))
            index = end
            continue

        tokens.append(RustToken("punct", char, line))
        index += 1
    return tokens


def _matching(tokens: list[RustToken], start: int, opening: str, closing: str) -> int | None:
    if start >= len(tokens) or tokens[start].value != opening:
        return None
    depth = 1
    for index in range(start + 1, len(tokens)):
        value = tokens[index].value
        if value == opening:
            depth += 1
        elif value == closing:
            depth -= 1
            if depth == 0:
                return index
    return None


def _cfg_test_attribute(tokens: list[RustToken], index: int) -> int | None:
    """Return the closing bracket for an attribute that enables test-only code."""

    if index + 1 >= len(tokens) or tokens[index].value != "#":
        return None
    close = _matching(tokens, index + 1, "[", "]")
    if close is None:
        return None
    body = tokens[index + 2 : close]
    if len(body) >= 3 and body[0].kind == "ident" and body[0].value == "cfg" and body[1].value == "(":
        end = _matching(body, 1, "(", ")")
        if end is not None and _cfg_requires_test(body[2:end]):
            return close
    return None


def _cfg_requires_test(expression: list[RustToken]) -> bool:
    """Whether a cfg expression is guaranteed false in a non-test build."""

    if len(expression) == 1:
        return expression[0].kind == "ident" and expression[0].value == "test"
    if len(expression) < 3 or expression[0].kind != "ident" or expression[1].value != "(":
        return False
    close = _matching(expression, 1, "(", ")")
    if close != len(expression) - 1:
        return False
    operator = expression[0].value
    if operator == "not":
        return False
    arguments: list[list[RustToken]] = []
    start = 2
    depth = 0
    for index in range(2, close):
        value = expression[index].value
        if value == "(":
            depth += 1
        elif value == ")":
            depth -= 1
        elif value == "," and depth == 0:
            arguments.append(expression[start:index])
            start = index + 1
    if start < close:
        arguments.append(expression[start:close])
    if operator == "all":
        return any(_cfg_requires_test(argument) for argument in arguments)
    if operator == "any":
        return bool(arguments) and all(_cfg_requires_test(argument) for argument in arguments)
    return False


def _skip_item(tokens: list[RustToken], index: int) -> int:
    """Skip attributes and one item after a cfg(test) attribute."""

    length = len(tokens)
    cursor = index
    while cursor < length:
        if cursor + 1 < length and tokens[cursor].value == "#":
            close = _matching(tokens, cursor + 1, "[", "]")
            if close is not None:
                cursor = close + 1
                continue
        break

    # Rust item bodies are the first top-level brace after the item header.
    # Parentheses and brackets are balanced first so nested syntax does not
    # make a closure or attribute look like the item's body.
    parens = brackets = angles = 0
    brace_depth = sum(
        1 if token.value == "{" else -1 if token.value == "}" else 0
        for token in tokens[:index]
    )
    cursor_body: int | None = None
    cursor = cursor
    while cursor < length:
        value = tokens[cursor].value
        if value == "(" and parens >= 0:
            parens += 1
        elif value == ")" and parens:
            parens -= 1
        elif value == "[" and parens == 0:
            brackets += 1
        elif value == "]" and brackets:
            brackets -= 1
        elif value == "<":
            angles += 1
        elif value == ">" and angles:
            angles -= 1
        elif parens == 0 and brackets == 0 and angles == 0:
            if value == ";":
                return cursor + 1
            if value == "," and brace_depth > 0:
                return cursor + 1
            if value == "{":
                cursor_body = _matching(tokens, cursor, "{", "}")
                if cursor_body is None:
                    return length
                # Const/static block expressions and tuple structs end with a
                # semicolon; consume it when immediately present.
                return cursor_body + 2 if cursor_body + 1 < length and tokens[cursor_body + 1].value == ";" else cursor_body + 1
        cursor += 1
    return length


def _production_strings(source: str) -> list[RustToken]:
    tokens = rust_tokens(source)
    strings: list[RustToken] = []
    index = 0
    while index < len(tokens):
        close = _cfg_test_attribute(tokens, index)
        if close is not None:
            index = _skip_item(tokens, close + 1)
            continue
        token = tokens[index]
        if token.kind == "string":
            strings.append(token)
        index += 1
    return strings


def scan_environment_names(repo_root: Path) -> dict[str, list[EnvironmentName]]:
    """Inventory complete BOB_* names in production Rust string literals."""

    root = repo_root.resolve()
    found: dict[str, list[EnvironmentName]] = {}
    source_root = root / "src"
    if not source_root.is_dir():
        return found
    for path in sorted(source_root.rglob("*.rs")):
        rel_path = path.relative_to(source_root)
        parts = rel_path.parts
        if (
            "tests" in parts
            or path.name in TEST_FILES
            or path.name.startswith("test_")
            or path.name.endswith(("_test.rs", "_tests.rs"))
        ):
            continue
        try:
            source = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError):
            continue
        for token in _production_strings(source):
            for match in ENV_NAME.finditer(token.value):
                line = token.line + token.value[: match.start()].count("\n")
                item = EnvironmentName(match.group(), path.relative_to(root).as_posix(), line)
                found.setdefault(item.name, []).append(item)
    return found


def _documented_names(path: Path) -> set[str]:
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return set()
    text = re.sub(r"(?s)<!--.*?-->", lambda match: "\n" * match.group().count("\n"), text)
    return {match.group() for match in DOC_NAME.finditer(text)}


def _load_ignores(path: Path) -> tuple[dict[str, str], str | None]:
    if not path.exists():
        return {}, None
    try:
        value = load_yaml(path)
    except DocsError as error:
        return {}, str(error)
    if not isinstance(value, dict):
        return {}, f"{path}: expected a mapping of exact environment names to reasons"
    ignores: dict[str, str] = {}
    for name, reason in value.items():
        if not isinstance(name, str) or not ENV_NAME.fullmatch(name):
            return {}, f"{path}: invalid environment name {name!r}"
        if not isinstance(reason, str) or len(reason.strip()) < 12:
            return {}, f"{path}: {name} needs a specific, nonempty reason"
        ignores[name] = reason.strip()
    return ignores, None


def check_environment_coverage(repo_root: Path, docs_root: Path) -> list[tuple[str, int, str, str]]:
    """Return (path, line, rule, message) findings for the environment inventory."""

    inventory = scan_environment_names(repo_root)
    if not inventory:
        return []
    config_rel = "reference/configuration.md"
    config_path = docs_root / config_rel
    ignore_path = docs_root / "_meta/environment_ignore.yml"
    findings: list[tuple[str, int, str, str]] = []
    ignores, error = _load_ignores(ignore_path)
    if error:
        findings.append(("docs/_meta/environment_ignore.yml", 1, "environment", error))
        ignores = {}
    for name in sorted(set(ignores) - set(inventory)):
        findings.append(
            ("docs/_meta/environment_ignore.yml", 1, "environment", f"stale exception for unknown name {name}")
        )
    if not config_path.is_file():
        findings.append(
            (f"docs/{config_rel}", 1, "environment", "missing configuration reference for production BOB_* names")
        )
        return findings
    documented = _documented_names(config_path)
    for name, locations in sorted(inventory.items()):
        if name in documented:
            if name in ignores:
                findings.append(
                    ("docs/_meta/environment_ignore.yml", 1, "environment", f"documented runtime name {name} must not be exempted")
                )
            continue
        if name in ignores:
            continue
        for location in locations:
            findings.append(
                (location.path, location.line, "environment", f"undocumented production environment name {name}; document it in docs/{config_rel} or add a reasoned exact exception")
            )
    return findings

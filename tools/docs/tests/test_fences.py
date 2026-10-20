"""Focused tests for the static bob-task fence."""

from __future__ import annotations

import markdown

from bob_docs import fences


def _convert(source: str) -> str:
    md = markdown.Markdown(
        extensions=["pymdownx.superfences"],
        extension_configs={
            "pymdownx.superfences": {
                "custom_fences": [
                    {
                        "name": "bob-task",
                        "class": "bob-task",
                        "format": fences.fence_format,
                        "validator": fences.fence_validator,
                    }
                ]
            }
        },
    )
    return md.convert(source)


def test_all_statuses_have_accessible_names() -> None:
    cases = {
        " ": "Ready",
        "*": "Next",
        "/": "In Progress",
        "?": "Blocked",
        "x": "Done",
        "-": "Cancelled",
    }
    for status, name in cases.items():
        html = fences.render_bob_tasks(f"- [{status}] #task Do it")
        assert f'aria-label="{name}"' in html, (status, html)
    # Uppercase X is also Done.
    assert 'aria-label="Done"' in fences.render_bob_tasks("- [X] #task Do it")


def test_task_token_boundaries() -> None:
    html = fences.render_bob_tasks("- [ ] #task Real #tasks #Task #prj")
    assert html.count("bob-tag-task") == 1
    assert "#tasks" in html and "#Task" in html
    assert "bob-tag-prj" in html
    # Inline code contents are not transformed.
    html = fences.render_bob_tasks("- [ ] `#task` literal")
    assert "bob-tag-task" not in html
    assert "<code>#task</code>" in html
    # Escaped tokens stay literal.
    html = fences.render_bob_tasks(r"- [ ] \#task literal")
    assert "bob-tag-task" not in html


def test_dates_leap_and_malformed() -> None:
    # Valid leap day renders with absolute year label and ISO data.
    html = fences.render_bob_tasks("- [ ] #task Leap [created:: 2024-02-29]")
    assert "Feb 29, 2024" in html
    assert 'data-date="2024-02-29"' in html
    # Non-leap Feb 29 stays literal.
    html = fences.render_bob_tasks("- [ ] #task Nope [created:: 2026-02-29]")
    assert "bob-date" not in html
    # Malformed and duplicate fields stay literal.
    html = fences.render_bob_tasks("- [ ] #task Bad [due:: 2026-01-01]")
    assert "[due:: 2026-01-01]" in html
    html = fences.render_bob_tasks("- [ ] #task Dup [scheduled:: 2026-01-01] [scheduled:: 2026-01-02]")
    assert "bob-date" not in html
    # Unsupported start stays literal.
    html = fences.render_bob_tasks("- [ ] #task Keep [start:: 09:00]")
    assert "[start:: 09:00]" in html


def test_children_and_ids_preserved() -> None:
    src = "- [ ] #task Parent [id:: abc]\n  - child Depends-On line\n\n  [dependsOn:: [[Other]]]"
    html = fences.render_bob_tasks(src)
    assert "bob-id" in html
    assert "child Depends-On line" in html


def test_today_only_on_open_rows() -> None:
    html = fences.render_bob_tasks("- [*] #task Open ^a\n- [x] #task Shut ^b", today=True)
    assert html.count("bob-today") == 1
    assert 'aria-label="Today"' in html
    html = fences.render_bob_tasks("- [x] #task Shut ^b", today=True)
    assert "bob-today" not in html


def test_html_injection_escaped_and_time_independent() -> None:
    src = '- [ ] #task <script>alert("x")</script> [created:: 2026-01-15]'
    first = fences.render_bob_tasks(src)
    second = fences.render_bob_tasks(src)
    assert first == second
    assert "<script>" not in first
    assert "&lt;script&gt;" in first
    assert "Jan 15, 2026" in first


def test_real_fence_invocation_reaches_formatter() -> None:
    src = "```bob-task\n- [ ] #task Hello\n```"
    html = _convert(src)
    assert "bob-task-line" in html
    assert "bob-tag-task" in html
    src = "```bob-task today\n- [*] #task Hello\n```"
    html = _convert(src)
    assert "bob-today" in html
    # Ordinary text fences still show the original source.
    md = markdown.Markdown(extensions=["pymdownx.superfences"])
    html = md.convert("```text\n- [ ] #task Hello\n```")
    assert "#task Hello" in html
    assert "bob-task-line" not in html

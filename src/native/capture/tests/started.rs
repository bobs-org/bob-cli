//! Started-Pomodoro placement tests.
use super::*;

#[test]
fn started_pomodoro_already_in_slot_keeps_blank_line() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] (**0715-0805** [t:: 50m]) — DONE\n",
        "  - child\n",
        "\n",
        "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
        "  - [[sase_goals#^research]]\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 4)
            .expect("already in slot");
    assert_eq!(updated, contents);
    assert_eq!(index, 4);
}

#[test]
fn started_pomodoro_moves_before_first_open_without_completed() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] () — BUGS\n",
        "  - [[sase#^bugs]]\n",
        "- [ ] (**0905-0930** [t:: 25m]) — FOCUS\n",
        "  - [[sase#^focus]]\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 3)
            .expect("move before first open");
    assert_eq!(index, 1);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — FOCUS\n",
            "  - [[sase#^focus]]\n",
            "- [ ] () — BUGS\n",
            "  - [[sase#^bugs]]\n",
        )
    );
}

#[test]
fn started_pomodoro_moves_after_completed_with_grandchildren() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] (**0715-0805** [t:: 50m]) — DONE\n",
        "  - child\n",
        "    - grandchild\n",
        "- [ ] () — PLANNED\n",
        "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
        "  - [[sase_goals#^research]]\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 5)
            .expect("move after completed block");
    assert_eq!(index, 4);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0715-0805** [t:: 50m]) — DONE\n",
            "  - child\n",
            "    - grandchild\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
            "  - [[sase_goals#^research]]\n",
            "- [ ] () — PLANNED\n",
        )
    );
}

#[test]
fn started_pomodoro_interleaved_moves_after_last_completed() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done A\n",
        "- [ ] (**0905-0930** [t:: 25m]) — X\n",
        "- [x] Done B\n",
        "  - child\n",
        "- [ ] () — Y\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 2)
            .expect("interleaved move");
    assert_eq!(index, 4);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] Done A\n",
            "- [x] Done B\n",
            "  - child\n",
            "- [ ] (**0905-0930** [t:: 25m]) — X\n",
            "- [ ] () — Y\n",
        )
    );
}

#[test]
fn started_pomodoro_ignores_cancelled_fenced_and_nested_anchors() {
    let contents = concat!(
        "## Pomodoros\n",
        "```md\n",
        "- [x] Fenced done\n",
        "```\n",
        "- [-] Cancelled\n",
        "  - [x] Nested done\n",
        "- [ ] () — FIRST\n",
        "- [ ] (**0905-0930** [t:: 25m]) — SECOND\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 7)
            .expect("lookalikes are not anchors");
    assert_eq!(index, 6);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "```md\n",
            "- [x] Fenced done\n",
            "```\n",
            "- [-] Cancelled\n",
            "  - [x] Nested done\n",
            "- [ ] (**0905-0930** [t:: 25m]) — SECOND\n",
            "- [ ] () — FIRST\n",
        )
    );
}

#[test]
fn started_pomodoro_moves_interior_blank_line_and_keeps_trailing() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done\n",
        "- [ ] () — PLANNED\n",
        "\n",
        "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
        "  - link\n",
        "\n",
        "  - notes\n",
        "\n",
        "- [ ] () — AFTER\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 4)
            .expect("interior blank moves");
    assert_eq!(index, 2);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] Done\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
            "  - link\n",
            "\n",
            "  - notes\n",
            "- [ ] () — PLANNED\n",
            "\n",
            "\n",
            "- [ ] () — AFTER\n",
        )
    );
}

#[test]
fn started_pomodoro_move_preserves_crlf() {
    let contents = concat!(
        "## Pomodoros\r\n",
        "- [x] Done\r\n",
        "- [ ] () — PLANNED\r\n",
        "- [ ] (**0905-0930** [t:: 25m]) — GOALS\r\n",
        "  - link\r\n",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 3)
            .expect("crlf move");
    assert_eq!(index, 2);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\r\n",
            "- [x] Done\r\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\r\n",
            "  - link\r\n",
            "- [ ] () — PLANNED\r\n",
        )
    );
    assert!(!updated.contains('\n') || updated.contains("\r\n"));
}

#[test]
fn started_pomodoro_eof_without_newline_moves_up() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [x] Done\n",
        "  - child\n",
        "- [ ] () — PLANNED\n",
        "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
        "  - link",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 4)
            .expect("eof block moves up");
    assert_eq!(index, 3);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] Done\n",
            "  - child\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
            "  - link\n",
            "- [ ] () — PLANNED",
        )
    );
    assert!(!updated.ends_with('\n'));
}

#[test]
fn started_pomodoro_moves_down_to_eof_without_newline() {
    let contents = concat!(
        "## Pomodoros\n",
        "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
        "  - link\n",
        "- [x] Done",
    );
    let (updated, index) =
        super::move_started_pomodoro_to_current_slot(contents, 1)
            .expect("move down to eof");
    assert_eq!(index, 2);
    assert_eq!(
        updated,
        concat!(
            "## Pomodoros\n",
            "- [x] Done\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
            "  - link",
        )
    );
    assert!(!updated.ends_with('\n'));
}

#[test]
fn non_tasks_section_headings_match_bullet_heading_scan() {
    let contents = concat!(
        "---\n",
        "## Frontmatter\n",
        "---\n",
        "# Title\n",
        "```md\n",
        "## Fenced\n",
        "```\n",
        "## Tasks\n",
        "### Ideas ###\n",
        "###### Log\n",
    );
    assert_eq!(
        non_tasks_section_headings(contents),
        vec![
            SectionHeading {
                title: "Title".to_string(),
                level: 1,
            },
            SectionHeading {
                title: "Ideas".to_string(),
                level: 3,
            },
            SectionHeading {
                title: "Log".to_string(),
                level: 6,
            },
        ]
    );
}

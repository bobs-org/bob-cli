//! Pure project-note content renderer for the `+` sigil capture family.
//!
//! `bob capture '@cash^goog-exit+' '...'` creates a brand-new sub-project note
//! `<route>_<block_id>.md` whose `parent` frontmatter points back at
//! `<route>.md`, mirroring the Obsidian Bob Navigation Hotkeys command
//! "Create project note from task". This module renders that note's basename
//! and contents from an already-parsed capture item.
//!
//! The module is free of filesystem and clock access: the caller passes the
//! resolved body, the current datetime (`bob_env::current_datetime()`), the
//! resolved scheduled date, the resolved priority field, pre-rendered
//! schedule-log lines, the Pomodoro-link flag, and the item's authored
//! sub-bullets. `render_project_note` returns the file contents plus a small
//! summary (`basename`, rendered `^prj` line, section titles, task count) for
//! the execution layer's JSON contract.
//!
//! Section mapping follows `buildProjectSeedFromChildBullets` in
//! `plugins/bob-navigation-hotkeys/main.js`: a first-level authored bullet is
//! a section when it has at least one nested bullet and its body matches the
//! ALL-CAPS title shape, otherwise it is a task. Managed schedule/work log
//! markers are deliberately not special-cased here: a capture draft authors a
//! brand-new task rather than moving an existing one, so a log-shaped bullet
//! is an ordinary task or section bullet.

use std::collections::HashMap;

use chrono::{Local, LocalResult, NaiveDateTime, TimeZone};

use super::capture_language::{AuthoredDepth, AuthoredSubBullet};

/// Placeholder task kept under `## Tasks` when no authored task children were
/// rendered. Matches the template's `(REPLACE WITH TASK DESCRIPTION)` line
/// that `replaceProjectTasksPlaceholder` leaves in place in that case.
pub(crate) const PLACEHOLDER_TASK_BODY: &str = "(REPLACE WITH TASK DESCRIPTION)";

/// Everything `render_project_note` needs. Dates arrive resolved: `scheduled`
/// is `YYYY-MM-DD`, `priority` is the `(name, value)` field pair (for example
/// `("priority", "lowest")`), and `schedule_log_lines` are pre-rendered,
/// already-indented lines (see `capture_schedule_log::plan`) inserted verbatim
/// under the `^prj` task. `pomodoro_link` is true for the `:` marker form.
#[derive(Debug, Clone)]
pub(crate) struct ProjectNoteRenderInput<'a> {
    pub(crate) route: &'a str,
    pub(crate) block_id: &'a str,
    pub(crate) body: &'a str,
    pub(crate) now: NaiveDateTime,
    pub(crate) scheduled: Option<&'a str>,
    pub(crate) priority: Option<(&'a str, &'a str)>,
    pub(crate) schedule_log_lines: &'a [String],
    pub(crate) pomodoro_link: bool,
    pub(crate) sub_bullets: &'a [AuthoredSubBullet],
}

/// Rendered note plus the summary the execution layer reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedProjectNote {
    /// File basename, for example `cash_goog_exit.md`.
    pub(crate) basename: String,
    /// Full file contents.
    pub(crate) contents: String,
    /// The rendered `^prj` lifecycle task line.
    pub(crate) task_line: String,
    /// Rendered section titles in source order, excluding `## Tasks`.
    pub(crate) sections: Vec<String>,
    /// Number of top-level task lines under `## Tasks`, including the
    /// placeholder when it is kept. Nested child lines and notes merged from
    /// an authored `TASKS` section are not counted.
    pub(crate) task_count: usize,
}

/// `<route>_<block-id with every '-' replaced by '_'>.md`, matching
/// `getProjectBasenameFromTaskBlockId`. The route arrives lower-cased from the
/// token parser; the block ID keeps its authored case.
pub(crate) fn project_note_basename(route: &str, block_id: &str) -> String {
    format!("{}_{}.md", route, block_id.replace('-', "_"))
}

/// Format the frontmatter `created` stamp (`YYYY-MM-DDTHH:mm:ss±ZZZZ`) from a
/// naive local datetime. The local offset is resolved for that naive value via
/// `chrono::Local` rather than calling `Local::now()` again, so `BOB_NOW`
/// keeps controlling the field.
pub(crate) fn format_created_timestamp(now: NaiveDateTime) -> String {
    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%z";
    match Local.from_local_datetime(&now) {
        LocalResult::Single(datetime) => datetime.format(FORMAT).to_string(),
        LocalResult::Ambiguous(earliest, _) => earliest.format(FORMAT).to_string(),
        LocalResult::None => gap_offset_timestamp(now, FORMAT),
    }
}

/// Fallback for nonexistent wall times in the spring-forward gap: borrow the
/// offset in effect an hour later so the authored wall time is preserved.
fn gap_offset_timestamp(now: NaiveDateTime, format: &str) -> String {
    const FALLBACK_SUFFIX: &str = "+0000";
    let later = now
        .checked_add_signed(chrono::Duration::hours(1))
        .unwrap_or(now);
    let offset = match Local.from_local_datetime(&later) {
        LocalResult::Single(datetime) => Some(*datetime.offset()),
        LocalResult::Ambiguous(earliest, _) => Some(*earliest.offset()),
        LocalResult::None => None,
    };
    offset
        .and_then(|offset| offset.from_local_datetime(&now).single())
        .map(|resolved| resolved.format(format).to_string())
        .unwrap_or_else(|| format!("{}{FALLBACK_SUFFIX}", now.format("%Y-%m-%dT%H:%M:%S")))
}

/// Whether a trimmed authored body matches the ALL-CAPS section-title shape
/// (`^[A-Z0-9][A-Z0-9 \t&'(),./-]*$` with at least one `A-Z`), mirroring
/// `PROJECT_SECTION_TITLE_RE` plus the letter check.
pub(crate) fn is_project_section_title(body: &str) -> bool {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) if first.is_ascii_uppercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    for character in trimmed.chars() {
        let allowed = character.is_ascii_uppercase()
            || character.is_ascii_digit()
            || matches!(
                character,
                ' ' | '\t' | '&' | '\'' | '(' | ')' | ',' | '.' | '/' | '-'
            );
        if !allowed {
            return false;
        }
    }
    trimmed
        .chars()
        .any(|character| character.is_ascii_uppercase())
}

/// Lowercase the whole body, then uppercase the first character of every
/// alphanumeric run: `FUTURE WORK` becomes `Future Work`, `NON-GOALS` becomes
/// `Non-Goals`, and acronyms are not preserved, so `API DESIGN` becomes
/// `Api Design`. Mirrors `formatProjectSectionTitle`.
pub(crate) fn format_project_section_title(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = collapsed.to_lowercase();
    let mut rendered = String::with_capacity(lower.len());
    let mut run_start = true;
    for character in lower.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if run_start {
                rendered.push(character.to_ascii_uppercase());
                run_start = false;
            } else {
                rendered.push(character);
            }
        } else {
            rendered.push(character);
            run_start = true;
        }
    }
    rendered
}

/// Trimmed, whitespace-collapsed, casefolded comparison key. Two section
/// bullets with equal keys merge into one section; a rendered title equal to
/// an existing `##` header's key appends to that header instead of adding a
/// second one. Mirrors `normalizeProjectSectionTitle`.
pub(crate) fn normalize_project_section_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Split a leading `[X]` checkbox marker off a trimmed authored body,
/// returning the status character and the remaining body. Returns
/// `(None, body)` when no checkbox is present.
fn split_leading_checkbox(trimmed: &str) -> (Option<char>, &str) {
    let mut chars = trimmed.char_indices();
    if chars.next().map(|(_, value)| value) != Some('[') {
        return (None, trimmed);
    }
    let Some((_, status)) = chars.next() else {
        return (None, trimmed);
    };
    if status == ']' || status == '\n' {
        return (None, trimmed);
    }
    let Some((close_start, close)) = chars.next() else {
        return (None, trimmed);
    };
    if close != ']' {
        return (None, trimmed);
    }
    let rest = &trimmed[close_start + 1..];
    if rest.is_empty() {
        return (Some(status), "");
    }
    if !rest.starts_with(char::is_whitespace) {
        return (None, trimmed);
    }
    (Some(status), rest.trim_start())
}

/// Whether `body` already carries a `#task` token, using the same boundary
/// rule as `PROJECT_TASK_TAG_RE`: `(^|[\s([{])#task($|[\s)\]},.;:!?])`.
fn contains_task_tag(body: &str) -> bool {
    let bytes = body.as_bytes();
    let mut index = 0;
    while let Some(found) = body[index..].find("#task") {
        let start = index + found;
        let before_ok = start == 0
            || matches!(
                bytes[start - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'(' | b'[' | b'{'
            );
        let end = start + "#task".len();
        let after_ok = end == bytes.len()
            || matches!(
                bytes[end],
                b' ' | b'\t'
                    | b'\n'
                    | b'\r'
                    | b')'
                    | b']'
                    | b'}'
                    | b','
                    | b'.'
                    | b';'
                    | b':'
                    | b'!'
                    | b'?'
            );
        if before_ok && after_ok {
            return true;
        }
        index = start + 1;
    }
    false
}

/// Render one authored task line: preserve an authored checkbox status
/// (defaulting to open), add `#task` unless present, and append
/// `[created::DATE]` unless the body already carries one. Nested child lines
/// are rendered by the caller as plain indented bullets.
fn render_authored_task(status: char, body: &str, created_date: &str) -> String {
    let task_body = if body.is_empty() {
        "#task".to_string()
    } else if contains_task_tag(body) {
        body.to_string()
    } else {
        format!("#task {body}")
    };
    let mut line = format!("- [{status}] {task_body}");
    if !task_body.contains("[created::") {
        line.push_str(&format!(" [created::{created_date}]"));
    }
    line
}

struct FirstGroup<'a> {
    owner: &'a AuthoredSubBullet,
    nested: Vec<&'a AuthoredSubBullet>,
}

struct SectionEntry {
    title: String,
    normalized: String,
    note_lines: Vec<String>,
}

/// Render the project note for `input`. See the module docs for the contract.
pub(crate) fn render_project_note(input: &ProjectNoteRenderInput<'_>) -> RenderedProjectNote {
    let basename = project_note_basename(input.route, input.block_id);
    let created_date = input.now.format("%Y-%m-%d").to_string();
    let created_timestamp = format_created_timestamp(input.now);

    let status = if input.scheduled.is_some() {
        '?'
    } else if input.pomodoro_link {
        '*'
    } else {
        ' '
    };
    let mut task_line = format!("- [{status}] #task #prj {}", input.body.trim());
    if let Some((name, value)) = input.priority {
        task_line.push_str(&format!(" [{name}::{value}]"));
    }
    task_line.push_str(" #hide ^prj");

    let mut groups: Vec<FirstGroup<'_>> = Vec::new();
    for bullet in input.sub_bullets {
        if bullet.depth == AuthoredDepth::First || groups.is_empty() {
            groups.push(FirstGroup {
                owner: bullet,
                nested: Vec::new(),
            });
        } else if let Some(current) = groups.last_mut() {
            current.nested.push(bullet);
        }
    }

    let mut tasks_block: Vec<String> = Vec::new();
    let mut task_entries: usize = 0;
    let mut sections: Vec<SectionEntry> = Vec::new();
    let mut section_index: HashMap<String, usize> = HashMap::new();
    for group in &groups {
        let owner_body = group.owner.body.trim();
        let (checkbox, bare) = split_leading_checkbox(owner_body);
        if checkbox.is_none() && !group.nested.is_empty() && is_project_section_title(owner_body) {
            let title = format_project_section_title(owner_body);
            let normalized = normalize_project_section_title(&title);
            let note_lines: Vec<String> = group
                .nested
                .iter()
                .map(|nested| format!("- {}", nested.body.trim()))
                .collect();
            match section_index.get(&normalized) {
                Some(&index) => {
                    sections[index].note_lines.extend(note_lines);
                }
                None => {
                    section_index.insert(normalized.clone(), sections.len());
                    sections.push(SectionEntry {
                        title,
                        normalized,
                        note_lines,
                    });
                }
            }
        } else {
            let task_status = checkbox.unwrap_or(' ');
            tasks_block.push(render_authored_task(task_status, bare, &created_date));
            task_entries += 1;
            for nested in &group.nested {
                tasks_block.push(format!("\t- {}", nested.body.trim()));
            }
        }
    }

    // A rendered section matching the generated `## Tasks` header merges into
    // it instead of adding a duplicate header. The fresh note holds no other
    // headers, so this covers the general match-an-existing-header rule.
    let mut merged_tasks: Vec<String> = Vec::new();
    sections.retain(|section| {
        if section.normalized == "tasks" {
            merged_tasks.extend(section.note_lines.iter().cloned());
            false
        } else {
            true
        }
    });

    let mut contents = String::new();
    contents.push_str("---\n");
    contents.push_str(&format!("parent: \"[[{}]]\"\n", input.route));
    contents.push_str("template: \"[[new_project]]\"\n");
    contents.push_str("type: \"[[project]]\"\n");
    contents.push_str("status: wip\n");
    if let Some(scheduled) = input.scheduled {
        contents.push_str(&format!("scheduled: {scheduled}\n"));
    }
    contents.push_str(&format!("created: {created_timestamp}\n"));
    contents.push_str("---\n\n");
    contents.push_str(&task_line);
    contents.push('\n');
    for line in input.schedule_log_lines {
        contents.push_str(line);
        contents.push('\n');
    }
    contents.push_str("\n## Tasks\n\n");
    let task_count = if tasks_block.is_empty() && merged_tasks.is_empty() {
        contents.push_str(&format!(
            "- [ ] #task {PLACEHOLDER_TASK_BODY} [created::{created_date}]\n"
        ));
        1
    } else {
        for line in &tasks_block {
            contents.push_str(line);
            contents.push('\n');
        }
        for line in &merged_tasks {
            contents.push_str(line);
            contents.push('\n');
        }
        task_entries
    };

    let mut section_titles = Vec::with_capacity(sections.len());
    for section in &sections {
        section_titles.push(section.title.clone());
        contents.push_str(&format!("\n## {}\n\n", section.title));
        for line in &section.note_lines {
            contents.push_str(line);
            contents.push('\n');
        }
    }

    RenderedProjectNote {
        basename,
        contents,
        task_line,
        sections: section_titles,
        task_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn test_now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 20)
            .expect("valid test date")
            .and_hms_opt(14, 31, 7)
            .expect("valid test time")
    }

    fn bullet(body: &str, depth: AuthoredDepth) -> AuthoredSubBullet {
        AuthoredSubBullet {
            body: body.to_string(),
            depth,
        }
    }

    fn base_input<'a>(sub_bullets: &'a [AuthoredSubBullet]) -> ProjectNoteRenderInput<'a> {
        ProjectNoteRenderInput {
            route: "cash",
            block_id: "goog-exit",
            body: "Finish the Google exit packet!",
            now: test_now(),
            scheduled: None,
            priority: None,
            schedule_log_lines: &[],
            pomodoro_link: false,
            sub_bullets,
        }
    }

    fn pin_utc() {
        // Keep the rendered `created` offset deterministic. Every test in this
        // module pins the same value, so concurrent tests cannot disagree.
        unsafe {
            std::env::set_var("TZ", "UTC0");
        }
    }

    #[test]
    fn basename_replaces_dashes_and_keeps_case() {
        assert_eq!(
            project_note_basename("cash", "goog-exit"),
            "cash_goog_exit.md"
        );
        assert_eq!(project_note_basename("sase", "TUI-Fix"), "sase_TUI_Fix.md");
    }

    #[test]
    fn basic_note_matches_the_plan_example() {
        pin_utc();
        let rendered = render_project_note(&base_input(&[]));
        assert_eq!(rendered.basename, "cash_goog_exit.md");
        assert_eq!(
            rendered.task_line,
            "- [ ] #task #prj Finish the Google exit packet! #hide ^prj"
        );
        assert_eq!(
            rendered.contents,
            "---\n\
             parent: \"[[cash]]\"\n\
             template: \"[[new_project]]\"\n\
             type: \"[[project]]\"\n\
             status: wip\n\
             created: 2026-09-20T14:31:07+0000\n\
             ---\n\
             \n\
             - [ ] #task #prj Finish the Google exit packet! #hide ^prj\n\
             \n\
             ## Tasks\n\
             \n\
             - [ ] #task (REPLACE WITH TASK DESCRIPTION) [created::2026-09-20]\n"
        );
        assert_eq!(rendered.sections, Vec::<String>::new());
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn created_timestamp_derives_from_the_passed_datetime() {
        pin_utc();
        let first = format_created_timestamp(test_now());
        let later = format_created_timestamp(
            NaiveDate::from_ymd_opt(2026, 9, 21)
                .expect("valid test date")
                .and_hms_opt(9, 0, 0)
                .expect("valid test time"),
        );
        assert_eq!(first, "2026-09-20T14:31:07+0000");
        assert_eq!(later, "2026-09-21T09:00:00+0000");
    }

    #[test]
    fn scheduled_date_lands_in_frontmatter_with_a_blocked_checkbox() {
        pin_utc();
        let mut input = base_input(&[]);
        input.scheduled = Some("2026-10-04");
        let rendered = render_project_note(&input);
        assert!(
            rendered
                .contents
                .contains("status: wip\nscheduled: 2026-10-04\n")
        );
        assert!(rendered.task_line.starts_with("- [?] "));
    }

    #[test]
    fn pomodoro_link_uses_next_status_unless_scheduled() {
        let mut input = base_input(&[]);
        input.pomodoro_link = true;
        assert!(render_project_note(&input).task_line.starts_with("- [*] "));
        input.scheduled = Some("2026-10-04");
        assert!(render_project_note(&input).task_line.starts_with("- [?] "));
    }

    #[test]
    fn priority_writes_an_inline_field_before_hide() {
        let mut input = base_input(&[]);
        input.priority = Some(("priority", "lowest"));
        let rendered = render_project_note(&input);
        assert_eq!(
            rendered.task_line,
            "- [ ] #task #prj Finish the Google exit packet! \
             [priority::lowest] #hide ^prj"
        );
    }

    #[test]
    fn schedule_log_lines_land_directly_under_the_prj_task() {
        let log = vec![
            "\t- 🗓️ **SCHEDULE LOG**".to_string(),
            "\t\t- *2026-11-02* — 🎲 P0 → P4 · in **91** (91–365) days".to_string(),
        ];
        let mut input = base_input(&[]);
        input.priority = Some(("priority", "lowest"));
        input.scheduled = Some("2026-11-02");
        input.schedule_log_lines = &log;
        let rendered = render_project_note(&input);
        let prj_index = rendered
            .contents
            .find(&rendered.task_line)
            .expect("prj line is rendered");
        let after = &rendered.contents[prj_index + rendered.task_line.len()..];
        assert!(after.starts_with("\n\t- 🗓️ **SCHEDULE LOG**\n"));
    }

    #[test]
    fn authored_tasks_render_with_created_stamps_and_tab_children() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::First),
            bullet("Confirm the account number", AuthoredDepth::Nested),
            bullet("Book the flight", AuthoredDepth::First),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains(
            "- [ ] #task Call Morgan Stanley [created::2026-09-20]\n\
             \t- Confirm the account number\n\
             - [ ] #task Book the flight [created::2026-09-20]\n"
        ));
        assert_eq!(rendered.task_count, 2);
        assert!(!rendered.contents.contains(PLACEHOLDER_TASK_BODY));
    }

    #[test]
    fn all_caps_section_with_children_becomes_a_header() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::First),
            bullet("FUTURE WORK", AuthoredDepth::First),
            bullet("Consider the Roth option", AuthoredDepth::Nested),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(rendered.contents.contains("\n## Future Work\n\n"));
        assert!(rendered.contents.contains("- Consider the Roth option\n"));
        assert_eq!(rendered.sections, vec!["Future Work".to_string()]);
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn acronym_titles_do_not_preserve_capitals() {
        assert_eq!(format_project_section_title("API DESIGN"), "Api Design");
        assert_eq!(format_project_section_title("NON-GOALS"), "Non-Goals");
        assert_eq!(format_project_section_title("FUTURE WORK"), "Future Work");
    }

    #[test]
    fn equal_section_titles_merge_in_source_order() {
        let sub_bullets = vec![
            bullet("FUTURE WORK", AuthoredDepth::First),
            bullet("First note", AuthoredDepth::Nested),
            bullet("FUTURE  WORK", AuthoredDepth::First),
            bullet("Second note", AuthoredDepth::Nested),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.contents.matches("## Future Work").count(), 1);
        let section_start = rendered
            .contents
            .find("## Future Work")
            .expect("section is rendered");
        let section = &rendered.contents[section_start..];
        let first = section.find("- First note").expect("first note kept");
        let second = section.find("- Second note").expect("second note kept");
        assert!(first < second);
        assert_eq!(rendered.sections, vec!["Future Work".to_string()]);
    }

    #[test]
    fn bare_all_caps_bullet_without_children_stays_a_task() {
        let sub_bullets = vec![bullet("FUTURE WORK", AuthoredDepth::First)];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert!(!rendered.contents.contains("## Future Work"));
        assert!(
            rendered
                .contents
                .contains("- [ ] #task FUTURE WORK [created::2026-09-20]\n")
        );
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn tasks_section_merges_into_the_generated_header() {
        let sub_bullets = vec![
            bullet("Call Morgan Stanley", AuthoredDepth::First),
            bullet("TASKS", AuthoredDepth::First),
            bullet("Carried reference note", AuthoredDepth::Nested),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.contents.matches("## Tasks").count(), 1);
        assert!(rendered.contents.contains("- Carried reference note\n"));
        assert!(rendered.sections.is_empty());
        assert_eq!(rendered.task_count, 1);
    }

    #[test]
    fn section_title_shape_rejects_mixed_case_and_banners() {
        assert!(is_project_section_title("FUTURE WORK"));
        assert!(is_project_section_title("Q&A"));
        assert!(!is_project_section_title("Future Work"));
        assert!(!is_project_section_title("WHAT'S NEXT?"));
        assert!(!is_project_section_title("123"));
        assert!(!is_project_section_title(""));
    }

    #[test]
    fn managed_log_shaped_bullets_are_not_special_cased() {
        let sub_bullets = vec![
            bullet("🗓️ **SCHEDULE LOG**", AuthoredDepth::First),
            bullet("Some entry", AuthoredDepth::Nested),
        ];
        let rendered = render_project_note(&base_input(&sub_bullets));
        assert_eq!(rendered.contents.matches("## ").count(), 1);
        assert!(
            rendered
                .contents
                .contains("- [ ] #task 🗓️ **SCHEDULE LOG** [created::2026-09-20]\n")
        );
        assert_eq!(rendered.task_count, 1);
    }
}

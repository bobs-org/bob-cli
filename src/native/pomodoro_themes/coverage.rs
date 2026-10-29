//! Note-default and task-field coverage for `bob types rename` / `split`.
//!
//! Rename rewrites matching `task_type:` / `theme:` frontmatter on
//! area/project notes and `[type::]` / `(type::)` plus legacy `[theme::]` /
//! `(theme::)` fields on task lines. Split leaves both alone
//! and warns with leftover counts.

use std::{fs, path::Path};

use crate::native::{
    capture_dependency_tasks,
    markdown::split_line_ending,
    note_tasks::{self, NoteTaskSettings},
    plan_budget::normalize_component,
    pomodoro_homes::{NoteFacts, NoteKind, NoteTheme},
    projects::trim_yaml_scalar,
    task_fields::{canonical_theme_name, inline_fields},
};

fn raw_lines_preserving(contents: &str) -> Vec<String> {
    contents.split_inclusive('\n').map(str::to_string).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LineEdit {
    pub(super) line: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) before: String,
    pub(super) after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LeftoverThemeUses {
    pub(super) defaults: usize,
    pub(super) default_notes: Vec<String>,
    pub(super) task_themes: usize,
}

pub(super) fn leftover_theme_uses(
    vault: &Path,
    old_key: &str,
) -> LeftoverThemeUses {
    let settings = note_tasks::read_settings(vault);
    let (paths, _) = capture_dependency_tasks::vault_note_paths(vault);
    let mut uses = LeftoverThemeUses::default();
    for relative in paths {
        let absolute = vault.join(&relative);
        let Ok(contents) = fs::read_to_string(&absolute) else {
            continue;
        };
        let path = capture_dependency_tasks::display_path(&relative);
        if !default_edits(&contents, old_key, "").is_empty() {
            uses.defaults += 1;
            uses.default_notes.push(path);
        }
        uses.task_themes +=
            task_theme_edits(&contents, old_key, "", &settings).len();
    }
    uses
}

#[allow(dead_code)]
pub(super) fn default_edit(
    contents: &str,
    old_key: &str,
    new_display: &str,
) -> Option<LineEdit> {
    default_edits(contents, old_key, new_display)
        .into_iter()
        .next()
}

pub(super) fn default_edits(
    contents: &str,
    old_key: &str,
    new_display: &str,
) -> Vec<LineEdit> {
    let facts = NoteFacts::parse(contents);
    match (facts.kind, facts.theme) {
        (NoteKind::Area | NoteKind::Project, Some(NoteTheme::Name(name)))
            if normalize_component(&name) == old_key => {}
        _ => return Vec::new(),
    }
    let owned = raw_lines_preserving(contents);
    let mut edits = Vec::new();
    for (index, raw) in owned.iter().enumerate() {
        if index == 0 {
            continue;
        }
        let (body, _) = split_line_ending(raw);
        if body.trim() == "---" {
            break;
        }
        for key in ["task_type", "theme"] {
            let Some((start, end, before)) =
                frontmatter_default_value_span(body, key, old_key)
            else {
                continue;
            };
            edits.push(LineEdit {
                line: index + 1,
                start,
                end,
                before,
                after: new_display.to_string(),
            });
        }
    }
    edits
}

pub(super) fn leftover_warning(
    old_name: &str,
    uses: &LeftoverThemeUses,
) -> Option<String> {
    if uses.defaults == 0 && uses.task_themes == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if uses.defaults > 0 {
        parts.push(format!(
            "{} default{}",
            uses.defaults,
            if uses.defaults == 1 { "" } else { "s" }
        ));
    }
    if uses.task_themes > 0 {
        parts.push(format!(
            "{} task field{}",
            uses.task_themes,
            if uses.task_themes == 1 { "" } else { "s" }
        ));
    }
    Some(format!(
        "split_leaves_task_themes: {} still name {old_name}",
        parts.join(", ")
    ))
}

pub(super) fn task_theme_edits(
    contents: &str,
    old_key: &str,
    new_display: &str,
    settings: &NoteTaskSettings,
) -> Vec<LineEdit> {
    let scan = note_tasks::scan(contents, settings);
    let owned = raw_lines_preserving(contents);
    let mut edits = Vec::new();
    for task in scan.tasks() {
        let Some(raw) = owned.get(task.line_index) else {
            continue;
        };
        let (body, _) = split_line_ending(raw);
        let mut fields = inline_fields(body, "type");
        fields.extend(inline_fields(body, "theme"));
        fields.sort_by_key(|field| field.value_start);
        for field in fields {
            let Some(name) = canonical_theme_name(&field.value) else {
                continue;
            };
            if normalize_component(&name) != old_key {
                continue;
            }
            edits.push(LineEdit {
                line: task.line_index + 1,
                start: field.value_start,
                end: field.value_end,
                before: field.value,
                after: new_display.to_string(),
            });
        }
    }
    edits
}

pub(super) fn apply_line_edits(contents: &str, edits: &[LineEdit]) -> String {
    if edits.is_empty() {
        return contents.to_string();
    }
    let mut owned = raw_lines_preserving(contents);
    let mut by_line: Vec<(usize, Vec<&LineEdit>)> = Vec::new();
    for edit in edits {
        match by_line.iter_mut().find(|(line, _)| *line == edit.line) {
            Some((_, group)) => group.push(edit),
            None => by_line.push((edit.line, vec![edit])),
        }
    }
    for (line, mut group) in by_line {
        let index = line.saturating_sub(1);
        let Some(raw) = owned.get(index).cloned() else {
            continue;
        };
        let (body, ending) = split_line_ending(&raw);
        group.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        let mut rewritten = body.to_string();
        for edit in group {
            if edit.end > rewritten.len() || edit.start > edit.end {
                continue;
            }
            rewritten.replace_range(edit.start..edit.end, &edit.after);
        }
        owned[index] = format!("{rewritten}{ending}");
    }
    owned.concat()
}

fn frontmatter_default_value_span(
    line: &str,
    key: &str,
    old_key: &str,
) -> Option<(usize, usize, String)> {
    let rest = line.strip_prefix(&format!("{key}:"))?;
    let leading = rest.len().saturating_sub(rest.trim_start().len());
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (inner, quote_prefix) = quoted_scalar(trimmed);
    let name = canonical_theme_name(inner)?;
    if normalize_component(&name) != old_key {
        return None;
    }
    let start = key.len() + 1 + leading + quote_prefix;
    let end = start + inner.len();
    Some((start, end, inner.to_string()))
}

fn quoted_scalar(trimmed: &str) -> (&str, usize) {
    let inner = trim_yaml_scalar(trimmed);
    if inner.len() + 2 == trimmed.len()
        && (trimmed.starts_with('"') && trimmed.ends_with('"')
            || trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        (inner, 1)
    } else {
        (inner, 0)
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use std::path::Path;

    fn settings() -> NoteTaskSettings {
        note_tasks::read_settings(Path::new("/nonexistent"))
    }

    #[test]
    fn default_edit_preserves_quote_styles() {
        let unquoted = "---\ntype: [[area]]\ntheme: BUILD\n---\n";
        let double = "---\ntype: [[project]]\ntheme: \"BUILD\"\n---\n";
        let single = "---\ntype: [[area]]\ntheme: 'BUILD'\n---\n";
        let padded = "---\ntype: [[area]]\ntheme:  BUILD  \n---\n";
        let edit = default_edit(unquoted, "build", "FIX").expect("unquoted");
        assert_eq!(edit.line, 3);
        assert_eq!(edit.before, "BUILD");
        assert_eq!(
            apply_line_edits(unquoted, &[edit]),
            "---\ntype: [[area]]\ntheme: FIX\n---\n"
        );
        let edit = default_edit(double, "build", "FIX").expect("double");
        assert_eq!(
            apply_line_edits(double, &[edit]),
            "---\ntype: [[project]]\ntheme: \"FIX\"\n---\n"
        );
        let edit = default_edit(single, "build", "FIX").expect("single");
        assert_eq!(
            apply_line_edits(single, &[edit]),
            "---\ntype: [[area]]\ntheme: 'FIX'\n---\n"
        );
        let edit = default_edit(padded, "build", "FIX").expect("padded");
        assert_eq!(
            apply_line_edits(padded, &[edit]),
            "---\ntype: [[area]]\ntheme:  FIX  \n---\n"
        );
    }

    #[test]
    fn default_edits_cover_canonical_and_legacy_spellings() {
        let canonical = "---\ntype: [[area]]\ntask_type: BUILD\n---\n";
        let edits = default_edits(canonical, "build", "FIX");
        assert_eq!(edits.len(), 1);
        assert_eq!(
            apply_line_edits(canonical, &edits),
            "---\ntype: [[area]]\ntask_type: FIX\n---\n"
        );
        let both = "---\ntype: [[area]]\ntask_type: BUILD\ntheme: BUILD\n---\n";
        let edits = default_edits(both, "build", "FIX");
        assert_eq!(edits.len(), 2);
        assert_eq!(
            apply_line_edits(both, &edits),
            "---\ntype: [[area]]\ntask_type: FIX\ntheme: FIX\n---\n"
        );
        let quoted = "---\ntype: [[project]]\ntask_type: \"BUILD\"\n---\n";
        let edits = default_edits(quoted, "build", "FIX");
        assert_eq!(
            apply_line_edits(quoted, &edits),
            "---\ntype: [[project]]\ntask_type: \"FIX\"\n---\n"
        );
    }

    #[test]
    fn default_edit_skips_other_kinds_and_off() {
        let other = "---\ntype: [[ref]]\ntheme: BUILD\n---\n";
        let off = "---\ntype: [[area]]\ntheme: off\n---\n";
        let invalid = "---\ntype: [[area]]\ntheme: BUILD + FIX\n---\n";
        assert!(default_edit(other, "build", "FIX").is_none());
        assert!(default_edit(off, "build", "FIX").is_none());
        assert!(default_edit(invalid, "build", "FIX").is_none());
    }

    #[test]
    fn task_theme_edits_cover_paren_and_skip_fenced_and_prose() {
        let contents = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Open [theme:: BUILD] ^open\n\
- [x] #task Done (theme:: BUILD) ^done\n\
Prose [theme:: BUILD] stays.\n\
```md\n\
- [ ] #task Fenced [theme:: BUILD]\n\
```\n\
- [ ] not a task [theme:: BUILD]\n";
        let edits = task_theme_edits(contents, "build", "FIX", &settings());
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].before, "BUILD");
        assert_eq!(edits[1].before, "BUILD");
        let after = apply_line_edits(contents, &edits);
        assert!(after.contains("[theme:: FIX]"), "{after}");
        assert!(after.contains("(theme:: FIX)"), "{after}");
        assert!(after.contains("Prose [theme:: BUILD] stays."), "{after}");
        assert!(
            after.contains("- [ ] #task Fenced [theme:: BUILD]"),
            "{after}"
        );
        assert!(
            after.contains("- [ ] not a task [theme:: BUILD]"),
            "{after}"
        );
    }

    #[test]
    fn task_theme_edits_cover_canonical_and_mixed_spellings() {
        let contents = "\
---\ntype: [[area]]\n---\n\
- [ ] #task Open [type:: BUILD] ^open\n\
- [ ] #task Legacy (theme:: BUILD) ^legacy\n\
- [ ] #task Mixed [type:: BUILD] (theme:: BUILD) ^mixed\n";
        let edits = task_theme_edits(contents, "build", "FIX", &settings());
        assert_eq!(edits.len(), 4);
        let after = apply_line_edits(contents, &edits);
        assert!(
            after.contains("- [ ] #task Open [type:: FIX] ^open"),
            "{after}"
        );
        assert!(
            after.contains("- [ ] #task Legacy (theme:: FIX) ^legacy"),
            "{after}"
        );
        assert!(
            after.contains(
                "- [ ] #task Mixed [type:: FIX] (theme:: FIX) ^mixed"
            ),
            "{after}"
        );
    }

    #[test]
    fn leftover_warning_names_counts() {
        let both = LeftoverThemeUses {
            defaults: 2,
            default_notes: vec!["dev.md".into(), "job.md".into()],
            task_themes: 3,
        };
        assert_eq!(
            leftover_warning("JOB", &both).as_deref(),
            Some("split_leaves_task_themes: 2 defaults, 3 task fields still name JOB")
        );
        let one = LeftoverThemeUses {
            defaults: 1,
            default_notes: vec!["dev.md".into()],
            task_themes: 0,
        };
        assert_eq!(
            leftover_warning("JOB", &one).as_deref(),
            Some("split_leaves_task_themes: 1 default still name JOB")
        );
        assert!(
            leftover_warning("JOB", &LeftoverThemeUses::default()).is_none()
        );
    }
}

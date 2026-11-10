use super::{
    super::model::{Candidates, SCHEMA_VERSION},
    result, write_file, TempDir,
};
use crate::native::{
    capture_language::{self, CompletionContext},
    env::{self, TestEnvGuard},
};
use std::{ffi::OsStr, path::Path};

fn directive_rows(
    root: &Path,
    raw: &str,
) -> Vec<super::super::model::DirectiveCandidate> {
    let value = result(root, raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::Directive));
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };
    rows
}

#[test]
fn bare_percent_opens_ordered_directive_menu_with_no_initial_selection() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-menu");
    let raw = "Remember this %";
    let value = result(temp.path(), raw, raw.len());

    assert_eq!(value.schema_version, SCHEMA_VERSION);
    assert_eq!(value.grammar, capture_language::GRAMMAR_REVISION);
    assert_eq!(value.context, Some(CompletionContext::Directive));
    assert_eq!(value.query.as_deref(), Some(""));
    assert_eq!(value.initial_selection, Some("none"));
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };
    assert_eq!(
        rows.iter()
            .map(|row| row.token.as_str())
            .collect::<Vec<_>>(),
        ["%c", "%n", "%t"]
    );
    assert!(rows[0].disabled_reason.is_none());
    assert!(rows[1].disabled_reason.is_some());
    assert_eq!(rows[0].replacement, "%c");
}

#[test]
fn shell_completion_exposes_safe_directive_rows_under_directives_group() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-shell");
    let _guard = TestEnvGuard::set(&[(
        "BOB_CONFIG_FILE",
        Some(OsStr::new("/nonexistent/bob-cli-test-config.yml")),
    )]);
    let raw = "Remember %";
    let shell =
        super::super::shell::shell_completion(temp.path(), raw, raw.len())
            .expect("shell result")
            .expect("directive completion");

    assert_eq!(
        shell
            .rows
            .iter()
            .map(|row| row.full.as_str())
            .collect::<Vec<_>>(),
        ["%c", "%t"]
    );
    assert!(shell.rows.iter().all(|row| row.group == "directives"));
    assert!(shell.rows[0].description.contains("clipboard"));
}

#[test]
fn directive_query_filters_by_letter_and_never_reads_clipboard() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-query");
    let raw = "Remember this %t";
    let value = result(temp.path(), raw, raw.len());

    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].token, "%t");
    assert_eq!(rows[0].replacement, "%t");
    assert_eq!(value.initial_selection, None);
}

#[test]
fn url_directive_menu_offers_listen_with_config_status() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-url");
    let raw = "https://example.com/article %";
    let rows = directive_rows(temp.path(), raw);

    assert_eq!(
        rows.iter()
            .map(|row| row.token.as_str())
            .collect::<Vec<_>>(),
        ["%c", "%n", "%t", "%l"]
    );
    let listen = rows.last().expect("Listen row");
    assert_eq!(listen.name, "Listen");
    assert!(listen
        .disabled_reason
        .as_deref()
        .unwrap()
        .contains("listen_command"));
    assert!(listen.replacement.is_empty());
}

#[test]
fn pomodoro_note_row_describes_the_selected_session() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-pomodoro");
    let day_file = temp.path().join("daily.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
    );
    let raw = "Remember this %";
    let value = env::with_var("BOB_DAY_FILE", day_file.as_os_str(), || {
        result(temp.path(), raw, raw.len())
    });
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };

    let note = rows.iter().find(|row| row.token == "%n").unwrap();
    assert_eq!(
        note.detail.as_deref(),
        Some("→ 08:30–08:55 PLAN · last completed")
    );
    assert!(note.disabled_reason.is_none());
    assert_eq!(note.replacement, "%n");
}

#[test]
fn pomodoro_note_row_names_a_trailing_range_as_running() {
    let temp =
        TempDir::new("bob-cli-capture-complete-directive-trailing-range");
    let day_file = temp.path().join("daily.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] (**1330-1355** [t:: 25m]) — PLAN\n- [ ] BUILD (1400-1425)\n",
    );
    let raw = "jot %n";
    let value = env::with_var("BOB_DAY_FILE", day_file.as_os_str(), || {
        result(temp.path(), raw, raw.len())
    });
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };

    let note = rows.iter().find(|row| row.token == "%n").unwrap();
    assert_eq!(
        note.detail.as_deref(),
        Some("→ BUILD (1400-1425) · running")
    );
    assert!(note.disabled_reason.is_none());
}

#[test]
fn pomodoro_note_row_disables_when_two_open_timed_entries_mix_formats() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-mixed-timed");
    let day_file = temp.path().join("daily.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**1330-1355** [t:: 25m]) — PLAN\n- [ ] BUILD (1400-1425)\n",
    );
    let raw = "jot %n";
    let value = env::with_var("BOB_DAY_FILE", day_file.as_os_str(), || {
        result(temp.path(), raw, raw.len())
    });
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };

    let note = rows.iter().find(|row| row.token == "%n").unwrap();
    assert_eq!(
        note.disabled_reason.as_deref(),
        Some("Today's note has multiple open timed Pomodoros.")
    );
}

#[test]
fn listen_row_activates_when_configured() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-listen");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "highlights:\n  listen_command: 'sase-listen render {target}'\n",
    );
    let raw = "https://example.com/article %";
    let _guard =
        TestEnvGuard::set(&[("BOB_CONFIG_FILE", Some(config.as_os_str()))]);
    let value = crate::native::capture_complete::engine::build_result(
        temp.path(),
        raw,
        raw.len(),
        false,
        false,
    )
    .unwrap();
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };

    let listen = rows.iter().find(|row| row.token == "%l").unwrap();
    assert!(listen.disabled_reason.is_none());
    assert_eq!(listen.replacement, "%l");
}

#[test]
fn bare_hash_type_menu_also_has_no_initial_selection() {
    let temp = TempDir::new("bob-cli-capture-complete-task-type-selection");
    let raw = "Fix it #";
    let value = result(temp.path(), raw, raw.len());

    assert_eq!(value.context, Some(CompletionContext::TaskType));
    assert_eq!(value.query.as_deref(), Some(""));
    assert_eq!(value.initial_selection, Some("none"));
}

#[test]
fn notebook_creation_directive_enables_without_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-notebook");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    // Complete caret seals enable `%n` as a notebook row even with no
    // Pomodoro in today's note.
    for raw in [
        "New notes @dev^nvim@ %",
        "@dev^nvim@ New notes %",
        "New notes @dev^nvim@ %n",
    ] {
        let rows = directive_rows(temp.path(), raw);
        let note = rows.iter().find(|row| row.token == "%n").expect("%n row");
        assert_eq!(note.name, "New notebook", "{raw}");
        assert_eq!(note.summary, "Create a notebook", "{raw}");
        assert!(
            note.disabled_reason.is_none(),
            "{raw}: {:?}",
            note.disabled_reason
        );
        assert_eq!(note.replacement, "%n");
    }
    // Empty summary still offers the row; the creation validator teaches
    // the required summary later.
    let rows = directive_rows(temp.path(), "@dev^nvim@ %");
    let note = rows.iter().find(|row| row.token == "%n").expect("%n row");
    assert!(note.disabled_reason.is_none());
    // Ordinary `%n` keeps Pomodoro availability: no Pomodoro means disabled.
    let rows = directive_rows(temp.path(), "Remember this %");
    let note = rows.iter().find(|row| row.token == "%n").expect("%n row");
    assert_eq!(note.name, "Pomodoro note");
    assert!(note.disabled_reason.is_some());
    // Invalid seals stay refused.
    for raw in [
        "New notes @dev:nvim@ %",
        "New notes @dev^nvim %",
        "New notes @dev^nvim@#extra %",
    ] {
        let rows = directive_rows(temp.path(), raw);
        let note = rows.iter().find(|row| row.token == "%n").expect("%n row");
        assert_eq!(note.name, "Pomodoro note", "{raw}");
        assert!(note.disabled_reason.is_some(), "{raw} should refuse");
    }
}

#[test]
fn directive_containing_menu_does_not_preselect_a_row() {
    let temp = TempDir::new("bob-cli-capture-complete-directive-template");
    let raw = "Apply {0} %t\n\n- first\n- second %";
    let value = result(temp.path(), raw, raw.len());

    assert_eq!(value.context, Some(CompletionContext::Directive));
    assert_eq!(value.initial_selection, Some("none"));
    let Candidates::Directive(rows) = value.candidates else {
        panic!("expected directive candidates");
    };
    let clip = rows.iter().find(|row| row.token == "%c").unwrap();
    assert!(clip
        .disabled_reason
        .as_deref()
        .unwrap()
        .contains("template"));
    assert!(clip.replacement.is_empty());
}

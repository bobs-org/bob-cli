//! The drawer menu: `task`, `recent`, and `new` rows, bob-authored copy,
//! refusals from the commit's own resolver, and the `#`-covering
//! replacement.

use super::{
    super::{
        drawers::target_drawer_candidates,
        model::{
            Candidates, CaptureCompleteResult, DrawerCandidate, Replacement,
        },
    },
    result, write_file, write_settings, TempDir,
};
use crate::native::{capture_language::CompletionContext, env::TestEnvGuard};

const TASK_NOTE: &str = concat!(
    "---\ntype: [[area]]\n---\n",
    "- [ ] #task Parent task ^bar\n",
    "\t- REQUIREMENTS\n",
    "\t\t- 2 GB RAM\n",
    "\t\t- fast disk\n",
    "\t- FUTURE WORKFLOW\n",
    "\t- NOTES\n",
    "\t\t- one\n",
    "\t- FUTURE WORK\n",
    "- [ ] #task Plain task ^plain\n",
);

/// A vault with the target note plus catalog notes: MOTIVATION in three
/// notes, ACCEPTANCE in two, and PERFORMANCE in one.
fn vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    write_settings(temp.path());
    write_file(&temp.path().join("foo.md"), TASK_NOTE);
    write_file(
        &temp.path().join("other1.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task One ^o1\n",
            "\t- MOTIVATION\n",
            "\t- ACCEPTANCE\n",
            "\t- PERFORMANCE\n",
        ),
    );
    write_file(
        &temp.path().join("other2.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Two ^o2\n",
            "\t- MOTIVATION\n",
            "\t- ACCEPTANCE\n",
        ),
    );
    write_file(
        &temp.path().join("other3.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Three ^o3\n",
            "\t- MOTIVATION\n",
        ),
    );
    temp
}

/// Keep the drawer catalog's cache out of the developer's real cache root.
fn isolate_cache() -> TestEnvGuard {
    let cache = std::env::temp_dir().join("bob-cli-drawer-menu-cache");
    TestEnvGuard::set(&[("XDG_CACHE_HOME", Some(cache.as_os_str()))])
}

fn menu(temp: &TempDir, raw: &str) -> CaptureCompleteResult {
    let _cache = isolate_cache();
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::Drawer), "{raw}");
    value
}

fn rows(value: &CaptureCompleteResult) -> &[DrawerCandidate] {
    let Candidates::Drawer(rows) = &value.candidates else {
        panic!("expected drawer candidates");
    };
    rows
}

/// `group:title` for each row, in order.
fn shape(value: &CaptureCompleteResult) -> Vec<String> {
    rows(value)
        .iter()
        .map(|row| format!("{}:{}", row.group, row.title))
        .collect()
}

fn row<'a>(
    value: &'a CaptureCompleteResult,
    group: &str,
    title: &str,
) -> &'a DrawerCandidate {
    rows(value)
        .iter()
        .find(|row| row.group == group && row.title == title)
        .unwrap_or_else(|| panic!("no {group} row {title}: {:?}", shape(value)))
}

#[test]
fn empty_query_lists_task_then_recent_rows_and_opens_unselected() {
    let temp = vault("bob-cli-drawer-menu-empty");
    let raw = "note @foo+bar#";
    let value = menu(&temp, raw);

    assert_eq!(value.initial_selection, Some("none"));
    assert_eq!(value.query.as_deref(), Some(""));
    assert_eq!(value.replacement, Replacement { start: 13, end: 14 });
    assert_eq!(
        shape(&value),
        [
            "task:REQUIREMENTS",
            "task:FUTURE WORKFLOW",
            "task:NOTES",
            "task:FUTURE WORK",
            // Used in at least two notes; PERFORMANCE (one note) stays off.
            "recent:MOTIVATION",
            "recent:ACCEPTANCE",
        ]
    );
    let target = value.target.as_ref().expect("target");
    assert_eq!(target.route, "foo.md");
    assert_eq!(target.block_id.as_deref(), Some("bar"));
    assert_eq!(target.text, "Parent task");
    assert!(value.warnings.is_empty());
}

#[test]
fn typed_query_drops_the_unselected_open_and_filters_every_group() {
    let temp = vault("bob-cli-drawer-menu-typed");
    let value = menu(&temp, "note @foo+bar#future");

    assert_eq!(value.initial_selection, None);
    assert_eq!(value.query.as_deref(), Some("future"));
    assert_eq!(
        shape(&value)[..3],
        ["task:FUTURE WORKFLOW", "task:FUTURE WORK", "new:FUTURE"]
    );

    // A typed query searches the whole catalog, even single-note keys.
    let perf = menu(&temp, "note @foo+bar#perf");
    assert_eq!(shape(&perf), ["recent:PERFORMANCE", "new:PERF"]);
}

#[test]
fn rows_rank_prefix_before_substring_in_document_order() {
    let temp = vault("bob-cli-drawer-menu-rank");
    let value = menu(&temp, "note @foo+bar#work");
    let task_titles: Vec<&str> = rows(&value)
        .iter()
        .filter(|row| row.group == "task")
        .map(|row| row.title.as_str())
        .collect();
    assert_eq!(task_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);

    // A quoted partial matches the hyphenated slug of a multi-word title.
    let quoted = menu(&temp, "note @foo+bar#\"FUTURE W");
    assert_eq!(quoted.query.as_deref(), Some("FUTURE W"));
    assert_eq!(
        shape(&quoted)[..2],
        ["task:FUTURE WORKFLOW", "task:FUTURE WORK"]
    );
}

#[test]
fn task_rows_carry_counts_singular_plural_detail_and_consequences() {
    let temp = vault("bob-cli-drawer-menu-task-copy");
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t\t- 2 GB RAM\n",
            "\t\t- fast disk\n",
            "\t- NOTES\n",
            "\t\t- only one\n",
            "\t\t\t- nested detail\n",
            "\t- EMPTY\n",
            "\t- LONG\n",
            "\t\t- a very long entry text that keeps on going past the cap\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
        ),
    );
    let value = menu(&temp, "note @foo+bar#");

    let requirements = row(&value, "task", "REQUIREMENTS");
    assert_eq!(requirements.entry_count, 2);
    assert_eq!(requirements.detail, "2 notes");
    assert_eq!(
        requirements.consequence.as_deref(),
        Some("Adds after \"fast disk\" in REQUIREMENTS")
    );
    assert!(!requirements.creates);

    let notes = row(&value, "task", "NOTES");
    assert_eq!(notes.detail, "1 note");
    // Nested detail is not an entry: the capture lands after "only one".
    assert_eq!(
        notes.consequence.as_deref(),
        Some("Adds after \"only one\" in NOTES")
    );

    let empty = row(&value, "task", "EMPTY");
    assert_eq!(empty.detail, "empty");
    assert_eq!(empty.consequence.as_deref(), Some("Adds to EMPTY"));

    let long = row(&value, "task", "LONG");
    let consequence = long.consequence.as_deref().expect("consequence");
    assert!(consequence.contains('…'), "{consequence}");
    assert!(consequence.ends_with("\" in LONG"), "{consequence}");
}

#[test]
fn recent_rows_are_catalog_keys_the_task_lacks() {
    let temp = vault("bob-cli-drawer-menu-recent");
    let value = menu(&temp, "note @foo+bar#");

    let motivation = row(&value, "recent", "MOTIVATION");
    assert!(motivation.creates);
    assert_eq!(motivation.note_count, Some(3));
    assert!(motivation.last_seen.is_some_and(|seen| seen > 0));
    assert_eq!(motivation.detail, "used in 3 notes · today");
    assert_eq!(motivation.replacement, "#motivation");
    assert_eq!(
        motivation.consequence.as_deref(),
        Some("Creates MOTIVATION under ^bar")
    );
    assert_eq!(motivation.refused, None);
    assert_eq!(motivation.line, 0);

    let acceptance = row(&value, "recent", "ACCEPTANCE");
    assert_eq!(acceptance.detail, "used in 2 notes · today");

    // Keys already on the task never repeat as recent rows.
    assert!(
        !rows(&value)
            .iter()
            .any(|row| row.group == "recent" && row.title == "REQUIREMENTS"),
        "{:?}",
        shape(&value)
    );

    let single = menu(&temp, "note @foo+bar#perfo");
    assert_eq!(
        row(&single, "recent", "PERFORMANCE").detail,
        "used in 1 note · today"
    );
}

#[test]
fn empty_query_recent_is_capped_at_eight_keys() {
    let temp = vault("bob-cli-drawer-menu-cap");
    for index in 0..12 {
        let body = format!(
            "- [ ] #task N ^n\n\t- TOPIC {}\n",
            char::from(b'A' + u8::try_from(index).expect("small"))
        );
        write_file(&temp.path().join(format!("a{index}.md")), &body);
        write_file(&temp.path().join(format!("b{index}.md")), &body);
    }
    let value = menu(&temp, "note @foo+bar#");
    let recent = rows(&value)
        .iter()
        .filter(|row| row.group == "recent")
        .count();
    assert_eq!(recent, 8);
}

#[test]
fn new_row_needs_a_typed_name_no_other_row_claims() {
    let temp = vault("bob-cli-drawer-menu-new");

    let empty = menu(&temp, "note @foo+bar#");
    assert!(!shape(&empty).iter().any(|row| row.starts_with("new:")));

    let typed = menu(&temp, "note @foo+bar#budget");
    assert_eq!(shape(&typed), ["new:BUDGET"]);
    let new = row(&typed, "new", "BUDGET");
    assert!(new.creates);
    assert_eq!(new.replacement, "#budget");
    assert_eq!(
        new.consequence.as_deref(),
        Some("Creates BUDGET under ^bar")
    );
    assert_eq!(new.detail, "");
    assert_eq!(new.refused, None);

    // An exact key on the task or in Recent claims the name.
    for raw in [
        "note @foo+bar#requirements",
        "note @foo+bar#future-work",
        "note @foo+bar#motivation",
        "note @foo+bar#\"MOTIVATION\"",
    ] {
        let value = menu(&temp, raw);
        assert!(
            !shape(&value).iter().any(|row| row.starts_with("new:")),
            "{raw}: {:?}",
            shape(&value)
        );
    }

    // A name that cannot be a drawer title offers nothing to create.
    let invalid = menu(&temp, "note @foo+bar#\"?!\"");
    assert!(shape(&invalid).is_empty(), "{:?}", shape(&invalid));
}

#[test]
fn new_row_hints_at_a_near_catalog_key() {
    let temp = vault("bob-cli-drawer-menu-near-catalog");
    let value = menu(&temp, "note @foo+bar#motivaton");
    let new = row(&value, "new", "MOTIVATON");
    assert_eq!(new.refused, None);
    assert_eq!(new.detail, "did you mean MOTIVATION?");
}

#[test]
fn refused_rows_carry_the_commit_resolvers_reason() {
    let temp = vault("bob-cli-drawer-menu-refused");
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t- NOTES\n",
            "\t- NOTES\n",
        ),
    );

    // Reserved: the reason names the key the writer would use.
    let reserved = menu(&temp, "note @foo+bar#tasks");
    let tasks = row(&reserved, "new", "TASKS");
    assert_eq!(
        tasks.refused.as_deref(),
        Some("`TASKS` is managed by Bob; name the drawer something else")
    );
    assert_eq!(tasks.consequence, None);
    let links = menu(&temp, "note @foo+bar#\"links\"");
    assert_eq!(
        row(&links, "new", "LINKS").refused.as_deref(),
        Some("`LINKS` is managed by Bob; name the drawer something else")
    );

    // Near-duplicate: a strict prefix of an existing key, with the
    // quoted override spelled out.
    let near = menu(&temp, "note @foo+bar#req");
    let req = row(&near, "new", "REQ");
    assert_eq!(
        req.refused.as_deref(),
        Some("did you mean `#requirements`? write `#\"REQ\"` to create REQ")
    );

    // Ambiguous: duplicate drawers are both listed, both refused.
    let ambiguous = menu(&temp, "note @foo+bar#notes");
    let duplicates: Vec<&DrawerCandidate> = rows(&ambiguous)
        .iter()
        .filter(|row| row.group == "task")
        .collect();
    assert_eq!(duplicates.len(), 2);
    for duplicate in duplicates {
        assert_eq!(duplicate.replacement, "#\"NOTES\"");
        assert_eq!(
            duplicate.refused.as_deref(),
            Some("drawer `NOTES` matches more than one drawer (lines 6, 7); rename one")
        );
        assert_eq!(duplicate.consequence, None);
    }
}

#[test]
fn replacement_is_a_slug_only_when_it_resolves_back_to_the_same_drawer() {
    let temp = vault("bob-cli-drawer-menu-quoting");
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- FUTURE WORK\n",
            "\t- FUTURE-WORK\n",
            "\t- Q&A\n",
            "\t- 2 GB\n",
            "\t- PLAIN\n",
        ),
    );
    let value = menu(&temp, "note @foo+bar#");

    // Both hyphen interpretations collide on the slug: quote each.
    assert_eq!(
        row(&value, "task", "FUTURE WORK").replacement,
        "#\"FUTURE WORK\""
    );
    assert_eq!(
        row(&value, "task", "FUTURE-WORK").replacement,
        "#\"FUTURE-WORK\""
    );
    // A title no slug can spell is quoted.
    assert_eq!(row(&value, "task", "Q&A").replacement, "#\"Q&A\"");
    assert_eq!(row(&value, "task", "2 GB").replacement, "#\"2 GB\"");
    assert_eq!(row(&value, "task", "PLAIN").replacement, "#plain");
    // Ambiguity is judged against the whole list, not the filtered rows.
    let filtered = menu(&temp, "note @foo+bar#future-work");
    assert!(
        rows(&filtered)
            .iter()
            .filter(|row| row.group == "task")
            .all(|row| row.replacement.starts_with("#\"")),
        "{:?}",
        rows(&filtered)
            .iter()
            .map(|row| &row.replacement)
            .collect::<Vec<_>>()
    );
}

#[test]
fn replacement_covers_the_hash_and_the_name_and_reports_cursor_after() {
    let temp = vault("bob-cli-drawer-menu-replacement");
    let raw = "note @foo+bar#req";
    let hash = raw.find('#').expect("hash");
    let value = menu(&temp, raw);
    assert_eq!(
        value.replacement,
        Replacement {
            start: hash,
            end: raw.len()
        }
    );
    let requirements = row(&value, "task", "REQUIREMENTS");
    assert_eq!(requirements.replacement, "#requirements");
    assert_eq!(
        requirements.cursor_after,
        Some(hash + "#requirements".len())
    );

    // A quoted partial replaces the quotes too, and the cursor lands after
    // the quoted replacement.
    let quoted_raw = "note @foo+bar#\"FUTURE W";
    let quoted = menu(&temp, quoted_raw);
    assert_eq!(
        quoted.replacement,
        Replacement {
            start: hash,
            end: quoted_raw.len()
        }
    );
    assert_eq!(
        row(&quoted, "task", "FUTURE WORK").cursor_after,
        Some(hash + "#future-work".len())
    );
}

#[test]
fn unresolvable_targets_report_a_diagnostic_and_no_rows() {
    let temp = vault("bob-cli-drawer-menu-diagnostics");
    write_file(
        &temp.path().join("dup.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task One ^same\n",
            "- [ ] #task Two ^same\n",
            "\t- REQUIREMENTS\n",
            "- plain bullet ^notatask\n",
        ),
    );

    let cases = [
        ("note @foo+nope#", "no task with block ID ^nope in foo.md"),
        ("note @nonote+bar#", "note does not exist: nonote.md"),
        (
            "note @dup+same#",
            "block ID ^same appears 2 times in dup.md",
        ),
        ("note @dup+notatask#", "^notatask in dup.md is not a task"),
    ];
    for (raw, warning) in cases {
        for typed in ["", "req"] {
            let draft = format!("{raw}{typed}");
            let value = menu(&temp, &draft);
            assert_eq!(rows(&value).len(), 0, "{draft}");
            assert_eq!(value.warnings, [warning], "{draft}");
        }
    }

    // An empty block ID is a successful empty list, without a warning.
    let empty = menu(&temp, "note @foo+#");
    assert_eq!(rows(&empty).len(), 0);
    assert!(empty.warnings.is_empty());
}

#[test]
fn a_task_without_drawers_lists_recent_and_new_only() {
    let temp = vault("bob-cli-drawer-menu-plain");
    let value = menu(&temp, "note @foo+plain#");
    assert_eq!(shape(&value), ["recent:MOTIVATION", "recent:ACCEPTANCE"]);
    assert_eq!(
        row(&value, "recent", "MOTIVATION").consequence.as_deref(),
        Some("Creates MOTIVATION under ^plain")
    );
}

#[test]
fn project_note_prj_target_lists_sections_and_creates_title_case_sections() {
    let temp = vault("bob-cli-drawer-menu-prj");
    write_file(
        &temp.path().join("proj.md"),
        concat!(
            "---\ntype: \"[[project]]\"\n---\n\n",
            "# Project\n\n",
            "- [ ] #task #prj Ship it ^prj\n\n",
            "## Requirements\n\n",
            "- first\n",
            "- second\n\n",
            "## Pomodoros\n\n",
            "- [ ] (0900-0930) Work\n\n",
            "## Future Work\n\n",
            "- existing\n",
        ),
    );
    let value = menu(&temp, "note @proj+prj#");
    assert_eq!(value.initial_selection, Some("none"));
    assert_eq!(
        shape(&value),
        [
            "task:Requirements",
            "task:Future Work",
            "recent:Motivation",
            "recent:Acceptance",
        ]
    );
    let requirements = row(&value, "task", "Requirements");
    assert_eq!(requirements.detail, "2 notes");
    assert_eq!(
        requirements.consequence.as_deref(),
        Some("Adds to ## Requirements")
    );
    assert_eq!(requirements.replacement, "#requirements");
    assert_eq!(
        row(&value, "recent", "Motivation").consequence.as_deref(),
        Some("Creates ## Motivation")
    );

    // A structural heading refuses, and points at the section syntax.
    let structural = menu(&temp, "note @proj+prj#pomodoros");
    assert_eq!(
        row(&structural, "new", "POMODOROS").refused.as_deref(),
        Some("use `@proj#<Heading>` to capture into a project `##` section")
    );
}

#[test]
fn shell_drawer_rows_are_the_targets_own_unrefused_drawers() {
    let temp = vault("bob-cli-drawer-menu-shell");
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t- NOTES\n",
            "\t- NOTES\n",
        ),
    );
    let (candidates, warnings) =
        target_drawer_candidates(temp.path(), "foo", Some("bar"))
            .expect("shell rows");
    let Candidates::Drawer(items) = candidates else {
        panic!("expected drawer candidates");
    };
    // Duplicates are refused; no catalog rows join the shell list.
    let titles: Vec<&str> =
        items.iter().map(|item| item.title.as_str()).collect();
    assert_eq!(titles, ["REQUIREMENTS"]);
    assert_eq!(items[0].replacement, "#requirements");
    assert!(warnings.is_empty());
}

// ---------------------------------------------------------------------------
// The drawer menu at a first-level bullet's leading `#`.
// ---------------------------------------------------------------------------

/// A bullet menu with the cursor at the end of the draft.
fn bullet(temp: &TempDir, raw: &str) -> CaptureCompleteResult {
    menu(temp, raw)
}

/// `true` when the draft's cursor offers no completion at all.
fn is_empty_success(temp: &TempDir, raw: &str, cursor: usize) -> bool {
    let _cache = isolate_cache();
    let value = result(temp.path(), raw, cursor);
    value.context.is_none() && value.candidates.is_empty()
}

#[test]
fn bullet_menu_on_an_existing_task_offers_bare_titles_with_the_glued_copy() {
    let temp = vault("bob-cli-bullet-menu-existing");
    let raw = "@foo+bar\n- #";
    let value = bullet(&temp, raw);

    assert_eq!(value.initial_selection, Some("none"));
    assert_eq!(value.query.as_deref(), Some(""));
    assert_eq!(
        value.replacement,
        Replacement {
            start: raw.len() - 1,
            end: raw.len()
        }
    );
    assert_eq!(
        shape(&value),
        [
            "task:REQUIREMENTS",
            "task:FUTURE WORKFLOW",
            "task:NOTES",
            "task:FUTURE WORK",
            "recent:MOTIVATION",
            "recent:ACCEPTANCE",
        ]
    );
    let target = value.target.as_ref().expect("target");
    assert_eq!(target.route, "foo.md");
    assert_eq!(target.block_id.as_deref(), Some("bar"));
    assert_eq!(target.text, "Parent task");

    let requirements = row(&value, "task", "REQUIREMENTS");
    assert_eq!(requirements.replacement, "REQUIREMENTS\n  - ");
    assert_eq!(
        requirements.consequence.as_deref(),
        Some("Adds after \"fast disk\" in REQUIREMENTS")
    );
    assert_eq!(requirements.detail, "2 notes");
    assert_eq!(
        row(&value, "recent", "MOTIVATION").consequence.as_deref(),
        Some("Creates MOTIVATION under ^bar")
    );
    // Every replacement is a bare title: never `#slug` or `#"TITLE"`.
    for item in rows(&value) {
        assert!(!item.replacement.starts_with('#'), "{item:?}");
    }
}

#[test]
fn bullet_replacement_continues_with_a_nested_bullet_only_when_nothing_follows()
{
    let temp = vault("bob-cli-bullet-menu-continuation");
    let hash = "@foo+bar\n- ".len();

    // Token last on its line, nothing after: the continuation opens the
    // drawer's first note and the caret lands past it.
    let end = bullet(&temp, "@foo+bar\n- #req");
    let requirements = row(&end, "task", "REQUIREMENTS");
    assert_eq!(requirements.replacement, "REQUIREMENTS\n  - ");
    assert_eq!(
        requirements.cursor_after,
        Some(hash + "REQUIREMENTS\n  - ".len())
    );

    // A nested bullet already follows: bare title, caret after the title.
    let nested_raw = "@foo+bar\n- #req\n  - already";
    let _cache = isolate_cache();
    let nested = result(temp.path(), nested_raw, hash + "#req".len());
    let requirements = row(&nested, "task", "REQUIREMENTS");
    assert_eq!(requirements.replacement, "REQUIREMENTS");
    assert_eq!(requirements.cursor_after, Some(hash + "REQUIREMENTS".len()));

    // A sibling first-level bullet is not a nested bullet.
    let sibling_raw = "@foo+bar\n- #req\n- other";
    let sibling = result(temp.path(), sibling_raw, hash + "#req".len());
    assert_eq!(
        row(&sibling, "task", "REQUIREMENTS").replacement,
        "REQUIREMENTS\n  - "
    );

    // Text after the token on the line: no continuation.
    let trailing_raw = "@foo+bar\n- #req tail";
    let trailing = result(temp.path(), trailing_raw, hash + "#req".len());
    assert_eq!(
        row(&trailing, "task", "REQUIREMENTS").replacement,
        "REQUIREMENTS"
    );
}

#[test]
fn bullet_menu_filters_by_the_typed_name_and_decodes_a_quoted_literal() {
    let temp = vault("bob-cli-bullet-menu-typed");
    let value = bullet(&temp, "@foo+bar\n- #future");
    assert_eq!(value.initial_selection, None);
    assert_eq!(value.query.as_deref(), Some("future"));
    assert_eq!(
        shape(&value),
        ["task:FUTURE WORKFLOW", "task:FUTURE WORK", "new:FUTURE"]
    );

    // A closed literal queries the whole decoded name and its replacement
    // covers the quotes too.
    let raw = "@foo+bar\n- #\"FUTURE WORK\"";
    let quoted = bullet(&temp, raw);
    assert_eq!(quoted.query.as_deref(), Some("FUTURE WORK"));
    assert_eq!(
        quoted.replacement,
        Replacement {
            start: raw.len() - "#\"FUTURE WORK\"".len(),
            end: raw.len()
        }
    );
    assert_eq!(
        row(&quoted, "task", "FUTURE WORK").replacement,
        "FUTURE WORK\n  - "
    );
}

#[test]
fn bullet_new_row_spells_a_typed_slug_as_the_title_the_shortcut_creates() {
    let temp = vault("bob-cli-bullet-menu-new");
    let value = bullet(&temp, "@foo+bar\n- #brand-new");
    let created = row(&value, "new", "BRAND NEW");
    assert_eq!(created.replacement, "BRAND NEW\n  - ");
    assert_eq!(
        created.consequence.as_deref(),
        Some("Creates BRAND NEW under ^bar")
    );

    // A quoted literal keeps its hyphen.
    let literal = bullet(&temp, "@foo+bar\n- #\"brand-new\"");
    assert_eq!(
        row(&literal, "new", "BRAND-NEW").replacement,
        "BRAND-NEW\n  - "
    );
}

#[test]
fn bullet_menu_on_a_new_task_offers_recent_and_new_rows_only() {
    let temp = vault("bob-cli-bullet-menu-new-task");
    let value = bullet(&temp, "Ship it\n- #");
    assert_eq!(shape(&value), ["recent:MOTIVATION", "recent:ACCEPTANCE"]);
    let target = value.target.as_ref().expect("target");
    assert_eq!(target.route, "mac_inbox.md");
    assert_eq!(target.block_id, None);
    assert_eq!(target.text, "Ship it");
    assert_eq!(
        row(&value, "recent", "MOTIVATION").consequence.as_deref(),
        Some("Starts MOTIVATION on the new task")
    );

    // The item's route, and a global `@@route`, name the destination.
    let routed = bullet(&temp, "Ship it @foo\n- #mot");
    assert_eq!(routed.target.as_ref().expect("target").route, "foo.md");
    assert_eq!(
        shape(&routed),
        ["recent:MOTIVATION", "new:MOT"],
        "a prefix of a catalog key still gets its own new row"
    );
    let global = bullet(&temp, "@@foo\nShip it\n- #");
    assert_eq!(global.target.as_ref().expect("target").route, "foo.md");

    // A new task owns no drawers, so a repeated name never shows a task
    // row, only the create rows.
    let typed = bullet(&temp, "Ship it\n- #brand-new");
    assert_eq!(shape(&typed), ["new:BRAND NEW"]);
}

#[test]
fn bullet_menu_on_a_new_project_note_creates_title_case_sections() {
    let temp = vault("bob-cli-bullet-menu-project-note");
    let value = bullet(&temp, "@foo^fresh@ Plan it\n- #");
    assert_eq!(shape(&value), ["recent:Motivation", "recent:Acceptance"]);
    let target = value.target.as_ref().expect("target");
    assert_eq!(target.route, "foo_fresh.md");
    assert_eq!(target.block_id.as_deref(), Some("prj"));
    assert_eq!(target.text, "Plan it");
    let motivation = row(&value, "recent", "Motivation");
    assert_eq!(
        motivation.consequence.as_deref(),
        Some("Creates ## Motivation")
    );
    // The bullet text stays ALL-CAPS: the project-note writer converts it.
    assert_eq!(motivation.replacement, "MOTIVATION\n  - ");

    // The offer form `@route:id@` is also a new project note.
    let offer = bullet(&temp, "@foo:fresh@ Plan it\n- #");
    assert_eq!(offer.target.as_ref().expect("target").route, "foo_fresh.md");
}

#[test]
fn bullet_menu_on_a_prj_task_lists_sections_and_refuses_structural_ones() {
    let temp = vault("bob-cli-bullet-menu-prj");
    write_file(
        &temp.path().join("proj.md"),
        concat!(
            "---\ntype: \"[[project]]\"\n---\n\n",
            "# Project\n\n",
            "- [ ] #task #prj Ship it ^prj\n\n",
            "## Requirements\n\n",
            "- first\n",
            "- second\n\n",
            "## Pomodoros\n\n",
            "- [ ] (0900-0930) Work\n\n",
            "## Future Work\n\n",
            "- existing\n",
        ),
    );
    let value = bullet(&temp, "@proj+prj\n- #");
    assert_eq!(
        shape(&value),
        [
            "task:Requirements",
            "task:Future Work",
            "recent:Motivation",
            "recent:Acceptance",
        ]
    );
    let requirements = row(&value, "task", "Requirements");
    assert_eq!(requirements.replacement, "REQUIREMENTS\n  - ");
    assert_eq!(
        requirements.consequence.as_deref(),
        Some("Adds to ## Requirements")
    );

    let structural = bullet(&temp, "@proj+prj\n- #pomodoros");
    assert_eq!(
        row(&structural, "new", "POMODOROS").refused.as_deref(),
        Some("use `@proj#<Heading>` to capture into a project `##` section")
    );
}

#[test]
fn bullet_rows_refuse_with_the_exact_resolvers_reason() {
    let temp = vault("bob-cli-bullet-menu-refused");
    write_file(
        &temp.path().join("foo.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Parent task ^bar\n",
            "\t- REQUIREMENTS\n",
            "\t- NOTES\n",
            "\t- NOTES\n",
        ),
    );

    let reserved = bullet(&temp, "@foo+bar\n- #tasks");
    let tasks = row(&reserved, "new", "TASKS");
    assert_eq!(
        tasks.refused.as_deref(),
        Some("`TASKS` is managed by Bob; name the drawer something else")
    );
    assert_eq!(tasks.consequence, None);

    // Exact resolution has no near-duplicate guard: `REQ` is a plain new
    // drawer for a bullet, just like typing `- REQ` with notes under it.
    let near = bullet(&temp, "@foo+bar\n- #req");
    let req = row(&near, "new", "REQ");
    assert_eq!(req.refused, None);
    assert_eq!(req.consequence.as_deref(), Some("Creates REQ under ^bar"));

    // Duplicate drawers are both listed and both refused.
    let ambiguous = bullet(&temp, "@foo+bar\n- #notes");
    let duplicates: Vec<&DrawerCandidate> = rows(&ambiguous)
        .iter()
        .filter(|row| row.group == "task")
        .collect();
    assert_eq!(duplicates.len(), 2);
    for duplicate in duplicates {
        assert_eq!(duplicate.replacement, "NOTES\n  - ");
        assert_eq!(
            duplicate.refused.as_deref(),
            Some("drawer `NOTES` matches more than one drawer (lines 6, 7); rename one")
        );
    }
}

#[test]
fn bullet_starts_that_cannot_name_a_drawer_are_empty_successes() {
    let temp = vault("bob-cli-bullet-menu-positions");
    for raw in [
        // The bullets nest under a same-line note.
        "@foo+bar fix it\n- #",
        // A glued drawer already files every bullet.
        "@foo+bar#requirements\n- #",
        // A Pomodoro note, a close, and an adjustment own their bullets.
        "%n\n- #",
        "x\n- #",
        "+5\n- #",
        // An explicit toggle refuses bullets.
        "@foo+bar!\n- #",
        // An escaped hash is prose.
        "Ship it\n- \\#",
    ] {
        assert!(
            is_empty_success(&temp, raw, raw.len()),
            "{raw:?} must not open a menu"
        );
    }

    // The cursor must sit after the `#`.
    let before = "Ship it\n- #fu";
    assert!(is_empty_success(&temp, before, before.len() - 3));
}

#[test]
fn nested_bullet_hash_still_offers_task_types() {
    let temp = vault("bob-cli-bullet-menu-nested-types");
    let _cache = isolate_cache();
    let raw = "Ship it\n- check\n  - #";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::TaskType));

    let parent = "Ship it #";
    let value = result(temp.path(), parent, parent.len());
    assert_eq!(value.context, Some(CompletionContext::TaskType));
}

#[test]
fn shell_completion_never_reaches_a_bullet_start() {
    // A shell command line is one line, so a bullet start (which needs a
    // second physical line) is outside shell completion.
    let temp = vault("bob-cli-bullet-menu-shell");
    let _cache = isolate_cache();
    let raw = "@foo+bar\n- #";
    let shell = crate::native::capture_complete::shell::shell_completion(
        temp.path(),
        raw,
        raw.len(),
    )
    .expect("shell completion");
    assert_eq!(shell, None);
}

//! Retired close spellings: whole-item `=x…`/`=X…`/`=*…`/`=!…` and the link
//! suffix `=*…`/`=!…` aliases fail with a teaching error that names the
//! rewrite, in both parsers and inside chains.

use super::super::editor_model::*;
use super::super::model::*;
use super::*;

fn retired_message(token: &str, rewrite: &str) -> String {
    format!(
        "`{token}` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `{rewrite}`"
    )
}

fn retired_alias_message(token: &str, rewrite: &str) -> String {
    format!(
        "`{token}` is retired: a link or task close spells the `x`; write `{rewrite}`"
    )
}

#[test]
fn retired_whole_item_closes_fail_with_the_bare_rewrite_in_both_parsers() {
    for (raw, token, rewrite) in [
        ("=x", "=x", "x"),
        ("=X", "=X", "x"),
        ("=x2!1", "=x2!1", "x2!1"),
        ("=X2", "=X2", "x2"),
        ("=x1,3!2", "=x1,3!2", "x1,3!2"),
        ("=x@2", "=x@2", "x@2"),
        ("=*", "=*", "x*"),
        ("=!", "=!", "x!"),
        ("=*1", "=*1", "x*1"),
        ("=!2", "=!2", "x!2"),
        ("=*!2", "=*!2", "x*!2"),
        ("=*abc", "=*abc", "x*abc"),
        ("=x#bugs", "=x#bugs", "x =#bugs"),
        ("=X#bugs", "=X#bugs", "x =#bugs"),
        ("=x1,", "=x1,", "x1,"),
        ("=x wired", "=x", "x wired"),
        ("=x2,3 2 foo", "=x2,3", "x2,3 2 foo"),
        ("=x\n- 1 wired", "=x", "x"),
    ] {
        let expected = retired_message(token, rewrite);
        assert_eq!(execute(raw).expect_err(raw), expected, "{raw}");

        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroClose, "{raw}");
        assert!(parse.pomodoro_close.is_none(), "{raw}");
        assert_eq!(codes(&parse), vec!["invalid_pomodoro_close"], "{raw}");
        assert_eq!(parse.diagnostics[0].message, expected, "{raw}");
        assert_eq!(parse.diagnostics[0].range, Some((0, token.len())), "{raw}");
        assert_eq!(
            ranges(&parse),
            vec![(0, token.len(), SpanKind::PomodoroClose)],
            "{raw}"
        );
    }
}

#[test]
fn retired_closes_with_an_inline_entry_name_the_whole_rewritten_line() {
    let error = execute("=x wired the lexer").expect_err("inline entry");
    assert_eq!(
        error,
        "`=x` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x wired the lexer`"
    );
    let error = execute("=x1,3 3 fixed it").expect_err("inline entry");
    assert_eq!(
        error,
        "`=x1,3` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x1,3 3 fixed it`"
    );
}

#[test]
fn retired_closes_stay_distinct_from_prose_lookalikes() {
    for raw in ["=xx", "=xa", "=x.", "Plan =x", "Plan =x1", "a=x"] {
        assert_eq!(execute(raw).expect(raw).kind, CaptureKind::Task, "{raw}");
        assert_eq!(editor(raw).mode, EditorMode::Task, "{raw}");
    }
}

#[test]
fn retired_closes_inside_chains_fail_on_their_own_range() {
    // The retired token fails on its own item; its siblings still parse.
    let parse = parse_for_editor("=x =");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroClose);
    assert!(parse.items[0].pomodoro_close.is_none());
    assert_eq!(
        parse.items[0].diagnostics[0].message,
        retired_message("=x", "x")
    );
    assert_eq!(parse.items[0].diagnostics[0].range, Some((0, 2)));
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroStart);
    assert!(parse.items[1].diagnostics.is_empty());
    assert!(execute_draft("=x =")
        .expect_err("retired chain")
        .contains("`=x` is retired"));

    let parse = parse_for_editor("+2 =x1!2");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(parse.items[0].mode, EditorMode::PomodoroAdjust);
    assert!(parse.items[0].diagnostics.is_empty());
    assert_eq!(
        parse.items[1].diagnostics[0].message,
        retired_message("=x1!2", "x1!2")
    );
    assert_eq!(parse.items[1].diagnostics[0].range, Some((3, 8)));

    // A bare `x` next to a retired token keeps its own valid parse.
    let parse = parse_for_editor("x =*");
    assert_eq!(parse.items.len(), 2);
    assert!(parse.items[0].diagnostics.is_empty());
    assert!(parse.items[0].pomodoro_close.is_some());
    assert_eq!(
        parse.items[1].diagnostics[0].message,
        retired_message("=*", "x*")
    );
    assert_eq!(parse.items[1].diagnostics[0].range, Some((2, 4)));

    // An inline owner carries the retired token and its entry.
    let parse = parse_for_editor("=x wired the lexer =");
    assert_eq!(parse.items.len(), 2);
    assert_eq!(
        parse.items[0].diagnostics[0].message,
        retired_message("=x", "x wired the lexer")
    );
    assert_eq!(parse.items[1].mode, EditorMode::PomodoroStart);
}

#[test]
fn double_equals_close_lookalikes_teach_the_bare_spelling() {
    for (raw, spelling) in [
        ("==x", "x"),
        ("==X", "x"),
        ("==x2", "x2"),
        ("==*2", "x*2"),
        ("==!", "x!"),
        ("==x#bugs", "x =#bugs"),
    ] {
        let expected = format!(
            "`{raw}` is not a close: close the running Pomodoro with `{spelling}`"
        );
        assert_eq!(execute(raw).expect_err(raw), expected, "{raw}");
        let parse = editor(raw);
        assert_eq!(parse.diagnostics[0].message, expected, "{raw}");
    }
}

#[test]
fn retired_link_close_aliases_fail_with_the_x_rewrite() {
    for (raw, suffix, rewrite) in [
        ("^bob:stop=*", "=*", "=x*"),
        ("^bob:stop=!", "=!", "=x!"),
        ("^bob:stop=*2,3", "=*2,3", "=x*2,3"),
        ("^bob:stop=!2", "=!2", "=x!2"),
        ("^bob:stop=*!2", "=*!2", "=x*!2"),
        ("@r:id=!2", "=!2", "=x!2"),
        ("Text @r:id=*", "=*", "=x*"),
    ] {
        let expected = retired_alias_message(suffix, rewrite);
        assert!(
            execute(raw).expect_err(raw).contains(&expected),
            "{raw}: {:?}",
            execute(raw)
        );
        let parse = editor(raw);
        assert!(parse.pomodoro_close.is_none(), "{raw}");
        assert_eq!(codes(&parse), vec!["invalid_pomodoro_close"], "{raw}");
        assert_eq!(parse.diagnostics[0].message, expected, "{raw}");
        let eq = raw.find('=').expect("suffix");
        assert_eq!(parse.diagnostics[0].range, Some((eq, raw.len())), "{raw}");
    }
    // The `=x…` link spellings stay valid.
    for raw in [
        "^bob:stop=x*",
        "^bob:stop=x!2",
        "@r:id=x!",
        "Text @r:id=x*1",
    ] {
        let parse = editor(raw);
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert!(parse.pomodoro_close.is_some(), "{raw}");
        assert!(execute(raw).is_ok(), "{raw}");
    }
}

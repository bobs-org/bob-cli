//! Shared lexical ownership for `z:<when>` completion.

use super::*;

#[test]
fn snooze_completion_uses_value_range_in_both_marker_orders() {
    for raw in ["Fix sink @home z:19", "Fix sink z:19 @home"] {
        let value_start = raw.find("z:").expect("marker") + 2;
        let value_end = raw[value_start..]
            .find(' ')
            .map_or(raw.len(), |space| value_start + space);
        let field = field(raw, value_end).expect("snooze completion");
        assert_eq!(field.context, CompletionContext::Snooze, "{raw}");
        assert_eq!(field.query, "19", "{raw}");
        assert_eq!(field.replacement, (value_start, value_end), "{raw}");
    }
}

#[test]
fn snooze_completion_keeps_utf8_offsets_and_accepts_an_empty_value() {
    let raw = "Réparer évier @home z:";
    let completion = field(raw, raw.len()).expect("bare snooze value");
    let value_start = raw.find("z:").expect("marker") + 2;
    assert_eq!(completion.context, CompletionContext::Snooze);
    assert_eq!(completion.query, "");
    assert_eq!(completion.replacement, (value_start, raw.len()));

    let raw = "Réparer évier @home z:15";
    let completion = field(raw, raw.len()).expect("partial value");
    assert_eq!(completion.context, CompletionContext::Snooze);
    assert_eq!(completion.query, "15");
    assert_eq!(
        completion.replacement,
        (raw.find("z:").unwrap() + 2, raw.len())
    );
}

#[test]
fn embedded_literals_and_child_prose_do_not_claim_snooze_completion() {
    for raw in [
        "Read https://example.com/z:19",
        "Read `z:19`",
        "Read `https://example.com/z:19`",
    ] {
        assert_ne!(
            field(raw, raw.len()).map(|field| field.context),
            Some(CompletionContext::Snooze),
            "{raw}"
        );
    }

    let raw = "Parent task\n- child prose z:19";
    let field = field(raw, raw.len());
    assert_ne!(
        field.map(|field| field.context),
        Some(CompletionContext::Snooze)
    );
}

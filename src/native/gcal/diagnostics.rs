//! Google clock deprecation diagnostics shared by list and sync.
//!
//! Reports deprecated Google clock spellings (`#offclock`, `#off-clock`,
//! `#untracked`) from parsed explicit tags only. Never scans rendered
//! vault rows or provider description text.

use serde::Serialize;

use crate::native::clock_tags::ClockTag;

/// One structured deprecation record per distinct normalized alias per
/// occurrence. Field names match the additive JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Deprecation {
    pub calendar_id: String,
    pub event_id: String,
    pub title: String,
    pub token: String,
    #[serde(rename = "where")]
    pub where_: String,
    pub replacement: String,
    pub drop_suggested: bool,
    pub on_wins: bool,
}

/// Inputs for one occurrence's diagnostic. Titles are the cleaned display
/// titles; deprecated vectors are the normalized lowercase aliases retained
/// by parsing. `root_only_off_clock` is the untagged chosen-root verdict;
/// it must never be derived from an event whose explicit off tag is still
/// present.
pub(crate) struct DiagnosticInput<'a> {
    pub calendar_id: &'a str,
    pub event_id: &'a str,
    pub title: &'a str,
    pub clock_tag: &'a ClockTag,
    pub deprecated_title_tags: &'a [String],
    pub deprecated_description_tags: &'a [String],
    pub root_value: Option<&'a str>,
    pub root_only_off_clock: bool,
    pub root_only_note: Option<&'a str>,
}

/// Collect one warning per deprecated occurrence plus one structured record
/// per distinct normalized alias. Warnings and records share stable
/// (sorted) token ordering. A canonical `#off` + `#on` conflict with no
/// deprecated spelling still warns with `clock_tag_conflict`.
pub(crate) fn collect(
    inputs: &[DiagnosticInput<'_>],
) -> (Vec<String>, Vec<Deprecation>) {
    let mut warnings = Vec::new();
    let mut deprecations = Vec::new();
    for input in inputs {
        let (warning, records) = for_occurrence(input);
        if let Some(warning) = warning {
            warnings.push(warning);
        }
        deprecations.extend(records);
    }
    (warnings, deprecations)
}

fn for_occurrence(
    input: &DiagnosticInput<'_>,
) -> (Option<String>, Vec<Deprecation>) {
    let on_wins = matches!(input.clock_tag, ClockTag::Conflict { .. });
    let mut tokens: Vec<String> = input
        .deprecated_title_tags
        .iter()
        .chain(input.deprecated_description_tags.iter())
        .cloned()
        .collect();
    tokens.sort();
    tokens.dedup();
    if tokens.is_empty() {
        if on_wins {
            return (
                Some(format!(
                    "{}: clock_tag_conflict: #off and #on are both present; #on wins",
                    input.title
                )),
                Vec::new(),
            );
        }
        return (None, Vec::new());
    }
    let in_title = |token: &str| {
        input.deprecated_title_tags.iter().any(|seen| seen == token)
    };
    let in_description = |token: &str| {
        input
            .deprecated_description_tags
            .iter()
            .any(|seen| seen == token)
    };
    // On-wins copy carries its own drop action; otherwise a usable
    // root-only off-clock verdict justifies the inherited drop advice.
    // A missing/unusable chosen root never justifies that parenthetical.
    let inherited_advice: Option<String> = if on_wins {
        None
    } else if !input.root_only_off_clock {
        None
    } else {
        match (
            input.root_value.filter(|root| !root.is_empty()),
            input.root_only_note.filter(|note| !note.is_empty()),
        ) {
            (Some(root), Some(note)) => {
                Some(format!("{root} is already off the clock via {note}"))
            }
            _ => None,
        }
    };
    let drop_suggested = on_wins || inherited_advice.is_some();
    let records = tokens
        .iter()
        .map(|token| Deprecation {
            calendar_id: input.calendar_id.to_string(),
            event_id: input.event_id.to_string(),
            title: input.title.to_string(),
            token: token.clone(),
            where_: where_for(in_title(token), in_description(token)),
            replacement: "#off".to_string(),
            drop_suggested,
            on_wins,
        })
        .collect();
    let warning = if on_wins {
        Some(on_wins_warning(input.title, &tokens))
    } else {
        Some(deprecated_warning(
            input.title,
            &tokens,
            inherited_advice.as_deref(),
        ))
    };
    (warning, records)
}

fn where_for(in_title: bool, in_description: bool) -> String {
    match (in_title, in_description) {
        (true, true) => "both".to_string(),
        (true, false) => "title".to_string(),
        (false, true) => "description".to_string(),
        (false, false) => "title".to_string(),
    }
}

fn deprecated_warning(
    title: &str,
    tokens: &[String],
    inherited_advice: Option<&str>,
) -> String {
    let joined = tokens.join(", ");
    let verb = if tokens.len() == 1 { "is" } else { "are" };
    let mut warning = format!(
        "{title}: {joined} {verb} deprecated in Google Calendar; write #off"
    );
    if let Some(advice) = inherited_advice {
        warning.push_str(&format!(" (or drop it: {advice})"));
    }
    warning
}

fn on_wins_warning(title: &str, tokens: &[String]) -> String {
    let joined = tokens.join(", ");
    if tokens.len() == 1 {
        format!(
            "{title}: {joined} is deprecated in Google Calendar and #on wins over it; drop {joined}"
        )
    } else {
        format!(
            "{title}: {joined} are deprecated in Google Calendar and #on wins over them; drop {joined}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        title: &'a str,
        clock_tag: &'a ClockTag,
        deprecated_title: &'a [String],
        deprecated_description: &'a [String],
        root_value: Option<&'a str>,
        root_only_off_clock: bool,
        root_only_note: Option<&'a str>,
    ) -> DiagnosticInput<'a> {
        DiagnosticInput {
            calendar_id: "primary",
            event_id: "swings",
            title,
            clock_tag,
            deprecated_title_tags: deprecated_title,
            deprecated_description_tags: deprecated_description,
            root_value,
            root_only_off_clock,
            root_only_note,
        }
    }

    #[test]
    fn single_alias_warns_with_write_off() {
        let tag = ClockTag::Off {
            deprecated: vec!["#offclock".to_string()],
        };
        let title = ["#offclock".to_string()];
        let (warnings, records) =
            collect(&[input("Swings!", &tag, &title, &[], None, false, None)]);
        assert_eq!(
            warnings,
            ["Swings!: #offclock is deprecated in Google Calendar; write #off"]
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, "#offclock");
        assert_eq!(records[0].where_, "title");
        assert_eq!(records[0].replacement, "#off");
        assert!(!records[0].drop_suggested);
        assert!(!records[0].on_wins);
    }

    #[test]
    fn inherited_advice_appends_drop_parenthetical() {
        let tag = ClockTag::Off {
            deprecated: vec!["#offclock".to_string()],
        };
        let title = ["#offclock".to_string()];
        let (warnings, records) = collect(&[input(
            "Swings!",
            &tag,
            &title,
            &[],
            Some("abby"),
            true,
            Some("love.md"),
        )]);
        assert_eq!(
            warnings,
            ["Swings!: #offclock is deprecated in Google Calendar; write #off (or drop it: abby is already off the clock via love.md)"]
        );
        assert!(records[0].drop_suggested);
        assert!(!records[0].on_wins);
    }

    #[test]
    fn on_wins_copy_drops_the_redundant_alias() {
        let tag = ClockTag::Conflict {
            deprecated: vec!["#offclock".to_string()],
        };
        let title = ["#offclock".to_string()];
        let (warnings, records) =
            collect(&[input("Planning", &tag, &title, &[], None, false, None)]);
        assert_eq!(
            warnings,
            ["Planning: #offclock is deprecated in Google Calendar and #on wins over it; drop #offclock"]
        );
        assert!(records[0].drop_suggested);
        assert!(records[0].on_wins);
    }

    #[test]
    fn canonical_conflict_warns_without_deprecation_records() {
        let tag = ClockTag::Conflict {
            deprecated: Vec::new(),
        };
        let (warnings, records) =
            collect(&[input("Planning", &tag, &[], &[], None, false, None)]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("clock_tag_conflict"));
        assert!(warnings[0].contains("#on wins"));
        assert!(records.is_empty());
    }
}

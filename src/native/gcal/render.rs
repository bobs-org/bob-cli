//! Render Event Task and context rows exactly as `docs/gcal.md` specifies.
//!
//! Titles pass through the shared escaper plus a gcal pass that
//! neutralizes `🔁`, Tasks emoji signifiers, and priority emojis, and
//! escapes a leading `#` or list marker. Badges are title-free
//! URL-encoded links; `[type:: MEET]` is added only when the PT13
//! suggestion for the rooted line is not MEET.

use std::path::Path;

use crate::native::{
    clock_tags::{ClockKind, ClockTag},
    config as bob_config, env as bob_env,
    event_task::EventBuffer,
    pomodoro_homes::{HomeResolver, HomeSource},
    pomodoro_themes, task_text_escape,
};

/// Input for one Event Task row. The title is already route-stripped;
/// the block id is minted by the plan and appended last.
pub(crate) struct TaskRowInput<'a> {
    pub time_label: &'a str,
    pub title: &'a str,
    pub html_link: Option<&'a str>,
    pub join_link: Option<&'a str>,
    pub location: Option<&'a str>,
    pub root: Option<&'a str>,
    pub needs_type: bool,
    pub buffer: EventBuffer,
    pub clock_tag: ClockTag,
    /// Task checkbox: `*` (Next), `/` (In Progress), or ` ` (Ready).
    pub status: char,
}

/// Input for one context row.
pub(crate) struct ContextRowInput<'a> {
    pub time_label: &'a str,
    pub title: &'a str,
    pub html_link: Option<&'a str>,
    pub qualifier: Option<&'a str>,
}

/// Render `- [status] #task **HHMM-HHMM** title badges [#off/#on]
/// [buffer::] [root::] [type::] ^id`. Google-owned badges, off-clock tag,
/// and buffer precede Bryan's root and type fields.
pub(crate) fn render_task_row(
    input: &TaskRowInput<'_>,
    block_id: &str,
) -> String {
    let status = input.status;
    let mut row = format!(
        "- [{status}] #task **{}** {}",
        input.time_label,
        escape_title(input.title),
    );
    row.push_str(&badges(input.html_link, input.join_link, input.location));
    append_clock_tag(&mut row, &input.clock_tag);
    if let Some(field) = input.buffer.field() {
        row.push_str(&format!(" {field}"));
    }
    row = set_row_metadata(&row, input.root, input.needs_type);
    format!("{row} ^{block_id}")
}

/// Set `[root::]` (and add `[type:: MEET]` when asked) on a gcal Event
/// Task row without disturbing the Google-owned run: an existing
/// `[root::]` is replaced in place, otherwise the root is inserted after
/// the badge run (badges, explicit clock tag, `[buffer::]`) and before
/// `[type::]` / `^id`; `[type:: MEET]` goes after the root. Tokens Bryan
/// placed (his own fields, tags, and the block id) keep their text and
/// order. Falls back to the shared setters when the row does not parse.
pub(crate) fn set_row_metadata(
    line: &str,
    root: Option<&str>,
    needs_type: bool,
) -> String {
    let Some(mut spans) = super::lifecycle::split_row_spans(line) else {
        return legacy_set_metadata(line, root, needs_type);
    };
    if !spans.task {
        return legacy_set_metadata(line, root, needs_type);
    }
    let mut suffix = std::mem::replace(&mut spans.suffix, String::new());
    strip_bracket_field(&mut suffix, "root");
    let has_type = suffix.contains("[type::") || suffix.contains("(type::");
    let mut middle = String::new();
    if let Some(name) = root {
        middle.push_str(&format!(" [root:: {name}]"));
    }
    if needs_type && !has_type {
        middle.push_str(" [type:: MEET]");
    }
    spans.suffix = if suffix.is_empty() || suffix.starts_with([' ', '\t']) {
        format!("{middle}{suffix}")
    } else {
        format!("{middle} {suffix}")
    };
    spans.rebuild()
}

fn legacy_set_metadata(
    line: &str,
    root: Option<&str>,
    needs_type: bool,
) -> String {
    let mut out = line.to_string();
    if needs_type
        && matches!(
            crate::native::task_theme::parse_task_theme(&out),
            crate::native::task_theme::TaskThemeState::None
        )
    {
        out = crate::native::task_theme::set_task_theme(&out, "MEET");
    }
    if let Some(name) = root {
        out = crate::native::task_root::set_task_root(&out, name);
    }
    out
}

/// Remove one `[key:: …]` field (plus one adjacent space) from `suffix`.
fn strip_bracket_field(suffix: &mut String, key: &str) {
    let open = format!("[{key}::");
    let Some(start) = suffix.find(open.as_str()) else {
        return;
    };
    let Some(close) = suffix[start..].find(']') else {
        return;
    };
    let end = start + close + 1;
    // Drop one adjacent space so no double space (or glued token) is left.
    let (drop_start, drop_end) = if suffix[..start].ends_with([' ', '\t']) {
        (start - 1, end)
    } else if suffix[end..].starts_with([' ', '\t']) {
        (start, end + 1)
    } else {
        (start, end)
    };
    suffix.replace_range(drop_start..drop_end, "");
}

/// Render `- **all day** title [· qualifier] [📅](…)` (or the timed span
/// form for a timed Maybe or free event).
pub(crate) fn render_context_row(input: &ContextRowInput<'_>) -> String {
    let mut row =
        format!("- **{}** {}", input.time_label, escape_title(input.title),);
    if let Some(qualifier) = input.qualifier {
        row.push_str(" · *");
        row.push_str(qualifier);
        row.push('*');
    }
    if let Some(link) = input.html_link {
        row.push_str(&calendar_badge(link));
    }
    row
}

/// Escape a calendar title: the shared task-line escaper, then the gcal
/// pass (Tasks emoji, priority emojis, leading markers).
pub(crate) fn escape_title(title: &str) -> String {
    let escaped = task_text_escape::escape_task_text(title);
    let neutralized = neutralize_tasks_emoji(&escaped);
    escape_leading_title_marker(&neutralized)
}

/// Whether the PT13 suggestion for `line` (already carrying `[root::]`)
/// is something other than MEET, meaning `[type:: MEET]` must be
/// written. Falls back to writing the field when the catalog cannot
/// be read, so a missing catalog never drops attribution.
pub(crate) fn needs_meet_type<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    vault: &Path,
    daily_rel: &Path,
    line_with_root: &str,
) -> bool {
    let today = bob_env::current_datetime().date();
    let catalog = match pomodoro_themes::Catalog::load(vault, today, false) {
        Ok(catalog) => catalog,
        Err(_) => return true,
    };
    let plan = bob_config::load_plan_config(&bob_config::config_path())
        .unwrap_or_default();
    let result = pomodoro_themes::suggest::suggest_for_task(
        resolver,
        &catalog,
        &plan,
        daily_rel,
        line_with_root,
    );
    !result
        .suggestion
        .is_some_and(|suggestion| suggestion.theme.name == "MEET")
}

/// The badge suffix for one row (` 📅 …`, ` 📹 …`, ` 📍 …`, plus its
/// explicit ` #off`/` #on` tag and ` [buffer:: …]` for buffered
/// ones), shared by the row renderer and the journal span hashes.
pub(crate) fn badge_text(
    html_link: Option<&str>,
    join_link: Option<&str>,
    location: Option<&str>,
    clock_tag: Option<&ClockTag>,
    buffer: EventBuffer,
) -> String {
    let mut out = badges(html_link, join_link, location);
    if let Some(clock_tag) = clock_tag {
        append_clock_tag(&mut out, clock_tag);
    }
    if let Some(field) = buffer.field() {
        out.push_str(&format!(" {field}"));
    }
    out
}

fn append_clock_tag(out: &mut String, clock_tag: &ClockTag) {
    match clock_tag.kind() {
        ClockKind::None => {}
        ClockKind::Off => out.push_str(" #off"),
        ClockKind::On => out.push_str(" #on"),
    }
}

/// Badge links are deliberately title-free, because Obsidian's Jump to
/// link plugin (Ctrl+L) and other naive Markdown parsers fold a `"title"`
/// into the opened URL. Google then 400s on the corrupted `eid`.
fn badges(
    html_link: Option<&str>,
    join_link: Option<&str>,
    location: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some(link) = html_link {
        out.push_str(&calendar_badge(link));
    }
    if let Some(link) = join_link {
        out.push_str(&format!(" [📹]({})", escape_badge_url(link),));
    }
    // 📍 is for a non-URL location only: a URL location already feeds
    // the 📹 join link. `&` would split the Maps query, so it is encoded.
    if let Some(place) = location
        && !place.trim().is_empty()
        && !is_url(place)
    {
        out.push_str(&format!(" [📍]({})", maps_url(place),));
    }
    out
}

fn is_url(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

fn calendar_badge(html_link: &str) -> String {
    format!(" [📅]({})", escape_badge_url(html_link),)
}

/// Keep an already-encoded Google URL intact while protecting the
/// Markdown link: encode spaces and parentheses.
pub(super) fn escape_badge_url(url: &str) -> String {
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('`', "%60")
}

fn maps_url(location: &str) -> String {
    format!(
        "https://www.google.com/maps/search/?api=1&query={}",
        percent_encode_query(location.trim()),
    )
}

pub(crate) fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Insert a zero-width space after every Tasks-signifier or priority
/// emoji so the row never parses as recurring, dated, or prioritized.
/// The bytes stay visually identical; the emoji regexes in the Tasks
/// plugin and in `migrate_tasks` require the emoji to be followed by
/// whitespace or end-of-token text, which the joiner breaks.
fn neutralize_tasks_emoji(text: &str) -> String {
    const MARKERS: &[char] = &[
        '🔁', '⏫', '🔼', '🔽', '🔺', '⏬', '🛫', '➕', '⏳', '⌛', '📅', '📆',
        '🗓', '✅', '❌', '🏁', '🆔', '⛔',
    ];
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        out.push(ch);
        if MARKERS.contains(&ch) {
            out.push('\u{200b}');
        }
    }
    out
}

/// A title that starts with `#` would mint a real tag, and one that
/// starts with a list marker would confuse list parsing; backslash the
/// first character. The span precedes the title, so only `#` needs
/// this in practice.
fn escape_leading_title_marker(title: &str) -> String {
    match title.chars().next() {
        Some('#' | '-' | '+' | '*' | '>') => format!("\\{title}"),
        _ => title.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_input<'a>(
        title: &'a str,
        root: Option<&'a str>,
        needs_type: bool,
        status: char,
    ) -> TaskRowInput<'a> {
        TaskRowInput {
            time_label: "0800-0830",
            title,
            html_link: Some("https://www.google.com/calendar/event?eid=1"),
            join_link: None,
            location: None,
            root,
            needs_type,
            buffer: EventBuffer::default(),
            clock_tag: ClockTag::None,
            status,
        }
    }

    #[test]
    fn task_row_field_order_and_status() {
        let row = render_task_row(
            &task_input("Product interview", Some("job"), true, '*'),
            "product-interview",
        );
        assert_eq!(
            row,
            "- [*] #task **0800-0830** Product interview \
             [📅](https://www.google.com/calendar/event?eid=1) \
             [root:: job] [type:: MEET] ^product-interview",
        );
        let row = render_task_row(
            &task_input("Standup", Some("meet"), false, ' '),
            "standup",
        );
        assert!(
            row.starts_with("- [ ] #task **0800-0830** Standup "),
            "{row}",
        );
        assert!(!row.contains("[type::"), "{row}");
        assert!(row.ends_with(" ^standup"), "{row}");
    }

    #[test]
    fn off_clock_task_rows_render_the_canonical_tag_before_root() {
        let row = render_task_row(
            &TaskRowInput {
                clock_tag: ClockTag::Off {
                    deprecated: Vec::new(),
                },
                ..task_input("Lunch", Some("love"), true, ' ')
            },
            "lunch",
        );
        assert!(row.contains(" #off [root:: love] [type:: MEET]"), "{row}");
        assert!(!row.contains("#untracked"), "{row}");
    }

    #[test]
    fn on_and_conflicting_tags_render_as_canonical_on() {
        for clock_tag in [
            ClockTag::On,
            ClockTag::Conflict {
                deprecated: vec!["#offclock".to_string()],
            },
        ] {
            let row = render_task_row(
                &TaskRowInput {
                    clock_tag,
                    ..task_input("Planning", Some("love"), true, '*')
                },
                "planning",
            );
            assert!(row.contains(" #on [root:: love]"), "{row}");
            assert!(!row.contains("#offclock"), "{row}");
        }
    }

    #[test]
    fn context_row_qualifiers_and_badges() {
        let row = render_context_row(&ContextRowInput {
            time_label: "all day",
            title: "School holiday",
            html_link: Some("https://www.google.com/calendar/event?eid=2"),
            qualifier: None,
        });
        assert_eq!(
            row,
            "- **all day** School holiday \
             [📅](https://www.google.com/calendar/event?eid=2)",
        );
        let row = render_context_row(&ContextRowInput {
            time_label: "0915-1000",
            title: "Toddler class",
            html_link: None,
            qualifier: Some("maybe"),
        });
        assert_eq!(row, "- **0915-1000** Toddler class · *maybe*");
    }

    #[test]
    fn hostile_titles_stay_data() {
        assert_eq!(escape_title("Fix #task now"), "Fix \\#task now");
        assert_eq!(escape_title("ends with ^abc"), "ends with \\^abc");
        assert_eq!(escape_title("a %% b"), "a %&#37; b");
        assert_eq!(escape_title("a :: b"), "a \\:\\: b");
        assert_eq!(escape_title("#hashtag first"), "\\#hashtag first",);
        let review = escape_title("Weekly 🔁 review");
        assert!(review.contains("🔁\u{200b}"), "{review}");
        let due = escape_title("Party 📅 2026-10-30");
        assert!(due.contains("📅\u{200b}"), "{due}");
        let priority = escape_title("Urgent ⏫ now");
        assert!(priority.contains("⏫\u{200b}"), "{priority}");
        assert_eq!(escape_title("Dentist \"Main St\""), "Dentist \"Main St\"",);
    }

    #[test]
    fn calendar_badges_never_parse_as_due_dates() {
        // 📅 is the Obsidian Tasks due signifier, but only when a date
        // follows it. The badge link never carries a date, and a
        // date-like title is neutralized, so no Bob parser reads a due
        // date from a synced row.
        use crate::native::dataview::{parse_details, TaskFormat};
        let row = render_task_row(
            &TaskRowInput {
                time_label: "1830-1900",
                title: "Party",
                html_link: Some("https://www.google.com/calendar/event?eid=7"),
                join_link: None,
                location: None,
                root: Some("body"),
                needs_type: true,
                buffer: EventBuffer::default(),
                clock_tag: ClockTag::None,
                status: '*',
            },
            "party",
        );
        let body = row
            .trim_start_matches("- [*] #task ")
            .trim_end_matches(" ^party");
        for format in [TaskFormat::Dataview, TaskFormat::Emoji] {
            let details = parse_details(body, format);
            assert!(details.due.is_none(), "{format:?}: {row}");
        }
        // Control: in the Tasks emoji format, a bare 📅 date parses as
        // due; without the neutralizing joiner the title would too.
        let control = parse_details("Party 📅 2026-10-30", TaskFormat::Emoji);
        assert!(control.due.is_some());
        let escaped = escape_title("Party 📅 2026-10-30");
        let details = parse_details(&escaped, TaskFormat::Emoji);
        assert!(details.due.is_none(), "{escaped}");
    }

    #[test]
    fn badge_urls_and_maps_links_are_encoded() {
        let row = render_task_row(
            &TaskRowInput {
                join_link: Some("https://teams.microsoft.com/l/meet (1)"),
                location: Some("Main St Dental"),
                ..task_input("Dentist", Some("body"), true, '*')
            },
            "dentist",
        );
        assert!(
            row.contains("[📹](https://teams.microsoft.com/l/meet%20%281%29"),
            "{row}",
        );
        assert!(
            row.contains(
                "[📍](https://www.google.com/maps/search/?api=1&query=Main%20St%20Dental",
            ),
            "{row}",
        );
    }

    #[test]
    fn badges_render_title_free() {
        let html = "https://www.google.com/calendar/event?eid=1";
        let join = "https://meet.google.com/abc";
        let maps =
            "https://www.google.com/maps/search/?api=1&query=Main%20St%20Dental";
        let row = render_task_row(
            &TaskRowInput {
                html_link: Some(html),
                join_link: Some(join),
                location: Some("Main St Dental"),
                root: Some("body"),
                needs_type: false,
                buffer: EventBuffer::default(),
                clock_tag: ClockTag::None,
                time_label: "0800-0830",
                title: "Dentist",
                status: '*',
            },
            "dentist",
        );
        assert!(
            row.contains(&format!("[📅]({html}) [📹]({join}) [📍]({maps})")),
            "{row}",
        );
        assert!(!row.contains('"'), "{row}");
        let context = render_context_row(&ContextRowInput {
            time_label: "all day",
            title: "Holiday",
            html_link: Some(html),
            qualifier: None,
        });
        assert_eq!(context, format!("- **all day** Holiday [📅]({html})"));
        assert!(!context.contains('"'), "{context}");
    }

    #[test]
    fn buffered_row_renders_buffer_between_badges_and_root() {
        let buffer = EventBuffer::parse("-30m +15m").expect("buffer");
        let row = render_task_row(
            &TaskRowInput {
                clock_tag: ClockTag::Off {
                    deprecated: Vec::new(),
                },
                buffer,
                ..task_input("Lunch with Kelly", Some("love"), true, '*')
            },
            "lunch-with-kelly",
        );
        assert_eq!(
            row,
            "- [*] #task **0800-0830** Lunch with Kelly \
             [📅](https://www.google.com/calendar/event?eid=1) #off \
             [buffer:: -30m +15m] [root:: love] [type:: MEET] \
             ^lunch-with-kelly",
            "{row}"
        );
        // The rendered badges span matches badge_text, so the journal
        // hashes agree with the note.
        let spans = crate::native::gcal::lifecycle::split_row_spans(&row)
            .expect("row parses");
        assert_eq!(
            spans.badges,
            badge_text(
                Some("https://www.google.com/calendar/event?eid=1"),
                None,
                None,
                Some(&ClockTag::Off {
                    deprecated: Vec::new(),
                }),
                buffer
            ),
            "{row}"
        );
    }

    #[test]
    fn buffer_sides_render_only_when_set() {
        let lead_in = EventBuffer::parse("-30").expect("lead-in");
        let row = render_task_row(
            &TaskRowInput {
                buffer: lead_in,
                ..task_input("Standup", Some("meet"), false, ' ')
            },
            "standup",
        );
        assert!(row.contains(" [buffer:: -30m] [root:: meet]"), "{row}");
        assert!(!row.contains("+"), "{row}");

        let plain = render_task_row(
            &task_input("Standup", Some("meet"), false, ' '),
            "standup",
        );
        assert!(!plain.contains("[buffer::"), "{plain}");
    }

    #[test]
    fn row_metadata_reroute_keeps_the_google_run_intact() {
        let buffer = EventBuffer::parse("-30m +15m").expect("buffer");
        let before = render_task_row(
            &TaskRowInput {
                clock_tag: ClockTag::Off {
                    deprecated: Vec::new(),
                },
                buffer,
                ..task_input("Lunch", Some("love"), false, '*')
            },
            "lunch",
        );
        let rerouted = set_row_metadata(&before, Some("job"), false);
        assert!(
            rerouted.contains(" #off [buffer:: -30m +15m] [root:: job] ^lunch"),
            "{rerouted}"
        );
        // The badges span (what the journal hashes) is unchanged, so the
        // next sync reports no badges edit.
        let badges_of = |line: &str| {
            crate::native::gcal::lifecycle::split_row_spans(line)
                .expect("row parses")
                .badges
        };
        assert_eq!(badges_of(&rerouted), badges_of(&before));
    }
}

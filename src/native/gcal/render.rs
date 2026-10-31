//! Render Event Task and context rows exactly as `docs/gcal.md` specifies.
//!
//! Titles pass through the shared escaper plus a gcal pass that
//! neutralizes `🔁`, Tasks emoji signifiers, and priority emojis, and
//! escapes a leading `#` or list marker. Badges carry URL-encoded links
//! with quoted titles; `[type:: MEET]` is added only when the PT13
//! suggestion for the rooted line is not MEET.

use std::path::Path;

use crate::native::{
    config as bob_config, env as bob_env,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
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

/// Render `- [status] #task **HHMM-HHMM** title badges [root::] [type::] ^id`.
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
    if input.needs_type {
        // Theme first, then root: each setter inserts at the start of
        // the trailing metadata run, so the root lands left of the
        // type field (`… [root:: job] [type:: MEET] ^id`).
        row = crate::native::task_theme::set_task_theme(&row, "MEET");
    }
    if let Some(root) = input.root {
        row = crate::native::task_root::set_task_root(&row, root);
    }
    format!("{row} ^{block_id}")
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
pub(crate) fn needs_meet_type(
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
    let mut resolver = HomeResolver::new(DiskHomeSource::new(vault));
    let result = pomodoro_themes::suggest::suggest_for_task(
        &mut resolver,
        &catalog,
        &plan,
        daily_rel,
        line_with_root,
    );
    !result
        .suggestion
        .is_some_and(|suggestion| suggestion.theme.name == "MEET")
}

/// The badge suffix for one row (` 📅 …`, ` 📹 …`, ` 📍 …`), shared
/// by the row renderer and the journal span hashes.
pub(crate) fn badge_text(
    html_link: Option<&str>,
    join_link: Option<&str>,
    location: Option<&str>,
) -> String {
    badges(html_link, join_link, location)
}

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
        out.push_str(&format!(
            " [📹]({} \"Join call\")",
            escape_badge_url(link),
        ));
    }
    // 📍 is for a non-URL location only: a URL location already feeds
    // the 📹 join link. `&` would split the Maps query, so it is encoded.
    if let Some(place) = location
        && !place.trim().is_empty()
        && !is_url(place)
    {
        out.push_str(&format!(
            " [📍]({} \"{}\")",
            maps_url(place),
            escape_link_title(place),
        ));
    }
    out
}

fn is_url(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

fn calendar_badge(html_link: &str) -> String {
    format!(
        " [📅]({} \"Open in Google Calendar\")",
        escape_badge_url(html_link),
    )
}

/// Keep an already-encoded Google URL intact while protecting the
/// Markdown link: encode spaces and parentheses.
fn escape_badge_url(url: &str) -> String {
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// Escape a Markdown link title: `"` would end the quoted title.
fn escape_link_title(title: &str) -> String {
    escape_title(title).replace('"', "&quot;")
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
             [📅](https://www.google.com/calendar/event?eid=1 \
             \"Open in Google Calendar\") [root:: job] [type:: MEET] \
             ^product-interview",
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
             [📅](https://www.google.com/calendar/event?eid=2 \
             \"Open in Google Calendar\")",
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
}

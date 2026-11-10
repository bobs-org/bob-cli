//! Sanitized Event Task description bullets.
//!
//! Buffer reasons lead the block: at most one `↤ *Nm before*` and one
//! `↦ *Nm after*` bullet, then uncapped private-note prose, then the
//! capped copied description lines.

use crate::native::{
    capture::parse_managed_task_log_marker, capture_drawers,
    event_task::BufferPart, gcal::journal, task_dependencies, task_text_escape,
};

use super::super::render::escape_badge_url as escape_destination;
use super::{BufferReason, EventDescription, Inline, LineMarker};

/// Glyph that reaches back from the meeting's start.
const BEFORE_GLYPH: char = '↤';
/// Glyph that reaches forward from the meeting's end.
const AFTER_GLYPH: char = '↦';

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bullet {
    pub depth: u8,
    /// A list marker and body, with no indentation.
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DescriptionBullets {
    pub bullets: Vec<Bullet>,
}

impl DescriptionBullets {
    /// Materialize bullets at the row's first-child indentation. Depth 2
    /// adds one indentation unit to that prefix.
    pub(crate) fn materialize(
        &self,
        depth1_indent: &str,
        unit: &str,
    ) -> Vec<String> {
        self.bullets
            .iter()
            .map(|bullet| {
                let indent = if bullet.depth > 1 {
                    format!("{depth1_indent}{unit}")
                } else {
                    depth1_indent.to_string()
                };
                format!("{indent}{}", bullet.body)
            })
            .collect()
    }
}

/// Render the parsed description into inert, capped list-item bodies.
///
/// Reason bullets lead in timeline order (before, then after), then
/// private-note prose, then copied Google lines. The `cap` applies to
/// copied description lines only, so reasons and note prose are never
/// truncated and never count toward the overflow math.
pub(crate) fn render_bullets(
    description: &EventDescription,
    cap: usize,
    html_link: Option<&str>,
) -> DescriptionBullets {
    let mut bullets = Vec::new();
    if !description.buffer_reasons.is_empty() {
        for (part, reason) in description.buffer_reasons.iter() {
            bullets.push(render_reason(part, reason));
        }
    }
    bullets.extend(description.note_lines.iter().map(render_line));
    if description.oversized {
        let calendar = calendar_link("Google Calendar", html_link);
        bullets.push(Bullet {
            depth: 1,
            body: format!(
                "- *Description too long to copy — open it in {calendar}*"
            ),
        });
        return DescriptionBullets { bullets };
    }

    // The config layer requires one content line plus its overflow line.
    let cap = cap.max(2);
    let mut copied = description
        .lines
        .iter()
        .map(render_line)
        .collect::<Vec<_>>();
    if copied.len() > cap {
        let hidden = copied.len().saturating_sub(cap - 1);
        copied.truncate(cap - 1);
        let calendar = calendar_link("Google Calendar", html_link);
        copied.push(Bullet {
            depth: 1,
            body: format!("- *…{hidden} more lines in {calendar}*"),
        });
    }
    bullets.extend(copied);
    DescriptionBullets { bullets }
}

fn render_reason(part: BufferPart, reason: &BufferReason) -> Bullet {
    let (glyph, side) = match part {
        BufferPart::Before => (BEFORE_GLYPH, "before"),
        BufferPart::After => (AFTER_GLYPH, "after"),
    };
    let rendered = render_inlines(&reason.inlines, false);
    Bullet {
        depth: 1,
        body: format!("- {glyph} *{}m {side}* · {rendered}", reason.minutes),
    }
}

/// Short SHA-256 over the exact depth/body pairs written to the note.
pub(crate) fn bullets_hash<'a>(
    pairs: impl IntoIterator<Item = (u8, &'a str)>,
) -> String {
    let mut span = String::new();
    for (depth, body) in pairs {
        use std::fmt::Write as _;
        let _ = writeln!(span, "{depth}\t{body}");
    }
    journal::span_hash(&span)
}

fn render_line(line: &super::DescriptionLine) -> Bullet {
    let body_text = plain_text(&line.inlines);
    let rendered = render_inlines(&line.inlines, true);
    let marker = match line.marker {
        LineMarker::Bullet => "-".to_string(),
        LineMarker::Ordered(number) => format!("{number}."),
    };
    let normal = format!("{marker} {rendered}");
    let managed = is_managed_line(&normal, &body_text);
    let body = if managed {
        format!("{marker} *{}*", escape_note_body(&body_text))
    } else if capture_drawers::is_drawer_title(body_text.trim()) {
        format!("{marker} **{}**", escape_note_body(&body_text))
    } else {
        normal
    };
    Bullet {
        depth: line.depth.clamp(1, 2),
        body,
    }
}

fn is_managed_line(rendered: &str, plain: &str) -> bool {
    parse_managed_task_log_marker(rendered).is_some()
        || task_dependencies::is_managed_link_line(rendered)
        || task_dependencies::blocks::is_cancel_log(rendered)
        || capture_drawers::is_reserved_drawer_key(
            &capture_drawers::drawer_key(plain.trim()),
        )
}

fn render_inlines(inlines: &[Inline], first_starts_body: bool) -> String {
    let mut rendered = String::new();
    for (index, inline) in inlines.iter().enumerate() {
        let starts_body =
            first_starts_body && index == 0 && rendered.is_empty();
        match inline {
            Inline::Text { text, bold, italic } => rendered
                .push_str(&render_text(text, *bold, *italic, starts_body)),
            Inline::Link { text, url } => {
                rendered.push_str(&render_link(text, url, starts_body));
            }
        }
    }
    rendered
}

fn render_text(
    text: &str,
    bold: bool,
    italic: bool,
    starts_body: bool,
) -> String {
    let normalized = one_line(text);
    if !bold && !italic {
        return escape_text(&normalized, starts_body);
    }
    let leading_len = normalized.len() - normalized.trim_start().len();
    let leading = &normalized[..leading_len];
    let rest = &normalized[leading_len..];
    let trailing_len = rest.len() - rest.trim_end().len();
    let (content, trailing) = rest.split_at(rest.len() - trailing_len);
    if content.is_empty() {
        return format!("{leading}{trailing}");
    }
    let escaped = escape_text(content, starts_body);
    let mark = match (bold, italic) {
        (true, true) => "***",
        (true, false) => "**",
        (false, true) => "*",
        (false, false) => "",
    };
    format!("{leading}{mark}{escaped}{mark}{trailing}")
}

fn render_link(text: &str, raw_url: &str, starts_body: bool) -> String {
    let text = one_line(text);
    let Some(url) = allowed_url(raw_url) else {
        return escape_text(&text, starts_body);
    };
    let parsed = match url::Url::parse(&url) {
        Ok(parsed) => parsed,
        Err(_) => return escape_text(&text, starts_body),
    };
    let scheme = parsed.scheme();
    let shown_url = text == raw_url || text == url;

    if shown_url {
        if matches!(scheme, "mailto" | "tel") {
            let address =
                url.strip_prefix(&format!("{scheme}:")).unwrap_or(&url);
            let target = escape_destination(&url);
            return format!("[{}]({target})", escape_link_text(address));
        }
        if is_short_bare_url(&url) {
            return escape_destination(&url);
        }
        let Some(host) = parsed.host_str() else {
            return escape_text(&text, starts_body);
        };
        let label = host.strip_prefix("www.").unwrap_or(host);
        return format!("[{label}]({})", escape_destination(&url));
    }

    format!(
        "[{}]({})",
        escape_link_text(&text),
        escape_destination(&url)
    )
}

fn allowed_url(raw: &str) -> Option<String> {
    let mut candidate = one_line(raw.trim());
    for _ in 0..2 {
        let parsed = url::Url::parse(&candidate).ok()?;
        if parsed.scheme() == "https"
            && parsed.host_str() == Some("www.google.com")
            && parsed.path() == "/url"
            && let Some((_, target)) =
                parsed.query_pairs().find(|(key, _)| key == "q")
        {
            candidate = target.into_owned();
            continue;
        }
        return matches!(parsed.scheme(), "http" | "https" | "mailto" | "tel")
            .then_some(candidate);
    }
    None
}

fn is_short_bare_url(url: &str) -> bool {
    url.chars().count() <= 40
        && !url.chars().any(char::is_whitespace)
        && !url
            .chars()
            .any(|ch| matches!(ch, '<' | '>' | '[' | ']' | '`' | '"'))
        && !url.contains("::")
}

fn escape_link_text(text: &str) -> String {
    neutralize_tasks_emoji(
        &task_text_escape::escape_note_inline(text).replace(']', "\\]"),
    )
}

fn escape_note_body(text: &str) -> String {
    neutralize_tasks_emoji(&task_text_escape::escape_note_text(&one_line(text)))
}

fn escape_text(text: &str, starts_body: bool) -> String {
    let escaped = if starts_body {
        task_text_escape::escape_note_text(text)
    } else {
        task_text_escape::escape_note_inline(text)
    };
    neutralize_tasks_emoji(&escaped)
}

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

fn one_line(text: &str) -> String {
    text.chars()
        .map(|ch| if matches!(ch, '\r' | '\n') { ' ' } else { ch })
        .collect()
}

fn plain_text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { text, .. } | Inline::Link { text, .. } => {
                text.as_str()
            }
        })
        .collect::<String>()
}

fn calendar_link(label: &str, html_link: Option<&str>) -> String {
    match html_link.and_then(allowed_url) {
        Some(url) => format!("[{label}]({})", escape_destination(&url)),
        None => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::native::{
        capture_pomodoros, event_task, gcal::lifecycle, task_status_hooks,
    };

    fn parse(raw: &str) -> EventDescription {
        super::super::parse_description(
            raw,
            &super::super::DescriptionContext {
                join_link: None,
                location: None,
            },
        )
    }

    fn expected_bodies(value: &Value) -> Vec<(u8, String)> {
        value["bullets"]
            .as_array()
            .expect("bullets")
            .iter()
            .map(|bullet| {
                (
                    u8::try_from(bullet["depth"].as_u64().expect("depth"))
                        .expect("depth fits"),
                    bullet["body"].as_str().expect("body").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn bullet_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/gcal_description/bullets.json"
        ))
        .expect("bullet vectors JSON");
        for case in vectors["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("id");
            let raw = case["input"].as_str().expect("input");
            let parsed = parse(raw);
            let cap = case["cap"].as_u64().unwrap_or(8) as usize;
            let html_link = case["htmlLink"].as_str();
            let rendered = render_bullets(&parsed, cap, html_link);
            let expected = expected_bodies(case);
            let got = rendered
                .bullets
                .iter()
                .map(|bullet| (bullet.depth, bullet.body.clone()))
                .collect::<Vec<_>>();
            assert_eq!(got, expected, "{id}");
            let repeated = render_bullets(&parsed, cap, html_link);
            assert_eq!(rendered, repeated, "{id} deterministic output");
            assert_eq!(
                bullets_hash(
                    rendered
                        .bullets
                        .iter()
                        .map(|bullet| { (bullet.depth, bullet.body.as_str()) })
                ),
                bullets_hash(
                    repeated
                        .bullets
                        .iter()
                        .map(|bullet| { (bullet.depth, bullet.body.as_str()) })
                ),
                "{id} deterministic hash"
            );
        }
    }

    #[test]
    fn oversize_input_uses_one_calendar_fallback() {
        let parsed = parse(&"x".repeat(65_537));
        assert!(parsed.oversized);
        assert_eq!(
            render_bullets(
                &parsed,
                8,
                Some("https://calendar.google.com/event?eid=abc")
            )
            .bullets,
            [Bullet {
                depth: 1,
                body: "- *Description too long to copy — open it in [Google Calendar](https://calendar.google.com/event?eid=abc)*".to_string(),
            }]
        );
    }

    #[test]
    fn note_prose_is_exempt_from_the_cap() {
        let mut parsed = parse("one\ntwo\nthree");
        parsed.note_lines = parse("Note snacks").lines;
        let rendered = render_bullets(&parsed, 2, None);
        let bodies: Vec<&str> = rendered
            .bullets
            .iter()
            .map(|bullet| bullet.body.as_str())
            .collect();
        assert_eq!(
            bodies,
            [
                "- Note snacks",
                "- one",
                "- *…2 more lines in Google Calendar*",
            ]
        );
    }

    #[test]
    fn oversize_input_keeps_reasons_above_the_fallback() {
        let raw = format!("{}\n-30 Drive home", "x".repeat(65_537));
        let parsed = parse(&raw);
        assert!(parsed.oversized);
        assert_eq!(
            render_bullets(&parsed, 8, None).bullets,
            [
                Bullet {
                    depth: 1,
                    body: "- ↤ *30m before* · Drive home".to_string(),
                },
                Bullet {
                    depth: 1,
                    body: "- *Description too long to copy — open it in Google Calendar*"
                        .to_string(),
                },
            ]
        );
    }

    #[test]
    fn markdown_destination_encoding_covers_delimiters() {
        assert_eq!(
            escape_destination("https://example.test/a b(c)<d>`"),
            "https://example.test/a%20b%28c%29%3Cd%3E%60"
        );
    }

    #[test]
    fn hash_and_materialization_follow_depth_and_body() {
        let bullets = DescriptionBullets {
            bullets: vec![
                Bullet {
                    depth: 1,
                    body: "- top".to_string(),
                },
                Bullet {
                    depth: 2,
                    body: "1. nested".to_string(),
                },
            ],
        };
        assert_eq!(
            bullets.materialize("\t", "\t"),
            ["\t- top", "\t\t1. nested"]
        );
        assert_eq!(
            bullets_hash(
                bullets
                    .bullets
                    .iter()
                    .map(|bullet| { (bullet.depth, bullet.body.as_str()) })
            ),
            journal::span_hash("1\t- top\n2\t1. nested\n")
        );
        assert_eq!(bullets_hash(std::iter::empty()), journal::span_hash(""));
    }

    #[test]
    fn hostile_bullets_are_isolated_from_bob_scanners() {
        let hostile = [
            "- [ ] x",
            "[1] ref",
            "#task",
            "x ^abc-1",
            "%%c%%",
            "a:: b",
            "[[#^x]]",
            "![[e]]",
            "[^1]",
            "[y]: https://x",
            "**WORK LOG**",
            "<b>Work log</b>",
            "❌ **CANCEL LOG**",
            "DEPENDS ON",
            "LINKS",
            "AGENDA",
            "TBD",
            "🍅 [[#^swings]]",
            "`$= dv.el(\"b\",1)`",
            "<% tp.date.now() %>",
            "**0900-1000** standup",
            "*↻ synced 14:05 · x*",
            "#offclock",
            "(type:: MEET)",
            "Due 📅 2026-11-03",
        ];
        for child_indent in ["  ", "\t"] {
            let parsed = parse(&hostile.join("<br>"));
            let rendered = render_bullets(&parsed, 64, None);
            let bullets = rendered.materialize(child_indent, child_indent);
            assert!(bullets.iter().all(|line| !line.contains('\n')));
            let note = format!(
                    "## Events\n- [ ] #task **0900-1000** Standup ^event\n{}\n\n## Pomodoros\n- [ ] (09:00) — Focus\n",
                    bullets.join("\n")
                );

            let baseline = "## Events\n- [ ] #task **0900-1000** Standup ^event\n\n## Pomodoros\n- [ ] (09:00) — Focus\n".to_string();
            let before_pomodoros = capture_pomodoros::scan(&baseline);
            let after_pomodoros = capture_pomodoros::scan(&note);
            let projection = |scan: &capture_pomodoros::PomodoroScan| {
                (
                    scan.has_section,
                    scan.entries
                        .iter()
                        .map(|entry| {
                            (
                                entry.state,
                                entry.status_symbol,
                                entry.name.clone(),
                                entry.slug.clone(),
                                entry.selectable,
                                entry.time_range.clone(),
                                entry.placeholder,
                                entry.is_current,
                                entry.child_count,
                            )
                        })
                        .collect::<Vec<_>>(),
                    scan.warnings.clone(),
                )
            };
            assert_eq!(
                projection(&before_pomodoros),
                projection(&after_pomodoros),
                "Pomodoro scan (line offsets are allowed)"
            );
            assert_eq!(
                task_status_hooks::reconcile_for_scanner_isolation(&baseline),
                baseline,
                "task-status hooks reconcile baseline"
            );
            assert_eq!(
                task_status_hooks::reconcile_for_scanner_isolation(&note),
                note,
                "task-status hooks reconcile with bullets"
            );
            let baseline_lines =
                baseline.lines().map(str::to_string).collect::<Vec<_>>();
            let note_lines =
                note.lines().map(str::to_string).collect::<Vec<_>>();
            assert_eq!(
                super::super::super::plan::collect_block_ids(&baseline_lines),
                super::super::super::plan::collect_block_ids(&note_lines),
                "gcal block-id scan"
            );

            let before_spans = baseline
                .lines()
                .filter_map(event_task::parse_event_span)
                .collect::<Vec<_>>();
            let after_spans = note
                .lines()
                .filter_map(event_task::parse_event_span)
                .collect::<Vec<_>>();
            assert_eq!(before_spans, after_spans, "event span scan");

            assert_eq!(
                lifecycle::row_untouched(&baseline, 1, "event"),
                lifecycle::row_untouched(&note, 1, "event"),
                "row untouched"
            );
            let row_line = note
                .lines()
                .find(|line| line.contains("Standup ^event"))
                .expect("event task");
            assert!(event_task::parse_event_span(row_line).is_some());

            for line in bullets {
                assert!(
                    parse_managed_task_log_marker(&line).is_none(),
                    "managed log: {line}"
                );
                assert!(
                    !task_dependencies::is_managed_link_line(&line),
                    "managed relation: {line}"
                );
                assert!(
                    !task_dependencies::blocks::is_cancel_log(&line),
                    "cancel log: {line}"
                );
                let body = line
                    .trim_start_matches([' ', '\t'])
                    .strip_prefix("- ")
                    .or_else(|| {
                        line.trim_start_matches([' ', '\t'])
                            .split_once(". ")
                            .map(|(_, body)| body)
                    })
                    .unwrap_or(&line);
                assert!(
                    !capture_drawers::is_drawer_title(body),
                    "drawer: {line}"
                );
                assert!(
                    task_dependencies::block_link_spans(&line).is_empty(),
                    "block link/mention: {line}"
                );
            }

            let task_format = crate::native::dataview::TaskFormat::Emoji;
            for bullet in
                rendered.bullets.iter().filter(|b| b.body.contains("Due"))
            {
                let details = crate::native::dataview::parse_details(
                    bullet.body.trim_start_matches("- "),
                    task_format,
                );
                assert!(
                    details.due.is_none(),
                    "Tasks due parser: {}",
                    bullet.body
                );
            }
        }
        for reason_text in [
            "x #task",
            "x [[#^x]]",
            "x a:: b",
            "x ^abc-1",
            "Bring 🍅 [[#^swings]]",
            "Due 📅 2026-11-03",
            "TBD",
            "WORK LOG",
            "x (type:: MEET)",
        ] {
            let raw = format!("-30 {reason_text}\n+15 {reason_text}");
            let parsed = parse(&raw);
            assert_eq!(
                parsed.directives.buffer.before_min, 30,
                "reason {reason_text}"
            );
            assert_eq!(
                parsed.directives.buffer.after_min, 15,
                "reason {reason_text}"
            );
            assert_eq!(parsed.buffer_reasons.count(), 2, "{reason_text}");
            let rendered = render_bullets(&parsed, 64, None);
            assert_eq!(rendered.bullets.len(), 2, "{reason_text}");
            for child_indent in ["  ", "\t"] {
                let bullets = rendered.materialize(child_indent, child_indent);
                assert!(bullets.iter().all(|line| !line.contains('\n')));
                let note = format!(
                    "## Events\n- [ ] #task **0900-1000** Standup ^event\n{}\n\n## Pomodoros\n- [ ] (09:00) — Focus\n",
                    bullets.join("\n")
                );
                let baseline = "## Events\n- [ ] #task **0900-1000** Standup ^event\n\n## Pomodoros\n- [ ] (09:00) — Focus\n".to_string();
                let before_pomodoros = capture_pomodoros::scan(&baseline);
                let after_pomodoros = capture_pomodoros::scan(&note);
                let projection = |scan: &capture_pomodoros::PomodoroScan| {
                    (
                        scan.has_section,
                        scan.entries
                            .iter()
                            .map(|entry| {
                                (
                                    entry.state,
                                    entry.status_symbol,
                                    entry.name.clone(),
                                    entry.slug.clone(),
                                    entry.selectable,
                                    entry.time_range.clone(),
                                    entry.placeholder,
                                    entry.is_current,
                                    entry.child_count,
                                )
                            })
                            .collect::<Vec<_>>(),
                        scan.warnings.clone(),
                    )
                };
                assert_eq!(
                    projection(&before_pomodoros),
                    projection(&after_pomodoros),
                    "Pomodoro scan with reason {reason_text}"
                );
                assert_eq!(
                    task_status_hooks::reconcile_for_scanner_isolation(
                        &baseline
                    ),
                    baseline,
                    "task-status hooks reconcile baseline"
                );
                assert_eq!(
                    task_status_hooks::reconcile_for_scanner_isolation(&note),
                    note,
                    "task-status hooks reconcile with reason {reason_text}"
                );
                let baseline_lines =
                    baseline.lines().map(str::to_string).collect::<Vec<_>>();
                let note_lines =
                    note.lines().map(str::to_string).collect::<Vec<_>>();
                assert_eq!(
                    super::super::super::plan::collect_block_ids(
                        &baseline_lines
                    ),
                    super::super::super::plan::collect_block_ids(&note_lines),
                    "gcal block-id scan {reason_text}"
                );
                let before_spans = baseline
                    .lines()
                    .filter_map(event_task::parse_event_span)
                    .collect::<Vec<_>>();
                let after_spans = note
                    .lines()
                    .filter_map(event_task::parse_event_span)
                    .collect::<Vec<_>>();
                assert_eq!(
                    before_spans, after_spans,
                    "event span scan {reason_text}"
                );
                assert_eq!(
                    lifecycle::row_untouched(&baseline, 1, "event"),
                    lifecycle::row_untouched(&note, 1, "event"),
                    "row untouched {reason_text}"
                );
                for line in &bullets {
                    assert!(
                        parse_managed_task_log_marker(line).is_none(),
                        "managed log: {line}"
                    );
                    assert!(
                        !task_dependencies::is_managed_link_line(line),
                        "managed relation: {line}"
                    );
                    assert!(
                        !task_dependencies::blocks::is_cancel_log(line),
                        "cancel log: {line}"
                    );
                    let body = line
                        .trim_start_matches([' ', '\t'])
                        .strip_prefix("- ")
                        .or_else(|| {
                            line.trim_start_matches([' ', '\t'])
                                .split_once(". ")
                                .map(|(_, body)| body)
                        })
                        .unwrap_or(line);
                    assert!(
                        !capture_drawers::is_drawer_title(body),
                        "drawer: {line}"
                    );
                    assert!(
                        task_dependencies::block_link_spans(line).is_empty(),
                        "block link/mention: {line}"
                    );
                }
                let task_format = crate::native::dataview::TaskFormat::Emoji;
                for bullet in
                    rendered.bullets.iter().filter(|b| b.body.contains("Due"))
                {
                    let details = crate::native::dataview::parse_details(
                        bullet.body.trim_start_matches("- "),
                        task_format,
                    );
                    assert!(
                        details.due.is_none(),
                        "Tasks due parser: {}",
                        bullet.body
                    );
                }
            }
        }
    }
}

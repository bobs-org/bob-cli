//! Event Task grammar and clock classification shared by capture and gcal.

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use super::{markdown, task_status_hooks::canonical_daily_date};

const MINUTES_PER_DAY: i32 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventSpan {
    pub start_min: u16,
    pub end_min: u16,
    pub crosses_midnight: bool,
}

impl EventSpan {
    pub(crate) fn text(self) -> String {
        format!(
            "{:02}{:02}-{:02}{:02}",
            self.start_min / 60,
            self.start_min % 60,
            self.end_min / 60,
            self.end_min % 60,
        )
    }

    pub(crate) fn end_on_timeline(self) -> i32 {
        i32::from(self.end_min)
            + if self.crosses_midnight {
                MINUTES_PER_DAY
            } else {
                0
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventTask {
    pub line_index: usize,
    pub span: EventSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventTaskClock {
    pub class: ClockClass,
    pub span: EventSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockClass {
    NotStarted,
    EndedBefore,
    EndedInside,
    Running,
}

impl ClockClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::EndedBefore => "ended_before",
            Self::EndedInside => "ended_inside",
            Self::Running => "running",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseClock {
    pub day_relative: PathBuf,
    pub session_start_min: u16,
    pub close_at_min: u16,
}

impl CloseClock {
    pub(crate) fn classify(self: &Self, span: EventSpan) -> ClockClass {
        classify(span, self.session_start_min, self.close_at_min)
    }
}

pub(crate) fn classify(
    span: EventSpan,
    session_start_min: u16,
    close_at_min: u16,
) -> ClockClass {
    let start = i32::from(span.start_min);
    let mut close = i32::from(close_at_min);
    if close < i32::from(session_start_min) {
        close += MINUTES_PER_DAY;
    }
    let end = span.end_on_timeline();
    let session_start = i32::from(session_start_min);

    if end <= session_start {
        ClockClass::EndedBefore
    } else if start >= close {
        ClockClass::NotStarted
    } else if end <= close {
        ClockClass::EndedInside
    } else {
        ClockClass::Running
    }
}

pub(crate) fn events_section_range(lines: &[&str]) -> Option<Range<usize>> {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut section_start = None;
    let mut fence = None;

    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
            continue;
        }
        if let Some(start) = section_start {
            if is_level_two_heading(line) {
                return Some(start..index);
            }
        } else if is_events_heading(line) {
            section_start = Some(index + 1);
        }
    }
    section_start.map(|start| start..lines.len())
}

fn is_events_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("##") else {
        return false;
    };
    if !rest.starts_with([' ', '\t']) {
        return false;
    }
    rest.trim() == "Events"
}

fn is_level_two_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("##") else {
        return false;
    };
    rest.starts_with([' ', '\t'])
}

pub(crate) fn parse_event_span(line: &str) -> Option<EventSpan> {
    let mut body = line.trim_start_matches([' ', '\t']).strip_prefix("- ")?;
    body = body.strip_prefix('[')?;
    let close = body.find(']')?;
    let status = body.get(..close)?;
    if status.chars().count() != 1 {
        return None;
    }
    body = body.get(close + 1..)?;
    body = body.trim_start_matches([' ', '\t']);
    if let Some(rest) = body.strip_prefix("#task") {
        body = rest.trim_start_matches([' ', '\t']);
    }
    let body = body.strip_prefix("**")?;
    let raw = body.get(..9)?;
    if raw.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    if body.get(9..)?.get(..2) != Some("**") {
        return None;
    }
    let after = body.get(11..)?;
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    if raw
        .bytes()
        .enumerate()
        .any(|(index, byte)| index != 4 && !byte.is_ascii_digit())
    {
        return None;
    }
    let start_min = parse_hhmm(&raw[..4])?;
    let end_min = parse_hhmm(&raw[5..9])?;
    Some(EventSpan {
        start_min,
        end_min,
        crosses_midnight: end_min < start_min,
    })
}

fn parse_hhmm(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = value[..2].parse::<u16>().ok()?;
    let minute = value[2..].parse::<u16>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

pub(crate) fn event_task_at(
    relative_path: &Path,
    contents: &str,
    line_index: usize,
) -> Option<EventTask> {
    canonical_daily_date(relative_path)?;
    let lines: Vec<&str> = contents.lines().collect();
    let section = events_section_range(&lines)?;
    if !section.contains(&line_index) || is_fenced_line(&lines, line_index) {
        return None;
    }
    Some(EventTask {
        line_index,
        span: parse_event_span(lines.get(line_index).copied()?)?,
    })
}

pub(crate) fn event_task_by_id(
    relative_path: &Path,
    contents: &str,
    block_id: &str,
) -> Option<EventTask> {
    canonical_daily_date(relative_path)?;
    let lines: Vec<&str> = contents.lines().collect();
    let section = events_section_range(&lines)?;
    let suffix = format!("^{block_id}");
    let mut found = None;
    for index in section {
        let line = lines.get(index)?.trim_end();
        let Some(before_id) = line.strip_suffix(&suffix) else {
            continue;
        };
        if before_id
            .chars()
            .last()
            .is_some_and(|ch| !ch.is_whitespace())
        {
            continue;
        }
        let Some(event) = event_task_at(relative_path, contents, index) else {
            continue;
        };
        if found.replace(event).is_some() {
            return None;
        }
    }
    found
}

fn is_fenced_line(lines: &[&str], line_index: usize) -> bool {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate().take(line_index + 1) {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
        }
    }
    fence.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn hhmm(value: &str) -> u16 {
        parse_hhmm(value).expect("valid HHMM")
    }

    #[test]
    fn parses_the_documented_event_task_grammar() {
        assert_eq!(
            parse_event_span("- [*] #task **0800-0830** Product interview"),
            Some(EventSpan {
                start_min: hhmm("0800"),
                end_min: hhmm("0830"),
                crosses_midnight: false,
            })
        );
        assert_eq!(
            parse_event_span("- [ ] **2330-0030** Hand-written"),
            Some(EventSpan {
                start_min: hhmm("2330"),
                end_min: hhmm("0030"),
                crosses_midnight: true,
            })
        );
        assert!(parse_event_span("- **0800-0830** Missing checkbox").is_none());
        assert!(parse_event_span("- [ ] #task **2460-2500** invalid").is_none());
        assert!(parse_event_span("- [ ] #task **0800-0830**word").is_none());
    }

    #[test]
    fn event_tasks_must_be_in_the_events_section_of_a_canonical_day() {
        let contents = concat!(
            "---\n",
            "title: Example\n",
            "---\n",
            "- [ ] #task **0700-0730** Outside ^outside\n",
            "## Events\n",
            "- [ ] #task **0800-0830** Inside ^inside\n",
            "```md\n",
            "## Pomodoros\n",
            "- [ ] #task **0900-0930** Fenced ^fenced\n",
            "```\n",
            "## Pomodoros\n",
            "- [ ] #task **1000-1030** Later ^later\n",
        );
        let day = Path::new("2026/20261029.md");
        assert!(event_task_at(day, contents, 5).is_some());
        assert!(event_task_at(day, contents, 3).is_none());
        assert!(event_task_at(day, contents, 8).is_none());
        assert!(event_task_at(day, contents, 11).is_none());
        assert!(event_task_by_id(day, contents, "fenced").is_none());
        assert!(event_task_at(Path::new("events.md"), contents, 5).is_none());
        assert_eq!(
            event_task_by_id(day, contents, "inside")
                .unwrap()
                .line_index,
            5
        );
    }

    #[test]
    fn classifier_matches_shared_clock_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/event_task_close/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["classify"].as_array().expect("classify cases") {
            let event = parse_event_span(&format!(
                "- [ ] **{}** event",
                case["span"].as_str().expect("span")
            ))
            .expect("valid event span");
            let actual = classify(
                event,
                hhmm(case["session_start"].as_str().expect("session start")),
                hhmm(case["close_at"].as_str().expect("close time")),
            );
            assert_eq!(
                actual.as_str(),
                case["class"].as_str().expect("expected class"),
                "{}",
                case["id"]
            );
        }
    }
}

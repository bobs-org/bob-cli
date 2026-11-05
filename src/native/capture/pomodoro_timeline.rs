//! Pure catalog and placement engine for today's past-Pomodoro timeline.
//!
//! The model stays self-contained so recording and completion share exactly
//! the same segment boundaries and labels.

use super::line_spans;
use crate::native::capture_pomodoros::{self, PomodoroState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TimedRow {
    pub(super) line: usize,
    pub(super) name: Option<String>,
    pub(super) range: String,
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) is_open: bool,
    pub(super) line_text: String,
}

pub(super) fn parse_range_minutes(range: &str) -> Option<(i64, i64)> {
    let (s, e) = range.split_once('-')?;
    if s.len() != 4 || e.len() != 4 {
        return None;
    }
    let sh: i64 = s[..2].parse().ok()?;
    let sm: i64 = s[2..].parse().ok()?;
    let eh: i64 = e[..2].parse().ok()?;
    let em: i64 = e[2..].parse().ok()?;
    if sh >= 24 || sm >= 60 || eh >= 24 || em >= 60 {
        return None;
    }
    Some((sh * 60 + sm, eh * 60 + em))
}

/// Read every timed Pomodoro row without imposing timeline-specific filtering.
/// This is the former reader in `pomodoro_past`; its behavior is unchanged.
pub(super) fn collect_timed_rows(contents: &str) -> Vec<TimedRow> {
    let scan = capture_pomodoros::scan(contents);
    let lines = line_spans(contents);
    let line_texts: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let mut rows = Vec::new();
    for entry in scan.entries {
        let Some(range) = entry.time_range.clone() else {
            continue;
        };
        let Some((s, mut e)) = parse_range_minutes(&range) else {
            continue;
        };
        if s == e {
            // Zero-length rows occupy no time.
            continue;
        }
        if e <= s {
            // Genuine midnight-ending spans wrap across 1440.
            e += 1440;
        }
        let line_text = line_texts
            .get(entry.line.saturating_sub(1))
            .unwrap_or(&"")
            .to_string();
        rows.push(TimedRow {
            line: entry.line,
            name: entry.name.clone(),
            range,
            start: s,
            end: e,
            is_open: entry.state == PomodoroState::Open,
            line_text,
        });
    }
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentKind {
    Pomodoro,
    Gap,
    Tail,
    Head,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Segment {
    pub(super) kind: SegmentKind,
    pub(super) index: Option<u32>,
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) name: Option<String>,
    pub(super) line: Option<usize>,
}

impl Segment {
    /// Canonical short label shared by completion and out-of-range errors.
    pub(super) fn label(&self) -> String {
        let span =
            format!("{}-{}", format_hhmm(self.start), format_hhmm(self.end));
        match self.kind {
            SegmentKind::Pomodoro => format!(
                "~{} {} {span}",
                self.index.unwrap_or_default(),
                display_name(self.name.as_deref())
            ),
            SegmentKind::Gap => {
                format!("~{} gap {span}", self.index.unwrap_or_default())
            }
            SegmentKind::Tail => {
                format!("~ since {} {span}", display_name(self.name.as_deref()))
            }
            SegmentKind::Head => {
                format!(
                    "~~ before {} {span}",
                    display_name(self.name.as_deref())
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Wall {
    pub(super) start: i64,
    pub(super) name: Option<String>,
    pub(super) line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentQuery {
    Tail,
    Start(u32),
    End(u32),
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LimitKind {
    Now,
    Wall(Wall),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pin {
    pub(super) segment: Segment,
    pub(super) edge: Edge,
    pub(super) at: i64,
    pub(super) far_edge: i64,
    /// Present for a start pin. End pins flow backward and have no forward cap.
    pub(super) forward_limit: Option<i64>,
    pub(super) limit: LimitKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TimelineError {
    NothingClosed {
        first_row: Option<(Option<String>, i64)>,
    },
    EmptyTail {
        after: Segment,
        wall: Option<Wall>,
    },
    NoHead {
        first: (Option<String>, i64),
    },
    NoRows,
    OutOfRange {
        count: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Timeline {
    pub(super) now: i64,
    pub(super) t: i64,
    pub(super) rows: Vec<TimedRow>,
    pub(super) walls: Vec<Wall>,
    pub(super) numbered: Vec<Segment>,
    pub(super) tail: Option<Segment>,
    pub(super) head: Option<Segment>,
    pub(super) tail_wall: Option<Wall>,
}

impl Timeline {
    pub(super) fn build(contents: &str, now_min: i64, t: i64) -> Self {
        let now = now_min.clamp(0, 1440);
        let t = t.clamp(0, 1440);
        let rows: Vec<TimedRow> = collect_timed_rows(contents)
            .into_iter()
            .filter(|row| row.start < t)
            .collect();

        let walls: Vec<Wall> = rows
            .iter()
            .filter(|row| row.is_open)
            .map(|row| Wall {
                start: row.start,
                name: row.name.clone(),
                line: row.line,
            })
            .collect();

        let closed: Vec<&TimedRow> =
            rows.iter().filter(|row| !row.is_open).collect();
        let mut numbered = Vec::new();
        for row in &closed {
            // The timeline ends at midnight. A row that crosses midnight keeps
            // the part belonging to this day's 0..=1440 frame.
            let end = row.end.min(1440);
            if end > row.start {
                numbered.push(Segment {
                    kind: SegmentKind::Pomodoro,
                    index: None,
                    start: row.start,
                    end,
                    name: row.name.clone(),
                    line: Some(row.line),
                });
            }
        }

        if let (Some(first), Some(last_closed_end)) = (
            rows.iter().map(|row| row.start).min(),
            closed.iter().map(|row| row.end.min(1440)).max(),
        ) && last_closed_end > first
        {
            let mut covered: Vec<(i64, i64)> = rows
                .iter()
                .filter_map(|row| {
                    let start = row.start.max(first);
                    let end = if row.is_open { t } else { row.end.min(1440) }
                        .min(last_closed_end);
                    (start < end).then_some((start, end))
                })
                .collect();
            covered.sort_unstable();
            let mut merged: Vec<(i64, i64)> = Vec::new();
            for (start, end) in covered {
                if let Some(last) = merged.last_mut()
                    && start <= last.1
                {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            let mut cursor = first;
            for (start, end) in merged {
                if start - cursor >= 5 {
                    numbered.push(Segment {
                        kind: SegmentKind::Gap,
                        index: None,
                        start: cursor,
                        end: start,
                        name: None,
                        line: None,
                    });
                }
                cursor = cursor.max(end);
            }
            if last_closed_end - cursor >= 5 {
                numbered.push(Segment {
                    kind: SegmentKind::Gap,
                    index: None,
                    start: cursor,
                    end: last_closed_end,
                    name: None,
                    line: None,
                });
            }
        }

        // Newest end first. On a tie, the later start and later source line win.
        numbered.sort_by(|a, b| {
            b.end
                .cmp(&a.end)
                .then_with(|| b.start.cmp(&a.start))
                .then_with(|| b.line.unwrap_or(0).cmp(&a.line.unwrap_or(0)))
        });
        for (index, segment) in numbered.iter_mut().enumerate() {
            segment.index = Some((index + 1) as u32);
        }

        let first_row = rows.iter().min_by_key(|row| (row.start, row.line));
        let head = first_row.filter(|row| row.start > 0).map(|row| Segment {
            kind: SegmentKind::Head,
            index: None,
            start: 0,
            end: row.start,
            name: row.name.clone(),
            line: Some(row.line),
        });

        let last_closed = closed
            .iter()
            .max_by_key(|row| (row.end, row.start, row.line));
        let mut tail_wall = None;
        let tail = last_closed.and_then(|row| {
            let start = row.end.min(1440);
            let limiting = walls
                .iter()
                .filter(|wall| wall.start >= start)
                .min_by_key(|wall| (wall.start, wall.line));
            let mut end = t;
            if let Some(wall) = limiting
                && wall.start < end
            {
                end = wall.start;
                tail_wall = Some(wall.clone());
            }
            // A wall can already cover the tail start; preserve it as the
            // reason for the empty tail.
            if let Some(wall) = walls
                .iter()
                .filter(|wall| wall.start <= start && t > start)
                .max_by_key(|wall| (wall.start, wall.line))
            {
                end = start;
                tail_wall = Some(wall.clone());
            }
            (end > start).then_some(Segment {
                kind: SegmentKind::Tail,
                index: None,
                start,
                end,
                name: row.name.clone(),
                line: Some(row.line),
            })
        });

        Self {
            now,
            t,
            rows,
            walls,
            numbered,
            tail,
            head,
            tail_wall,
        }
    }

    pub(super) fn resolve(
        &self,
        query: SegmentQuery,
    ) -> Result<Pin, TimelineError> {
        match query {
            SegmentQuery::Tail => {
                let Some(segment) = self.tail.clone() else {
                    if let Some(after) = self.latest_closed_segment() {
                        return Err(TimelineError::EmptyTail {
                            after,
                            wall: self.tail_wall.clone(),
                        });
                    }
                    return Err(TimelineError::NothingClosed {
                        first_row: self.first_row_position(),
                    });
                };
                Ok(self.start_pin(segment))
            }
            SegmentQuery::Start(count) | SegmentQuery::End(count) => {
                if !self.rows.iter().any(|row| !row.is_open) {
                    return Err(TimelineError::NothingClosed {
                        first_row: self.first_row_position(),
                    });
                }
                let Some(segment) = self
                    .numbered
                    .iter()
                    .find(|segment| segment.index == Some(count))
                    .cloned()
                else {
                    return Err(TimelineError::OutOfRange { count });
                };
                match query {
                    SegmentQuery::Start(_) => Ok(self.start_pin(segment)),
                    SegmentQuery::End(_) => Ok(Pin {
                        at: segment.end,
                        far_edge: segment.start,
                        segment,
                        edge: Edge::End,
                        forward_limit: Some(self.t),
                        limit: LimitKind::Now,
                    }),
                    _ => unreachable!(),
                }
            }
            SegmentQuery::Head => {
                if self.rows.is_empty() {
                    return Err(TimelineError::NoRows);
                }
                let Some(segment) = self.head.clone() else {
                    let row = self
                        .rows
                        .iter()
                        .min_by_key(|row| (row.start, row.line))
                        .expect("nonempty rows checked above");
                    return Err(TimelineError::NoHead {
                        first: (row.name.clone(), row.start),
                    });
                };
                Ok(Pin {
                    at: segment.end,
                    far_edge: 0,
                    segment,
                    edge: Edge::End,
                    forward_limit: Some(self.t),
                    limit: LimitKind::Now,
                })
            }
        }
    }

    pub(super) fn listing(&self) -> String {
        if self.rows.is_empty() {
            return "nothing is on today's timeline yet".to_string();
        }
        let labels =
            self.numbered.iter().map(Segment::label).collect::<Vec<_>>();
        let mut listing = format!(
            "today has {} segment{}",
            self.numbered.len(),
            if self.numbered.len() == 1 { "" } else { "s" }
        );
        if !labels.is_empty() {
            listing.push_str(": ");
            listing.push_str(&labels.join(", "));
        }
        if let Some(head) = &self.head {
            listing.push_str(&format!(
                "; the time before {} is ~~",
                format_hhmm(head.end)
            ));
        }
        listing
    }

    // Used by the segment picker (assists phase) and unit tests.
    #[allow(dead_code)]
    pub(super) fn picker_rows(&self) -> Vec<Segment> {
        self.tail
            .iter()
            .cloned()
            .chain(self.numbered.iter().cloned())
            .chain(self.head.iter().cloned())
            .collect()
    }

    fn start_pin(&self, segment: Segment) -> Pin {
        let wall = self
            .walls
            .iter()
            .filter(|wall| wall.start >= segment.start)
            .min_by_key(|wall| (wall.start, wall.line));
        let (forward_limit, limit) =
            match wall.filter(|wall| wall.start < self.t) {
                Some(wall) => (wall.start, LimitKind::Wall(wall.clone())),
                None => (self.t, LimitKind::Now),
            };
        Pin {
            at: segment.start,
            far_edge: segment.end,
            segment,
            edge: Edge::Start,
            forward_limit: Some(forward_limit),
            limit,
        }
    }

    fn latest_closed_segment(&self) -> Option<Segment> {
        self.numbered
            .iter()
            .filter(|segment| segment.kind == SegmentKind::Pomodoro)
            .max_by_key(|segment| (segment.end, segment.start, segment.line))
            .cloned()
    }

    fn first_row_position(&self) -> Option<(Option<String>, i64)> {
        self.rows
            .iter()
            .min_by_key(|row| (row.start, row.line))
            .map(|row| (row.name.clone(), row.start))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MemberLength {
    pub(super) requested_minutes: u64,
    pub(super) typed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cut {
    Now,
    Wall,
    Segment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Laid {
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) requested_minutes: u64,
    pub(super) cut: Option<Cut>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LayoutError {
    NoRoom { member: usize },
    PastLimit { member: usize },
    BeforeMidnight { member: usize, max_blocks: u32 },
    EndPinAfterNow,
}

pub(super) fn layout_run(
    pin: &Pin,
    members: &[MemberLength],
) -> Result<Vec<Laid>, LayoutError> {
    if pin.edge == Edge::End && pin.at > pin.forward_limit.unwrap_or(i64::MAX) {
        return Err(LayoutError::EndPinAfterNow);
    }
    match pin.edge {
        Edge::Start => layout_forward(pin, members),
        Edge::End => layout_backward(pin, members),
    }
}

fn layout_forward(
    pin: &Pin,
    members: &[MemberLength],
) -> Result<Vec<Laid>, LayoutError> {
    let limit = pin.forward_limit.unwrap_or(i64::MAX);
    let mut cursor = pin.at;
    let mut laid = Vec::with_capacity(members.len());
    for (member_index, member) in members.iter().enumerate() {
        if cursor >= limit {
            return Err(LayoutError::PastLimit {
                member: member_index,
            });
        }
        let requested = minutes_i64(member.requested_minutes, member_index)?;
        let room_to_edge = pin.far_edge.saturating_sub(cursor);
        let mut end = if member.typed {
            cursor.saturating_add(requested)
        } else {
            if room_to_edge <= 0 {
                return Err(LayoutError::NoRoom {
                    member: member_index,
                });
            }
            cursor.saturating_add(requested.min(room_to_edge))
        };
        let segment_cut = !member.typed && requested > room_to_edge;
        let limit_cut = end > limit;
        let cut = if limit_cut {
            end = limit;
            Some(match pin.limit {
                LimitKind::Now => Cut::Now,
                LimitKind::Wall(_) => Cut::Wall,
            })
        } else if segment_cut && pin.far_edge < limit {
            Some(Cut::Segment)
        } else if segment_cut {
            Some(match pin.limit {
                LimitKind::Now => Cut::Now,
                LimitKind::Wall(_) => Cut::Wall,
            })
        } else {
            None
        };
        if end <= cursor {
            return Err(LayoutError::NoRoom {
                member: member_index,
            });
        }
        laid.push(Laid {
            start: cursor,
            end,
            requested_minutes: member.requested_minutes,
            cut,
        });
        cursor = end;
    }
    Ok(laid)
}

fn layout_backward(
    pin: &Pin,
    members: &[MemberLength],
) -> Result<Vec<Laid>, LayoutError> {
    if pin.at > pin.forward_limit.unwrap_or(i64::MAX) {
        return Err(LayoutError::EndPinAfterNow);
    }
    let mut cursor = pin.at;
    let mut reversed = Vec::with_capacity(members.len());
    for member_index in (0..members.len()).rev() {
        let member = members[member_index];
        let requested = minutes_i64(member.requested_minutes, member_index)?;
        let room_to_edge = cursor.saturating_sub(pin.far_edge);
        let mut start = if member.typed {
            cursor.saturating_sub(requested)
        } else {
            if room_to_edge <= 0 {
                return Err(LayoutError::NoRoom {
                    member: member_index,
                });
            }
            cursor.saturating_sub(requested.min(room_to_edge))
        };
        let segment_cut = !member.typed && requested > room_to_edge;
        let cut = segment_cut.then_some(Cut::Segment);
        if start < 0 {
            return Err(LayoutError::BeforeMidnight {
                member: member_index,
                max_blocks: cursor.max(0).div_euclid(5) as u32,
            });
        }
        if start >= cursor {
            return Err(LayoutError::NoRoom {
                member: member_index,
            });
        }
        // `far_edge` is zero for a head pin. A typed exact count can cross its
        // segment edge; only midnight is a hard stop.
        if !member.typed && start < pin.far_edge {
            start = pin.far_edge;
        }
        reversed.push(Laid {
            start,
            end: cursor,
            requested_minutes: member.requested_minutes,
            cut,
        });
        cursor = start;
    }
    reversed.reverse();
    Ok(reversed)
}

fn minutes_i64(minutes: u64, member: usize) -> Result<i64, LayoutError> {
    i64::try_from(minutes).map_err(|_| LayoutError::BeforeMidnight {
        member,
        max_blocks: 0,
    })
}

fn format_hhmm(minutes: i64) -> String {
    let minutes = minutes.clamp(0, 1440);
    if minutes == 1440 {
        "0000".to_string()
    } else {
        format!("{:02}{:02}", minutes / 60, minutes % 60)
    }
}

fn display_name(name: Option<&str>) -> &str {
    name.filter(|name| !name.trim().is_empty())
        .unwrap_or("session")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "## Pomodoros\n\
- [x] (0730-0800) — PLAN\n\
- [x] (0830-0900) — BUILD\n\
- [ ] () — CHORE\n\
- [ ] () — READ\n";

    fn timeline(contents: &str, now: i64) -> Timeline {
        Timeline::build(contents, now, (now + 4) / 5 * 5)
    }

    fn one(requested_minutes: u64, typed: bool) -> MemberLength {
        MemberLength {
            requested_minutes,
            typed,
        }
    }

    #[test]
    fn example_catalog_numbers_segments_and_resolves_every_pin() {
        let day = timeline(EXAMPLE, 9 * 60 + 13);
        assert_eq!(
            day.numbered.iter().map(Segment::label).collect::<Vec<_>>(),
            [
                "~1 BUILD 0830-0900",
                "~2 gap 0800-0830",
                "~3 PLAN 0730-0800"
            ]
        );
        assert_eq!(
            day.tail.as_ref().unwrap().label(),
            "~ since BUILD 0900-0915"
        );
        assert_eq!(
            day.head.as_ref().unwrap().label(),
            "~~ before PLAN 0000-0730"
        );

        let tail = day.resolve(SegmentQuery::Tail).unwrap();
        assert_eq!(
            (tail.edge, tail.at, tail.far_edge),
            (Edge::Start, 540, 555)
        );
        for (count, start, end) in [(1, 510, 540), (2, 480, 510), (3, 450, 480)]
        {
            let pin = day.resolve(SegmentQuery::Start(count)).unwrap();
            assert_eq!(
                (pin.edge, pin.at, pin.far_edge),
                (Edge::Start, start, end)
            );
            let pin = day.resolve(SegmentQuery::End(count)).unwrap();
            assert_eq!(
                (pin.edge, pin.at, pin.far_edge),
                (Edge::End, end, start)
            );
        }
        let head = day.resolve(SegmentQuery::Head).unwrap();
        assert_eq!((head.edge, head.at, head.far_edge), (Edge::End, 450, 0));
        assert_eq!(day.picker_rows().len(), 5);
    }

    #[test]
    fn example_layouts_match_the_acceptance_windows() {
        let day = timeline(EXAMPLE, 553);
        let checks = [
            (
                SegmentQuery::Tail,
                one(25, false),
                (540, 555),
                Some(Cut::Now),
            ),
            (SegmentQuery::Tail, one(10, true), (540, 550), None),
            (SegmentQuery::Start(1), one(10, true), (510, 520), None),
            (SegmentQuery::Start(1), one(25, false), (510, 535), None),
            (SegmentQuery::End(1), one(10, true), (530, 540), None),
            (SegmentQuery::Start(2), one(25, false), (480, 505), None),
            (SegmentQuery::End(2), one(25, false), (485, 510), None),
            (SegmentQuery::Head, one(25, false), (425, 450), None),
        ];
        for (query, member, expected, cut) in checks {
            let pin = day.resolve(query).unwrap();
            let laid = layout_run(&pin, &[member]).unwrap()[0];
            assert_eq!((laid.start, laid.end), expected, "{query:?}");
            assert_eq!(laid.cut, cut, "{query:?}");
        }
        let tail = day.resolve(SegmentQuery::Tail).unwrap();
        assert_eq!(
            layout_run(&tail, &[one(20, true)]).unwrap()[0].cut,
            Some(Cut::Now)
        );
        assert_eq!(
            layout_run(&tail, &[one(25, false)]).unwrap()[0].cut,
            Some(Cut::Now)
        );
    }

    #[test]
    fn both_pins_layout_runs_in_typed_order() {
        let day = timeline(EXAMPLE, 553);
        let start = day.resolve(SegmentQuery::Start(2)).unwrap();
        assert_eq!(
            layout_run(&start, &[one(10, true), one(25, false)]).unwrap(),
            [
                Laid {
                    start: 480,
                    end: 490,
                    requested_minutes: 10,
                    cut: None,
                },
                Laid {
                    start: 490,
                    end: 510,
                    requested_minutes: 25,
                    cut: Some(Cut::Segment),
                },
            ]
        );
        let end = day.resolve(SegmentQuery::End(2)).unwrap();
        let laid = layout_run(&end, &[one(5, true), one(10, true)]).unwrap();
        assert_eq!((laid[0].start, laid[0].end), (495, 500));
        assert_eq!((laid[1].start, laid[1].end), (500, 510));
        let three =
            layout_run(&end, &[one(5, true), one(5, true), one(5, true)])
                .unwrap();
        assert_eq!(
            three
                .iter()
                .map(|window| (window.start, window.end))
                .collect::<Vec<_>>(),
            [(495, 500), (500, 505), (505, 510)]
        );
        let three =
            layout_run(&start, &[one(5, true), one(5, true), one(5, true)])
                .unwrap();
        assert_eq!(
            three
                .iter()
                .map(|window| (window.start, window.end))
                .collect::<Vec<_>>(),
            [(480, 485), (485, 490), (490, 495)]
        );
    }

    #[test]
    fn empty_future_and_back_to_back_days_are_handled() {
        assert!(timeline("## Pomodoros\n", 500).rows.is_empty());
        let future = "## Pomodoros\n- [x] (1400-1430) — FUTURE\n";
        assert!(timeline(future, 9 * 60).rows.is_empty());
        let back_to_back =
            "## Pomodoros\n- [x] (0800-0830) — A\n- [x] (0830-0900) — B\n";
        assert_eq!(timeline(back_to_back, 9 * 60).numbered.len(), 2);
    }

    #[test]
    fn short_slivers_are_unnumbered_and_overlaps_keep_each_closed_row() {
        let sliver =
            "## Pomodoros\n- [x] (0800-0825) — A\n- [x] (0827-0900) — B\n";
        assert_eq!(timeline(sliver, 9 * 60).numbered.len(), 2);
        let overlap =
            "## Pomodoros\n- [x] (0800-0830) — A\n- [x] (0810-0840) — B\n";
        let day = timeline(overlap, 9 * 60);
        assert_eq!(
            day.numbered
                .iter()
                .filter(|segment| segment.kind == SegmentKind::Pomodoro)
                .count(),
            2
        );
        assert!(!day
            .numbered
            .iter()
            .any(|segment| segment.kind == SegmentKind::Gap));
    }

    #[test]
    fn off_clock_rows_are_timeline_rows_and_midnight_edges_stay_bounded() {
        let off_clock = "## Pomodoros\n- [x] (0800-0837) [off:: 37m] — MEET\n";
        let day = timeline(off_clock, 9 * 60);
        assert_eq!((day.numbered[0].start, day.numbered[0].end), (480, 517));
        let midnight_end = "## Pomodoros\n- [x] (2330-0000) — LATE\n";
        let day = timeline(midnight_end, 1440);
        assert_eq!((day.numbered[0].start, day.numbered[0].end), (1410, 1440));
        assert!(day.head.is_some());
    }

    #[test]
    fn walls_bound_the_tail_and_start_pins() {
        let contents = "## Pomodoros\n- [x] (0830-0900) — BUILD\n- [ ] (0905-0925) — WORK\n";
        let day = timeline(contents, 553);
        assert_eq!(
            day.tail.as_ref().unwrap().label(),
            "~ since BUILD 0900-0905"
        );
        let pin = day.resolve(SegmentQuery::Tail).unwrap();
        let window = layout_run(&pin, &[one(25, true)]).unwrap()[0];
        assert_eq!(
            (window.start, window.end, window.cut),
            (540, 545, Some(Cut::Wall))
        );

        let covering = "## Pomodoros\n- [x] (0830-0900) — BUILD\n- [ ] (0855-0925) — WORK\n";
        let day = timeline(covering, 553);
        assert!(day.tail.is_none());
        assert!(matches!(
            day.resolve(SegmentQuery::Tail),
            Err(TimelineError::EmptyTail { wall: Some(_), .. })
        ));
        let pin = day.resolve(SegmentQuery::Start(1)).unwrap();
        assert_eq!(pin.forward_limit, Some(535));

        let at_t = "## Pomodoros\n- [x] (0830-0900) — BUILD\n- [ ] (0915-0930) — FUTURE\n";
        let day = timeline(at_t, 553);
        assert!(day.walls.is_empty());
        assert_eq!(day.tail.as_ref().unwrap().end, 555);
    }

    #[test]
    fn end_pins_check_now_midnight_and_empty_room() {
        let after_now = Pin {
            segment: Segment {
                kind: SegmentKind::Pomodoro,
                index: Some(1),
                start: 540,
                end: 560,
                name: Some("LATE".into()),
                line: Some(2),
            },
            edge: Edge::End,
            at: 560,
            far_edge: 540,
            forward_limit: Some(555),
            limit: LimitKind::Now,
        };
        assert_eq!(
            layout_run(&after_now, &[one(5, true)]),
            Err(LayoutError::EndPinAfterNow)
        );
        let day = timeline("## Pomodoros\n- [x] (0020-0040) — FIRST\n", 50);
        let head = day.resolve(SegmentQuery::Head).unwrap();
        assert_eq!(
            layout_run(&head, &[one(25, false)]).unwrap()[0].cut,
            Some(Cut::Segment)
        );
        assert_eq!(
            layout_run(&head, &[one(25, true)]),
            Err(LayoutError::BeforeMidnight {
                member: 0,
                max_blocks: 4,
            })
        );
        let short = Pin {
            far_edge: 25,
            ..head
        };
        assert_eq!(
            layout_run(&short, &[one(5, false)]),
            Err(LayoutError::NoRoom { member: 0 })
        );
    }

    #[test]
    fn errors_and_out_of_range_listing_are_structured() {
        let empty = timeline("## Pomodoros\n", 500);
        assert_eq!(
            empty.resolve(SegmentQuery::Head),
            Err(TimelineError::NoRows)
        );
        assert!(matches!(
            empty.resolve(SegmentQuery::Start(1)),
            Err(TimelineError::NothingClosed { first_row: None })
        ));
        let midnight =
            timeline("## Pomodoros\n- [x] (0000-0030) — FIRST\n", 40);
        assert!(matches!(
            midnight.resolve(SegmentQuery::Head),
            Err(TimelineError::NoHead { .. })
        ));
        assert_eq!(midnight.numbered[0].start, 0);
        let day = timeline(EXAMPLE, 553);
        assert_eq!(
            day.resolve(SegmentQuery::Start(4)),
            Err(TimelineError::OutOfRange { count: 4 })
        );
        assert!(day.listing().contains("today has 3 segments"));
        assert!(day.listing().contains("time before 0730 is ~~"));
    }

    #[test]
    fn tail_at_close_is_empty_and_stale_open_rows_do_not_make_gaps() {
        let at_t = "## Pomodoros\n- [x] (0830-0900) — BUILD\n";
        let day = timeline(at_t, 540);
        assert!(day.tail.is_none());
        assert!(matches!(
            day.resolve(SegmentQuery::Tail),
            Err(TimelineError::EmptyTail { wall: None, .. })
        ));

        let stale = "## Pomodoros\n- [ ] (0700-0800) — OLD\n- [x] (0800-0830) — BUILD\n";
        let day = timeline(stale, 553);
        assert!(day
            .numbered
            .iter()
            .all(|segment| segment.kind != SegmentKind::Gap));
        assert!(day.tail.is_none());
    }

    #[test]
    fn every_layout_cut_and_limit_error_is_accounted_for() {
        let day = timeline(EXAMPLE, 553);
        let tail = day.resolve(SegmentQuery::Tail).unwrap();
        assert_eq!(
            layout_run(&tail, &[one(25, false)]).unwrap()[0].cut,
            Some(Cut::Now)
        );
        assert_eq!(layout_run(&tail, &[one(10, true)]).unwrap()[0].cut, None);
        let start = day.resolve(SegmentQuery::Start(2)).unwrap();
        assert_eq!(layout_run(&start, &[one(35, true)]).unwrap()[0].cut, None);
        assert_eq!(
            layout_run(&tail, &[one(25, false), one(5, true)]),
            Err(LayoutError::PastLimit { member: 1 })
        );
        assert_eq!(
            layout_run(&start, &[one(1, false), one(1, false)]).unwrap()[0].cut,
            None
        );
        let no_room = Pin {
            far_edge: 480,
            ..start
        };
        assert_eq!(
            layout_run(&no_room, &[one(5, false)]),
            Err(LayoutError::NoRoom { member: 0 })
        );
    }

    #[test]
    fn equal_end_segments_sort_later_start_then_later_line_first() {
        let contents =
            "## Pomodoros\n- [x] (0800-0830) — A\n- [x] (0810-0830) — B\n";
        let day = timeline(contents, 9 * 60);
        assert_eq!(day.numbered[0].name.as_deref(), Some("B"));
        assert_eq!(day.numbered[1].name.as_deref(), Some("A"));
    }
}

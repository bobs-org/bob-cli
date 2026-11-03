//! Deterministic period-note scaffolds.
//!
//! Every scaffold is a pure function of its period id: no `created:`
//! field and no wall clock. The contract lives in `docs/periods.md` §2.

use super::calendar::{weeks_in_year, PeriodId};
use super::{instructions, labels};

/// Render the exact scaffold bytes for `id` (LF, one blank line between
/// blocks, a single trailing newline).
pub(crate) fn render(id: PeriodId) -> String {
    match id {
        PeriodId::Week { .. } => render_week(id),
        PeriodId::Quarter { .. } => render_quarter(id),
        PeriodId::Year(_) => render_year(id),
        PeriodId::AllTime => render_all_time(),
    }
}

fn frontmatter_week(id: PeriodId) -> String {
    let (Some(start), Some(end)) = (id.start(), id.end()) else {
        return String::new();
    };
    let parent = id.parent().expect("week has a quarter parent");
    let title = labels::title(id);
    format!(
        "---\nparent: \"[[{}]]\"\ntype: \"[[day]]\"\nid: {id}\ndate: {start}\nend: {end}\naliases:\n  - \"{title}\"\ntags:\n  - weekly\n---",
        parent.link_target(),
    )
}

fn frontmatter_quarter(id: PeriodId) -> String {
    let (Some(start), Some(end)) = (id.start(), id.end()) else {
        return String::new();
    };
    let parent = id.parent().expect("quarter has a year parent");
    let title = labels::title(id);
    format!(
        "---\nparent: \"[[{}]]\"\ntype: \"[[day]]\"\nid: {id}\ndate: {start}\nend: {end}\naliases:\n  - \"{title}\"\ntags:\n  - quarterly\n---",
        parent.link_target(),
    )
}

fn frontmatter_year(id: PeriodId) -> String {
    let (Some(start), Some(end)) = (id.start(), id.end()) else {
        return String::new();
    };
    let title = labels::title(id);
    format!(
        "---\nparent: \"[[all_time]]\"\ntype: \"[[day]]\"\nid: \"{id}\"\ndate: {start}\nend: {end}\naliases:\n  - \"{title}\"\ntags:\n  - yearly\n---"
    )
}

fn frontmatter_all_time() -> String {
    "---\nparent: \"[[org]]\"\ntype: \"[[day]]\"\nid: all_time\naliases:\n  - \"All time\"\ntags:\n  - all-time\n---"
        .to_string()
}

fn week_nav(id: PeriodId) -> String {
    let current_year = match id {
        PeriodId::Week { year, .. } => year,
        _ => 0,
    };
    let prev = id.prev().expect("week has neighbours in range");
    let parent = id.parent().expect("week has a parent");
    let next = id.next().expect("week has neighbours in range");
    format!(
        "[[{}|{}]] · [[{}|{}]] · [[{}|{}]]",
        prev.link_target(),
        labels::prev_label(prev, current_year),
        parent.link_target(),
        labels::up_label(Some(parent)),
        next.link_target(),
        labels::next_label(next, current_year),
    )
}

fn quarter_nav(id: PeriodId) -> String {
    let current_year = match id {
        PeriodId::Quarter { year, .. } => year,
        _ => 0,
    };
    let prev = id.prev().expect("quarter has neighbours in range");
    let parent = id.parent().expect("quarter has a parent");
    let next = id.next().expect("quarter has neighbours in range");
    format!(
        "[[{}|{}]] · [[{}|{}]] · [[{}|{}]]",
        prev.link_target(),
        labels::prev_label(prev, current_year),
        parent.link_target(),
        labels::up_label(Some(parent)),
        next.link_target(),
        labels::next_label(next, current_year),
    )
}

fn year_nav(id: PeriodId) -> String {
    let current_year = match id {
        PeriodId::Year(year) => year,
        _ => 0,
    };
    let prev = id.prev().expect("year has neighbours in range");
    let next = id.next().expect("year has neighbours in range");
    format!(
        "[[{}|{}]] · [[all_time|↑ All time]] · [[{}|{}]]",
        prev.link_target(),
        labels::prev_label(prev, current_year),
        next.link_target(),
        labels::next_label(next, current_year),
    )
}

fn week_day_links(id: PeriodId) -> String {
    let dates = id.week_dates();
    let mut parts = Vec::new();
    for (index, date) in dates.iter().enumerate() {
        let vault_path = format!(
            "{:04}/{:04}{:02}{:02}",
            date.year(),
            date.year(),
            date.month(),
            date.day()
        );
        let label = labels::day_label(*date, index == 0);
        parts.push(format!("[[{vault_path}|{label}]]"));
    }
    parts.join(" · ")
}

use chrono::Datelike;

fn quarter_child_links(id: PeriodId) -> String {
    id.quarter_weeks()
        .iter()
        .map(|week| {
            format!("[[{}|{}]]", week.link_target(), labels::short_label(*week))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn year_child_links(id: PeriodId) -> String {
    id.year_quarters()
        .iter()
        .map(|quarter| {
            format!(
                "[[{}|{}]]",
                quarter.link_target(),
                labels::short_label(*quarter)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn render_week(id: PeriodId) -> String {
    let title = labels::title(id);
    let parts = vec![
        frontmatter_week(id),
        format!("# {title}"),
        week_nav(id),
        week_day_links(id),
        "```bob-stats-strip\n\n```".to_string(),
        "## Intentions".to_string(),
        format!("## Days\n\n```bob-period-parts\n\n```"),
        "## Review".to_string(),
        "## Reflection".to_string(),
        instructions::render(id),
    ];
    format!("{}\n", parts.join("\n\n"))
}

fn render_quarter(id: PeriodId) -> String {
    let title = labels::title(id);
    let parts = vec![
        frontmatter_quarter(id),
        format!("# {title}"),
        quarter_nav(id),
        quarter_child_links(id),
        "```bob-stats-strip\n\n```".to_string(),
        "## Outcomes".to_string(),
        format!("## Weeks\n\n```bob-period-parts\n\n```"),
        "## Review".to_string(),
        "## Reflection".to_string(),
        instructions::render(id),
    ];
    format!("{}\n", parts.join("\n\n"))
}

fn render_year(id: PeriodId) -> String {
    let title = labels::title(id);
    let parts = vec![
        frontmatter_year(id),
        format!("# {title}"),
        year_nav(id),
        year_child_links(id),
        "```bob-stats-strip\n\n```".to_string(),
        "## Theme".to_string(),
        "## Outcomes".to_string(),
        format!("## Quarters\n\n```bob-period-parts\n\n```"),
        "## Review".to_string(),
        "## Reflection".to_string(),
        instructions::render(id),
    ];
    format!("{}\n", parts.join("\n\n"))
}

fn render_all_time() -> String {
    let parts = vec![
        frontmatter_all_time(),
        "# All time".to_string(),
        "[[org|↑ org]]".to_string(),
        "```bob-stats-strip\n\n```".to_string(),
        "## Years\n\n```bob-period-parts\n\n```".to_string(),
        "## Notes".to_string(),
        instructions::render(PeriodId::AllTime),
    ];
    format!("{}\n", parts.join("\n\n"))
}

/// The week count text for a year title (`53 weeks`).
#[allow(dead_code)]
pub(crate) fn year_week_count(year: i32) -> u32 {
    weeks_in_year(year)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn week_scaffold_is_deterministic() {
        let id = PeriodId::Week {
            year: 2026,
            week: 45,
        };
        let first = render(id);
        let second = render(id);
        assert_eq!(first, second);
        assert!(first.ends_with('\n'));
        assert!(!first.ends_with("\n\n"));
        assert!(first.contains("parent: \"[[2026/2026-Q4]]\""));
        assert!(first.contains("# 2026-W45 · Nov 2 – 8"));
        assert!(first.contains("## Intentions"));
        assert!(first.contains("## Days"));
        assert!(first.contains("```bob-stats-strip"));
        assert!(first.contains("```bob-period-parts"));
        assert!(first.contains("## Review"));
        assert!(first.contains("## Reflection"));
        assert!(first.contains("Agent instructions · weekly v1"));
    }

    #[test]
    fn quarter_year_and_all_time_scaffolds_carry_their_sections() {
        let quarter = PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        };
        let rendered = render(quarter);
        assert!(rendered
            .contains("# 2026-Q4 · W40–W53 · Sep 28, 2026 – Jan 3, 2027"));
        assert!(rendered.contains("## Outcomes"));
        assert!(rendered.contains("## Weeks"));

        let year = PeriodId::Year(2026);
        let rendered = render(year);
        assert!(
            rendered.contains("# 2026 · Dec 29, 2025 – Jan 3, 2027 · 53 weeks")
        );
        assert!(rendered.contains("## Theme"));
        assert!(rendered.contains("## Quarters"));

        let rendered = render(PeriodId::AllTime);
        assert!(rendered.contains("# All time"));
        assert!(rendered.contains("## Years"));
        assert!(rendered.contains("## Notes"));
    }

    fn golden_ids() -> Vec<(PeriodId, &'static str)> {
        vec![
            (
                PeriodId::Week {
                    year: 2026,
                    week: 45,
                },
                "scaffold-2026-W45.md",
            ),
            (
                PeriodId::Week {
                    year: 2026,
                    week: 1,
                },
                "scaffold-2026-W01.md",
            ),
            (
                PeriodId::Week {
                    year: 2026,
                    week: 53,
                },
                "scaffold-2026-W53.md",
            ),
            (
                PeriodId::Quarter {
                    year: 2026,
                    quarter: 4,
                },
                "scaffold-2026-Q4.md",
            ),
            (
                PeriodId::Quarter {
                    year: 2026,
                    quarter: 1,
                },
                "scaffold-2026-Q1.md",
            ),
            (PeriodId::Year(2026), "scaffold-2026.md"),
            (PeriodId::AllTime, "scaffold-all_time.md"),
        ]
    }

    #[test]
    fn scaffolds_match_golden_bytes() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let update = std::env::var("PERIODS_UPDATE_GOLDENS").is_ok();
        for (id, file) in golden_ids() {
            let path = manifest.join("tests/fixtures/periods").join(file);
            let rendered = render(id);
            if update {
                std::fs::write(&path, &rendered)
                    .unwrap_or_else(|_| panic!("write {}", path.display()));
            }
            let expected =
                std::fs::read_to_string(&path).unwrap_or_else(|_| {
                    panic!(
                        "missing golden {}; run with PERIODS_UPDATE_GOLDENS=1",
                        path.display()
                    )
                });
            assert_eq!(
                rendered,
                expected,
                "scaffold bytes drifted for {id}; run with PERIODS_UPDATE_GOLDENS=1 to refresh"
            );
        }
    }
}

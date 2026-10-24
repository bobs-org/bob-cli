use std::collections::BTreeSet;

use chrono::NaiveDate;

use crate::native::{
    capture_language::selector_slug, plan_budget::normalize_component,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RankTheme<'a> {
    pub(crate) key: &'a str,
    pub(crate) name: &'a str,
    pub(crate) last_used: NaiveDate,
    pub(crate) days_used: u32,
}

/// PT7 groups B (exact/prefix) then D (substring). Hidden and today-row
/// exclusions are the caller's job.
pub(crate) fn rank_history(
    themes: &[RankTheme<'_>],
    query: &str,
    exclusions: &BTreeSet<String>,
) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        let mut order: Vec<usize> = (0..themes.len())
            .filter(|index| !exclusions.contains(themes[*index].key))
            .collect();
        order.sort_by(|&left, &right| {
            compare_recent(&themes[left], &themes[right])
        });
        return order;
    }

    let query_slug = selector_slug(query);
    let query_key = normalize_component(query);
    let mut group_b = Vec::new();
    let mut group_d = Vec::new();
    for (index, theme) in themes.iter().enumerate() {
        if exclusions.contains(theme.key) {
            continue;
        }
        let slug = selector_slug(theme.name);
        let exact = slug == query_slug || theme.key == query_key;
        let prefix = !exact
            && ((!query_slug.is_empty() && slug.starts_with(&query_slug))
                || theme.key.starts_with(&query_key));
        if exact || prefix {
            group_b.push((exact, index));
            continue;
        }
        let substring = (!query_slug.is_empty() && slug.contains(&query_slug))
            || theme.key.contains(&query_key);
        if substring {
            group_d.push(index);
        }
    }
    group_b.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| compare_recent(&themes[left.1], &themes[right.1]))
    });
    group_d
        .sort_by(|&left, &right| compare_recent(&themes[left], &themes[right]));
    group_b
        .into_iter()
        .map(|(_, index)| index)
        .chain(group_d)
        .collect()
}

fn compare_recent(
    left: &RankTheme<'_>,
    right: &RankTheme<'_>,
) -> std::cmp::Ordering {
    right
        .last_used
        .cmp(&left.last_used)
        .then_with(|| right.days_used.cmp(&left.days_used))
        .then_with(|| left.name.cmp(right.name))
        .then_with(|| left.key.cmp(right.key))
}

use std::collections::BTreeSet;

use super::json::HomePart;

pub(super) fn name_components(name: Option<&str>) -> Vec<String> {
    name.into_iter()
        .flat_map(|name| name.split('+'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub(super) fn label(parts: &[HomePart]) -> String {
    let mut ranked = parts.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.share.cmp(&left.share));
    let shown = ranked
        .iter()
        .take(2)
        .map(|part| part.label.as_str())
        .collect::<Vec<_>>();
    if shown.is_empty() {
        return String::new();
    }
    let mut result = shown.join(" · ");
    if ranked.len() > shown.len() {
        result.push_str(&format!(" +{}", ranked.len() - shown.len()));
    }
    result
}

pub(super) fn chip(
    name: Option<&str>,
    parts: &[HomePart],
    named_review: bool,
    unlinked: bool,
) -> Option<String> {
    if unlinked || named_review {
        return None;
    }
    let value = label(parts);
    if value.is_empty() {
        return None;
    }
    let names = name_components(name).into_iter().collect::<BTreeSet<_>>();
    let labels = parts
        .iter()
        .map(|part| part.label.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    if !names.is_empty() && names == labels {
        None
    } else {
        Some(value)
    }
}

/// Allocate whole percentages so every populated dimension sums to 100.
/// Largest fractional remainders win; equal remainders keep ledger order.
pub(super) fn percentages(weights: &[u64]) -> Vec<u8> {
    let total = weights.iter().sum::<u64>();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let mut result = Vec::with_capacity(weights.len());
    let mut remainders = Vec::with_capacity(weights.len());
    let mut assigned = 0u16;
    for (index, weight) in weights.iter().copied().enumerate() {
        let numerator = weight.saturating_mul(100);
        let floor = (numerator / total) as u8;
        result.push(floor);
        assigned += u16::from(floor);
        remainders.push((index, numerator % total));
    }
    remainders.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
    });
    for (index, _) in remainders.into_iter().take((100 - assigned) as usize) {
        result[index] += 1;
    }
    result
}

//! Shared case-insensitive ranking and warning helpers for the
//! capture-completion candidate providers.

pub(super) fn bounded_warning(message: String) -> String {
    const LIMIT: usize = 300;
    if message.chars().count() <= LIMIT {
        return message;
    }
    let mut truncated = message.chars().take(LIMIT - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

/// Case-insensitive candidate ranking: exact prefix matches before
/// substring matches, keeping each discovery source's stable order within
/// each group. A non-matching item is dropped. An empty query keeps every
/// item so a fresh `@` lists the whole discovery set.
pub(super) fn rank<T>(
    items: Vec<T>,
    query: &str,
    key: impl Fn(&T) -> &str,
) -> Vec<T> {
    if query.is_empty() {
        return items;
    }

    let query = query.to_lowercase();
    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();
    for item in items {
        let value = key(&item).to_lowercase();
        if value.starts_with(&query) {
            prefix_matches.push(item);
        } else if value.contains(&query) {
            substring_matches.push(item);
        }
    }

    prefix_matches.extend(substring_matches);
    prefix_matches
}

//! Google-event clock resolution shared by list and snapshot.
//!
//! List previews Google's chosen root plus the explicit tag against the
//! daily-note residence. Sync later resolves the retained vault line.

use std::path::Path;

use crate::native::{
    clock_tags::{ClockKind, ClockTag},
    pomodoro_homes::{
        effective_clock, ClockResolution, ClockSource, HomeResolver, HomeSource,
    },
};

/// Effective clock (explicit tag, then chosen-root walk) plus the untagged
/// root-only walk used later for drop advice.
pub(crate) fn resolve_listed_clock<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    daily_rel: &Path,
    chosen_root: Option<&str>,
    tag: &ClockTag,
    inherit: bool,
) -> (ClockResolution, ClockResolution) {
    if !inherit {
        let tag_only = ClockResolution::tag_only(tag);
        return (tag_only.clone(), tag_only);
    }
    let Some(root) = chosen_root.filter(|root| !root.is_empty()) else {
        let tag_only = ClockResolution::tag_only(tag);
        return (tag_only.clone(), tag_only);
    };
    let effective =
        effective_clock(resolver, daily_rel, &probe_line(root, tag));
    let root_only = effective_clock(
        resolver,
        daily_rel,
        &probe_line(root, &ClockTag::None),
    );
    (effective, root_only)
}

fn probe_line(root: &str, tag: &ClockTag) -> String {
    let mut line = String::from("- [ ] #task Event");
    match tag.kind() {
        ClockKind::Off => line.push_str(" #off"),
        ClockKind::On => line.push_str(" #on"),
        ClockKind::None => {}
    }
    line.push_str(" [root:: ");
    line.push_str(root);
    line.push(']');
    line
}

pub(crate) fn explicit_clock_label(tag: &ClockTag) -> Option<&'static str> {
    match tag.kind() {
        ClockKind::None => None,
        ClockKind::Off => Some("off"),
        ClockKind::On => Some("on"),
    }
}

pub(crate) fn via_stem(note: &str) -> &str {
    Path::new(note)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(note)
}

pub(crate) fn off_clock_status_suffix(clock: &ClockResolution) -> String {
    if !clock.off_clock {
        return String::new();
    }
    if clock.source == ClockSource::Note
        && let Some(note) = clock.note.as_deref()
    {
        return format!(" · off-clock via {}", via_stem(note));
    }
    " · off-clock".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::vault_links::NoteIndex;
    use crate::native::{
        clock_tags::ClockTag,
        pomodoro_homes::{HomeResolver, IndexedHomeSource},
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn listed_clock_inherits_through_the_chosen_root_only() {
        let mut notes = BTreeMap::new();
        notes.insert(
            PathBuf::from("love.md"),
            "---\ntype: [[area]]\noffclock: true\n---\n".to_string(),
        );
        notes.insert(
            PathBuf::from("job.md"),
            "---\ntype: [[area]]\n---\n".to_string(),
        );
        notes.insert(PathBuf::from("2026/20261030.md"), "# Day\n".to_string());
        let index = NoteIndex::from_paths(notes.keys().cloned());
        let mut resolver =
            HomeResolver::new(IndexedHomeSource::new(&index, &notes));
        let daily = Path::new("2026/20261030.md");

        let (inherited, root_only) = resolve_listed_clock(
            &mut resolver,
            daily,
            Some("love"),
            &ClockTag::None,
            true,
        );
        assert!(inherited.off_clock);
        assert_eq!(inherited.source, ClockSource::Note);
        assert_eq!(inherited.note.as_deref(), Some("love.md"));
        assert!(root_only.off_clock);

        let (forced_on, untagged) = resolve_listed_clock(
            &mut resolver,
            daily,
            Some("love"),
            &ClockTag::On,
            true,
        );
        assert!(!forced_on.off_clock);
        assert_eq!(forced_on.source, ClockSource::OnTag);
        assert!(untagged.off_clock);

        let (context, _) = resolve_listed_clock(
            &mut resolver,
            daily,
            Some("love"),
            &ClockTag::Off {
                deprecated: Vec::new(),
            },
            false,
        );
        assert!(context.off_clock);
        assert_eq!(context.source, ClockSource::OffTag);
        assert_eq!(explicit_clock_label(&ClockTag::None), None);
        assert_eq!(explicit_clock_label(&ClockTag::On), Some("on"));
        assert_eq!(
            off_clock_status_suffix(&inherited),
            " · off-clock via love"
        );
    }
}

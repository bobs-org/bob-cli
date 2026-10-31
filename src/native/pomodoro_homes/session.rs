use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use crate::native::{
    capture_pomodoro_start::{self, PreparedSessionScan, StartLink},
    vault_links::LinkResolution,
};

use super::{
    json::{HomePart, Homes},
    label::{chip, label, percentages, LabelPart},
    resolve::{path_string, root_label, unlinked_home},
    Home, HomeResolver, HomeSource, Hue, SessionKind, SessionState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionLink {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) line: usize,
    pub(crate) embedded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionHomes {
    pub(crate) homes: Homes,
    pub(crate) task_homes: Vec<Home>,
    pub(crate) lead_home: Option<Home>,
}

pub(crate) fn prepare_session_scan(contents: &str) -> PreparedSessionScan<'_> {
    capture_pomodoro_start::prepare_session_scan(contents)
}

pub(crate) fn session_links(
    contents: &str,
    entry_index: usize,
) -> Vec<SessionLink> {
    map_session_links(capture_pomodoro_start::session_links(
        contents,
        entry_index,
    ))
}

pub(crate) fn session_links_from_scan(
    scan: &PreparedSessionScan<'_>,
    entry_index: usize,
) -> Vec<SessionLink> {
    map_session_links(capture_pomodoro_start::session_links_from_scan(
        scan,
        entry_index,
    ))
}

fn map_session_links(links: Vec<StartLink>) -> Vec<SessionLink> {
    links
        .into_iter()
        .map(|link: StartLink| SessionLink {
            target: link.path_part,
            block_id: link.block_id,
            line: link.ledger_line,
            embedded: link.embedded,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LinkIdentity {
    Found(String, String),
    Ambiguous(String, String),
    Missing(String, String),
}

struct ResolvedLink {
    resolution: LinkResolution,
    block_id: String,
}

pub(crate) fn attribute_ledger_entry<S: HomeSource>(
    contents: &str,
    entry_line: usize,
    name: Option<&str>,
    closed: bool,
    minutes: Option<u32>,
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
) -> SessionHomes {
    let links = session_links(contents, entry_line.saturating_sub(1));
    attribute_session(name, &links, closed, minutes, daily_path, resolver)
}

pub(crate) fn attribute_session<S: HomeSource>(
    name: Option<&str>,
    links: &[SessionLink],
    closed: bool,
    minutes: Option<u32>,
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
) -> SessionHomes {
    // R4: session names never choose a home. Every link attributes through
    // the linked task's residence or its `[root:: X]` field.
    let resolved = deduplicated_links(links, daily_path, resolver);

    let state = if closed {
        SessionState::Attributed
    } else {
        SessionState::Forecast
    };
    if resolved.is_empty() {
        let home = unlinked_home();
        let parts = if closed {
            vec![home_part(&home, 0, 100, Some(minutes.unwrap_or(0)))]
        } else {
            Vec::new()
        };
        return SessionHomes {
            homes: Homes {
                state,
                kind: SessionKind::Unlinked,
                label: String::new(),
                chip: None,
                hue: None,
                parts,
            },
            task_homes: Vec::new(),
            lead_home: Some(home),
        };
    }

    let task_homes = resolved
        .into_iter()
        .map(|link| {
            resolver.home_for_link(daily_path, link.resolution, &link.block_id)
        })
        .collect::<Vec<_>>();
    let per_task_minutes =
        closed.then(|| split_minutes(minutes.unwrap_or(0), task_homes.len()));
    let mut grouped = Vec::<HomeGroup>::new();
    let mut indices = HashMap::<String, usize>::new();
    for (index, home) in task_homes.iter().enumerate() {
        let group_index = if let Some(group_index) = indices.get(&home.key) {
            *group_index
        } else {
            let group_index = grouped.len();
            indices.insert(home.key.clone(), group_index);
            grouped.push(HomeGroup {
                home: home.clone(),
                tasks: 0,
                minutes: 0,
                first: index,
            });
            group_index
        };
        let group = &mut grouped[group_index];
        group.tasks += 1;
        if let Some(shares) = &per_task_minutes {
            group.minutes += shares[index];
        }
    }
    let weights = if closed && minutes.is_some() {
        let weights = grouped
            .iter()
            .map(|group| u64::from(group.minutes))
            .collect::<Vec<_>>();
        if weights.iter().sum::<u64>() == 0 {
            grouped.iter().map(|group| u64::from(group.tasks)).collect()
        } else {
            weights
        }
    } else {
        grouped.iter().map(|group| u64::from(group.tasks)).collect()
    };
    let shares = percentages(&weights);
    let parts = grouped
        .iter()
        .zip(shares)
        .map(|(group, share)| {
            home_part(
                &group.home,
                group.tasks,
                share,
                closed.then_some(group.minutes),
            )
        })
        .collect::<Vec<_>>();
    let lead_home = grouped
        .iter()
        .max_by(|left, right| {
            left.tasks
                .cmp(&right.tasks)
                .then_with(|| right.first.cmp(&left.first))
        })
        .map(|group| group.home.clone());
    // R7: the label, chip, and dot name root parents. `parts` and
    // `lead_home` stay per nearest home.
    let roots = root_groups(&grouped, &parts);
    let root_labels = roots
        .iter()
        .map(|root| LabelPart {
            label: root.label.clone(),
            share: root.share,
        })
        .collect::<Vec<_>>();
    let display_label = label(&root_labels);
    let display_chip = chip(name, &root_labels, false);
    let lead_hue = roots
        .iter()
        .max_by(|left, right| {
            left.tasks
                .cmp(&right.tasks)
                .then_with(|| right.first.cmp(&left.first))
        })
        .and_then(|root| root.hue);

    SessionHomes {
        homes: Homes {
            state,
            kind: SessionKind::Work,
            label: display_label,
            chip: display_chip,
            hue: lead_hue,
            parts,
        },
        task_homes,
        lead_home,
    }
}

struct HomeGroup {
    home: Home,
    tasks: u32,
    minutes: u32,
    first: usize,
}

/// Nearest-home parts bucketed by root parent (R7).
struct RootGroup {
    label: String,
    share: u16,
    tasks: u32,
    first: usize,
    hue: Option<Hue>,
}

/// Group `parts` by their home's root parent, in root order: the ledger
/// order of each root's first part. A root's hue is its first part's hue.
fn root_groups(grouped: &[HomeGroup], parts: &[HomePart]) -> Vec<RootGroup> {
    let mut roots = Vec::<(String, RootGroup)>::new();
    for (group, part) in grouped.iter().zip(parts) {
        let key = &group.home.root_parent;
        if let Some((_, root)) = roots.iter_mut().find(|(seen, _)| seen == key)
        {
            root.share += u16::from(part.share);
            root.tasks += group.tasks;
            root.first = root.first.min(group.first);
        } else {
            roots.push((
                key.clone(),
                RootGroup {
                    label: root_label(key),
                    share: u16::from(part.share),
                    tasks: group.tasks,
                    first: group.first,
                    hue: group.home.hue,
                },
            ));
        }
    }
    roots.into_iter().map(|(_, root)| root).collect()
}

fn deduplicated_links<S: HomeSource>(
    links: &[SessionLink],
    daily_path: &Path,
    resolver: &HomeResolver<S>,
) -> Vec<ResolvedLink> {
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    for link in links {
        let resolution = resolver.resolve_target(daily_path, &link.target);
        let identity = match &resolution {
            LinkResolution::Found(path) => {
                LinkIdentity::Found(path_string(path), link.block_id.clone())
            }
            LinkResolution::Ambiguous => LinkIdentity::Ambiguous(
                link.target.clone(),
                link.block_id.clone(),
            ),
            LinkResolution::Missing => LinkIdentity::Missing(
                link.target.clone(),
                link.block_id.clone(),
            ),
        };
        if seen.insert(identity.clone()) {
            resolved.push(ResolvedLink {
                resolution,
                block_id: link.block_id.clone(),
            });
        }
    }
    resolved
}

fn home_part(
    home: &Home,
    tasks: u32,
    share: u8,
    minutes: Option<u32>,
) -> HomePart {
    HomePart {
        key: home.key.clone(),
        label: home.label.clone(),
        kind: home.kind,
        area: Some(home.area.clone()),
        area_label: Some(home.area_label.clone()),
        hue: home.hue,
        tasks,
        share,
        minutes,
    }
}

pub(crate) fn split_minutes(minutes: u32, count: usize) -> Vec<u32> {
    if count == 0 {
        return Vec::new();
    }
    let quotient = minutes / count as u32;
    let remainder = minutes % count as u32;
    (0..count)
        .map(|index| quotient + if index == 0 { remainder } else { 0 })
        .collect()
}

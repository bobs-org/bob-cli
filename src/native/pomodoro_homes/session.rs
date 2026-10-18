use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::Path,
};

use crate::native::{
    capture_pomodoro_start::StartLink, vault_links::LinkResolution,
};

use super::{
    json::{HomePart, Homes},
    label::{chip, label, name_components, percentages},
    resolve::{path_string, unlinked_home},
    Home, HomeResolver, HomeSource, SessionKind, SessionState,
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

pub(crate) fn session_links(
    contents: &str,
    entry_index: usize,
) -> Vec<SessionLink> {
    crate::native::capture_pomodoro_start::session_links(contents, entry_index)
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
    minutes: Option<u32>,
    exempt: &[String],
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
) -> SessionHomes {
    let links = session_links(contents, entry_line.saturating_sub(1));
    attribute_session(name, &links, minutes, exempt, daily_path, resolver)
}

pub(crate) fn attribute_session<S: HomeSource>(
    name: Option<&str>,
    links: &[SessionLink],
    minutes: Option<u32>,
    exempt: &[String],
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
) -> SessionHomes {
    let resolved = deduplicated_links(links, daily_path, resolver);
    let exempt_names = exempt
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let components = name_components(name);
    let named_review = name.is_some()
        && !components.is_empty()
        && components
            .iter()
            .all(|component| exempt_names.contains(component));
    let bridge = name.is_none()
        && !exempt.is_empty()
        && resolved.iter().any(|link| {
            link.block_id.eq_ignore_ascii_case("gtd")
                && matches!(&link.resolution, LinkResolution::Found(path)
                    if path == daily_path
                        && crate::native::task_status_hooks::canonical_daily_date(path)
                            .is_some())
        });

    if named_review || bridge {
        let names = if named_review {
            raw_name_components(name)
        } else {
            vec![exempt[0].clone()]
        };
        return review_session(
            name,
            &names,
            minutes,
            named_review,
            daily_path,
            resolver,
        );
    }

    let state = if minutes.is_some() {
        SessionState::Attributed
    } else {
        SessionState::Forecast
    };
    if resolved.is_empty() {
        let home = unlinked_home();
        let parts = minutes
            .map(|minutes| vec![home_part(&home, 0, 100, Some(minutes))])
            .unwrap_or_default();
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

    let bridge_name = exempt.first().map(String::as_str);
    let task_homes = resolved
        .into_iter()
        .map(|link| {
            resolver.home_for_link(
                daily_path,
                link.resolution,
                &link.block_id,
                bridge_name,
            )
        })
        .collect::<Vec<_>>();
    let per_task_minutes =
        minutes.map(|minutes| split_minutes(minutes, task_homes.len()));
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
    let weights = if minutes.is_some() {
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
                minutes.map(|_| group.minutes),
            )
        })
        .collect::<Vec<_>>();
    let display_label = label(&parts);
    let lead_home = grouped
        .iter()
        .max_by(|left, right| {
            left.tasks
                .cmp(&right.tasks)
                .then_with(|| right.first.cmp(&left.first))
        })
        .map(|group| group.home.clone());
    let lead_hue = lead_home.as_ref().and_then(|home| home.hue);
    let display_chip = chip(name, &parts, false, false);

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

fn review_session<S: HomeSource>(
    name: Option<&str>,
    names: &[String],
    minutes: Option<u32>,
    named_review: bool,
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
) -> SessionHomes {
    let state = if minutes.is_some() {
        SessionState::Attributed
    } else {
        SessionState::Forecast
    };
    let minute_shares =
        minutes.map(|minutes| split_minutes(minutes, names.len()));
    let mut weights = names
        .iter()
        .enumerate()
        .map(|(index, _)| {
            minute_shares
                .as_ref()
                .map_or(1, |shares| u64::from(shares[index]))
        })
        .collect::<Vec<_>>();
    if weights.iter().sum::<u64>() == 0 {
        weights.fill(1);
    }
    let shares = percentages(&weights);
    let mut homes = Vec::new();
    let mut parts = Vec::new();
    for (index, review_name) in names.iter().enumerate() {
        let home = resolver.review_home(review_name, daily_path);
        parts.push(home_part(
            &home,
            0,
            shares[index],
            minute_shares.as_ref().map(|shares| shares[index]),
        ));
        homes.push(home);
    }
    let display_label = label(&parts);
    let lead_hue = homes.first().and_then(|home| home.hue);
    let lead_home = homes.first().cloned();
    let display_chip = if named_review {
        None
    } else {
        chip(name, &parts, false, false)
    };
    SessionHomes {
        homes: Homes {
            state,
            kind: SessionKind::Review,
            label: display_label,
            chip: display_chip,
            hue: lead_hue,
            parts,
        },
        task_homes: Vec::new(),
        lead_home,
    }
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

fn raw_name_components(name: Option<&str>) -> Vec<String> {
    name.into_iter()
        .flat_map(|name| name.split('+'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
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
        .map(|index| quotient + u32::from(index < remainder as usize))
        .collect()
}

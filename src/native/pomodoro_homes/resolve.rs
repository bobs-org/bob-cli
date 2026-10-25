use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use super::{facts::NoteFacts, palette::Hue, NoteKind};
use crate::native::vault_links::{
    LinkResolution, NoteIndex, VaultLinkResolver,
};

const MAX_PROJECT_DEPTH: usize = 16;

pub(crate) trait HomeSource {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution;
    fn read_facts(&self, path: &Path) -> Option<NoteFacts>;
}

/// Stats backend over one already-indexed pass of the vault.
pub(crate) struct IndexedHomeSource<'a> {
    note_index: &'a NoteIndex,
    contents: HashMap<PathBuf, &'a str>,
}

impl<'a> IndexedHomeSource<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        note_index: &'a NoteIndex,
        contents: &'a std::collections::BTreeMap<PathBuf, String>,
    ) -> Self {
        Self {
            note_index,
            contents: contents
                .iter()
                .map(|(path, contents)| (path.clone(), contents.as_str()))
                .collect(),
        }
    }

    /// Borrow the `(vault-relative path, markdown buffer)` rows already
    /// materialized by the Stats index pass.
    pub(crate) fn from_buffers(
        note_index: &'a NoteIndex,
        buffers: &'a [(String, String)],
    ) -> Self {
        Self {
            note_index,
            contents: buffers
                .iter()
                .map(|(path, contents)| {
                    (PathBuf::from(path), contents.as_str())
                })
                .collect(),
        }
    }
}

impl HomeSource for IndexedHomeSource<'_> {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution {
        self.note_index.resolve_detailed(Some(current), target)
    }

    fn read_facts(&self, path: &Path) -> Option<NoteFacts> {
        self.contents
            .get(path)
            .map(|contents| NoteFacts::parse(contents))
    }
}

/// Live-surface backend: exact path first, then the shared lazy basename
/// resolver, with frontmatter read only for notes reached by a link chain.
pub(crate) struct DiskHomeSource {
    root: PathBuf,
    links: VaultLinkResolver,
}

impl DiskHomeSource {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            links: VaultLinkResolver::new(&root),
            root,
        }
    }
}

impl HomeSource for DiskHomeSource {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution {
        self.links.resolve(current, target)
    }

    fn read_facts(&self, path: &Path) -> Option<NoteFacts> {
        let contents = fs::read_to_string(self.root.join(path)).ok()?;
        Some(NoteFacts::parse(&contents))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Home {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: super::HomeKind,
    pub(crate) area: String,
    pub(crate) area_label: String,
    pub(crate) subarea: Option<String>,
    pub(crate) hue: Option<Hue>,
    /// Full residence chain from the linked note through its ancestors.
    pub(crate) residence_chain: Vec<String>,
    /// Root parent grouping for stats: the area the task lives in, else
    /// the top-level project above its project note. Buckets are their
    /// own root.
    pub(crate) root_parent: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteLint {
    pub(crate) path: String,
    pub(crate) raw: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolverHealth {
    pub(crate) unfiled_links: u32,
    pub(crate) ambiguous_links: u32,
    pub(crate) unresolved_links: u32,
    pub(crate) area_color_invalid: Vec<NoteLint>,
}

pub(crate) struct HomeResolver<S> {
    source: S,
    homes: HashMap<PathBuf, Home>,
    facts: HashMap<PathBuf, Option<NoteFacts>>,
    color_linted: BTreeSet<PathBuf>,
    health: ResolverHealth,
}

impl<S: HomeSource> HomeResolver<S> {
    pub(crate) fn new(source: S) -> Self {
        Self {
            source,
            homes: HashMap::new(),
            facts: HashMap::new(),
            color_linted: BTreeSet::new(),
            health: ResolverHealth::default(),
        }
    }

    pub(crate) fn health(&self) -> &ResolverHealth {
        &self.health
    }

    pub(crate) fn facts(&mut self, path: &Path) -> Option<NoteFacts> {
        self.facts_for(path)
    }

    pub(crate) fn resolve_target(
        &self,
        current: &Path,
        target: &str,
    ) -> LinkResolution {
        self.source.resolve(current, target)
    }

    pub(crate) fn home_for_link(
        &mut self,
        current: &Path,
        resolution: LinkResolution,
        block_id: &str,
        bridge_name: Option<&str>,
    ) -> Home {
        match resolution {
            LinkResolution::Found(path) => {
                if block_id.eq_ignore_ascii_case("gtd")
                    && crate::native::task_status_hooks::canonical_daily_date(
                        &path,
                    )
                    .is_some()
                {
                    return self
                        .review_home(bridge_name.unwrap_or("GTD"), current);
                }
                let home = self.resolve_note(&path);
                if home.key == "@unfiled" {
                    self.health.unfiled_links += 1;
                }
                home
            }
            LinkResolution::Ambiguous => {
                self.health.ambiguous_links += 1;
                self.health.unfiled_links += 1;
                unfiled_home()
            }
            LinkResolution::Missing => {
                self.health.unresolved_links += 1;
                self.health.unfiled_links += 1;
                unfiled_home()
            }
        }
    }

    pub(crate) fn resolve_note(&mut self, path: &Path) -> Home {
        if let Some(home) = self.homes.get(path) {
            return home.clone();
        }

        let mut chain = Vec::<(PathBuf, NoteFacts)>::new();
        let mut visited = BTreeSet::new();
        let mut current = path.to_path_buf();
        for _ in 0..MAX_PROJECT_DEPTH {
            if !visited.insert(current.clone()) {
                break;
            }
            let Some(facts) = self.facts_for(&current) else {
                break;
            };
            let parent = facts.parent.clone();
            chain.push((current.clone(), facts));
            let Some(parent) = parent else {
                break;
            };
            match self.source.resolve(&current, &parent) {
                LinkResolution::Found(parent_path)
                    if parent_path != current =>
                {
                    current = parent_path;
                }
                _ => break,
            }
        }

        let nearest_home = chain.iter().position(|(_, facts)| {
            matches!(facts.kind, NoteKind::Area | NoteKind::Project)
        });
        let Some(nearest_home) = nearest_home else {
            let home = unfiled_at(
                chain.iter().map(|(path, _)| path_string(path)).collect(),
            );
            self.homes.insert(path.to_path_buf(), home.clone());
            return home;
        };

        // The home is the nearest area or project. The full residence
        // chain stays attached to the home for later `in:` filtering.
        let (home_path, home_facts) = &chain[nearest_home];

        let area_indices = chain
            .iter()
            .enumerate()
            .filter(|(_, (_, facts))| facts.kind == NoteKind::Area)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let top_area_index = area_indices.last().copied();
        let top_area = top_area_index.map(|index| &chain[index].0);
        let subarea = area_indices
            .first()
            .filter(|index| Some(**index) != top_area_index)
            .map(|index| path_string(&chain[*index].0));
        let (area, area_label, hue) = if let Some(area_path) = top_area {
            let area_facts = &chain[top_area_index.expect("top area")].1;
            let stem = note_stem(area_path);
            let explicit_hue = area_facts.color.as_deref().and_then(Hue::parse);
            if area_facts.color.is_some() && explicit_hue.is_none() {
                self.lint_color(
                    area_path,
                    area_facts.color.as_deref().unwrap_or(""),
                );
            }
            let hue =
                explicit_hue.unwrap_or_else(|| super::fallback_hue(&stem));
            (path_string(area_path), stem, Some(hue))
        } else {
            ("@unfiled".to_string(), "unfiled".to_string(), None)
        };

        let root_parent = match home_facts.kind {
            NoteKind::Area => path_string(home_path),
            NoteKind::Project => {
                let end = chain[nearest_home + 1..]
                    .iter()
                    .position(|(_, facts)| facts.kind == NoteKind::Area)
                    .map(|offset| nearest_home + 1 + offset)
                    .unwrap_or(chain.len());
                chain[nearest_home..end]
                    .iter()
                    .filter(|(_, facts)| facts.kind == NoteKind::Project)
                    .last()
                    .map(|(path, _)| path_string(path))
                    .unwrap_or_else(|| path_string(home_path))
            }
            NoteKind::Other => unreachable!("home kind was selected"),
        };

        let home = Home {
            key: path_string(home_path),
            label: note_stem(home_path),
            kind: match home_facts.kind {
                NoteKind::Project => super::HomeKind::Project,
                NoteKind::Area => super::HomeKind::Area,
                NoteKind::Other => unreachable!("home kind was selected"),
            },
            area,
            area_label,
            subarea,
            hue,
            residence_chain: chain
                .iter()
                .map(|(path, _)| path_string(path))
                .collect(),
            root_parent,
        };

        self.homes.insert(path.to_path_buf(), home.clone());
        home
    }

    pub(crate) fn review_home(&mut self, name: &str, current: &Path) -> Home {
        let bucket = format!("@review:{name}");
        let target = name.to_ascii_lowercase();
        let reviewed = match self.source.resolve(current, &target) {
            LinkResolution::Found(path) => Some(self.resolve_note(&path)),
            LinkResolution::Ambiguous | LinkResolution::Missing => None,
        };
        let residence_chain = reviewed
            .as_ref()
            .map(|home| home.residence_chain.clone())
            .unwrap_or_default();
        let (area, area_label, hue) = reviewed
            .filter(|home| home.area != "@unfiled")
            .map(|home| (home.area, home.area_label, home.hue))
            .unwrap_or_else(|| {
                (bucket.clone(), format!("{name} review"), None)
            });
        Home {
            key: bucket.clone(),
            label: format!("{name} review"),
            kind: super::HomeKind::Bucket,
            area,
            area_label,
            subarea: None,
            hue,
            residence_chain,
            root_parent: bucket,
        }
    }

    fn facts_for(&mut self, path: &Path) -> Option<NoteFacts> {
        if let Some(facts) = self.facts.get(path) {
            return facts.clone();
        }
        let facts = self.source.read_facts(path);
        self.facts.insert(path.to_path_buf(), facts.clone());
        facts
    }

    fn lint_color(&mut self, path: &Path, raw: &str) {
        if self.color_linted.insert(path.to_path_buf()) {
            self.health.area_color_invalid.push(NoteLint {
                path: path_string(path),
                raw: raw.to_string(),
            });
        }
    }
}

pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn note_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_lowercase()
}

pub(crate) fn unfiled_home() -> Home {
    unfiled_at(Vec::new())
}

fn unfiled_at(residence_chain: Vec<String>) -> Home {
    Home {
        key: "@unfiled".to_string(),
        label: "unfiled".to_string(),
        kind: super::HomeKind::Bucket,
        area: "@unfiled".to_string(),
        area_label: "unfiled".to_string(),
        subarea: None,
        hue: None,
        residence_chain,
        root_parent: "@unfiled".to_string(),
    }
}

pub(crate) fn unlinked_home() -> Home {
    Home {
        key: "@unlinked".to_string(),
        label: "unlinked".to_string(),
        kind: super::HomeKind::Bucket,
        area: "@unlinked".to_string(),
        area_label: "unlinked".to_string(),
        subarea: None,
        hue: None,
        residence_chain: Vec::new(),
        root_parent: "@unlinked".to_string(),
    }
}

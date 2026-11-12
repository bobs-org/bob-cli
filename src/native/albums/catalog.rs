//! Pure planner for generated `## Albums` catalogs in area and project notes.
//!
//! The planner consumes the prepared vault snapshot and the album registry.
//! It performs no filesystem reads or writes; callers can use the resulting
//! host records and byte edits for reports, sync, or an in-memory overlay.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{registry::AlbumRegistry, report::AlbumIssue, AlbumIssueSeverity};
use crate::native::{
    host_catalog::{
        self, EditState, FileEdit, HostOutcome, ReadDependencies, ALBUMS,
    },
    notebooks::{
        catalog::inert_text,
        snapshot::{display_slash, LiteKind, VaultSnapshot},
    },
    pomodoro_homes::{HomeResolver, IndexedHomeSource},
    vault_links::NoteIndex,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogState {
    Current,
    Missing,
    Stale,
    Damaged,
}

impl CatalogState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Damaged => "damaged",
        }
    }
}

/// One area or project host's generated Albums catalog report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumCatalogRecord {
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) root: Option<String>,
    pub(crate) state: CatalogState,
    pub(crate) albums: usize,
    pub(crate) issues: Vec<AlbumIssue>,
}

impl AlbumCatalogRecord {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "path": self.path,
            "root": self.root,
            "state": self.state.as_str(),
            "albums": self.albums,
            "issues": self.issues.iter().map(AlbumIssue::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone)]
struct CatalogHost {
    path: PathBuf,
    stem: String,
    link: String,
    root: Option<String>,
    contents: String,
}

#[derive(Debug, Clone)]
struct CatalogAlbum {
    path: PathBuf,
    link: String,
    title: String,
    emoji: Option<String>,
    summary: Option<String>,
    owner: PathBuf,
    hosts: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct CatalogGraph {
    hosts: BTreeMap<PathBuf, CatalogHost>,
    albums: BTreeMap<PathBuf, CatalogAlbum>,
}

#[derive(Debug, Clone)]
pub(crate) struct HostPlan {
    pub(crate) record: AlbumCatalogRecord,
    pub(crate) edit: Option<FileEdit>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogPlan {
    pub(crate) hosts: Vec<HostPlan>,
    album_hosts: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl CatalogPlan {
    pub(crate) fn records(&self) -> Vec<AlbumCatalogRecord> {
        self.hosts.iter().map(|host| host.record.clone()).collect()
    }

    pub(crate) fn edits(&self) -> Vec<&FileEdit> {
        self.hosts
            .iter()
            .filter_map(|host| host.edit.as_ref())
            .collect()
    }

    pub(crate) fn host(&self, path: &Path) -> Option<&HostPlan> {
        self.hosts
            .iter()
            .find(|host| Path::new(&host.record.path) == path)
    }

    /// Owner-chain hosts that gain a row for one album, through its root
    /// inclusive. Album creation stages only these hosts so unrelated stale
    /// catalogs are never repaired as a side effect.
    pub(crate) fn hosts_for_album(&self, album_path: &Path) -> Vec<PathBuf> {
        self.album_hosts
            .get(album_path)
            .cloned()
            .unwrap_or_default()
    }
}

/// Plan every host with expected rows or an existing Albums region.
pub(crate) fn plan_catalogs(
    snapshot: &VaultSnapshot,
    registry: &AlbumRegistry,
) -> CatalogPlan {
    let graph = build_graph(snapshot, registry);
    let mut hosts = Vec::new();

    for host in graph.hosts.values() {
        let members = graph
            .albums
            .values()
            .filter(|album| album.hosts.contains(&host.path))
            .collect::<Vec<_>>();
        let rows = render_rows(&graph, host, &members);
        let callout = if rows.is_empty() {
            Vec::new()
        } else {
            let mut lines = vec!["> [!albums]".to_owned(), ">".to_owned()];
            lines.extend(rows);
            lines
        };
        let planned = host_catalog::plan_host_region(
            host.path.clone(),
            &host.contents,
            &ALBUMS,
            &callout,
        );
        let path = display_slash(&host.path);
        let mut record = AlbumCatalogRecord {
            host: host.stem.clone(),
            path: path.clone(),
            root: host.root.clone(),
            state: CatalogState::Current,
            albums: members.len(),
            issues: Vec::new(),
        };
        let mut edit = None;
        match planned.outcome {
            HostOutcome::Nothing => continue,
            HostOutcome::Current => {}
            HostOutcome::Update { state, .. } => {
                record.state = match state {
                    EditState::Missing => CatalogState::Missing,
                    EditState::Stale => CatalogState::Stale,
                };
                record.issues.push(catalog_issue(
                    "album_catalog_stale",
                    AlbumIssueSeverity::Warning,
                    &path,
                    None,
                    format!(
                        "the generated album catalog in {path} is {}; run `bob albums sync`",
                        record.state.as_str()
                    ),
                ));
                edit = planned.edit;
            }
            HostOutcome::Damaged(damage) => {
                record.state = CatalogState::Damaged;
                record.issues.push(catalog_issue(
                    "album_catalog_damaged",
                    AlbumIssueSeverity::Error,
                    &path,
                    Some(damage.line),
                    format!(
                        "{path}: {}; repair the old markers by hand, then run `bob albums sync`",
                        damage.message
                    ),
                ));
            }
        }
        hosts.push(HostPlan { record, edit });
    }
    let mut album_hosts = BTreeMap::new();
    for album in graph.albums.values() {
        album_hosts.insert(album.path.clone(), album.hosts.clone());
    }
    CatalogPlan { hosts, album_hosts }
}

/// Owner, ancestor, and declaration preimages catalog rows actually use,
/// plus vault-root Markdown membership so a concurrent new or removed album
/// is noticed before install.
pub(crate) fn catalog_read_dependencies(
    snapshot: &VaultSnapshot,
    registry: &AlbumRegistry,
) -> ReadDependencies {
    let mut dependencies = ReadDependencies::default();
    for album in &registry.albums {
        if let Some(contents) = &album.contents {
            dependencies.insert(
                PathBuf::from(&album.declaration.path),
                Some(contents.clone()),
            );
        }
    }
    for (path, file) in &snapshot.files {
        if !matches!(file.lite.kind, LiteKind::Area | LiteKind::Project { .. })
        {
            continue;
        }
        if let Some(contents) = snapshot.contents(path) {
            dependencies.insert(path.clone(), Some(contents.to_owned()));
        }
    }
    let members = snapshot
        .discovered
        .iter()
        .filter(|path| is_vault_root_markdown(path))
        .cloned()
        .collect::<Vec<_>>();
    dependencies.insert_directory_membership(PathBuf::new(), Some(members));
    dependencies
}

fn is_vault_root_markdown(path: &Path) -> bool {
    path.components().count() == 1
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn build_graph(
    snapshot: &VaultSnapshot,
    registry: &AlbumRegistry,
) -> CatalogGraph {
    let index = snapshot.index();
    let buffers = snapshot
        .files
        .iter()
        .map(|(path, file)| (display_slash(path), file.header.clone()))
        .collect::<Vec<_>>();
    let source = IndexedHomeSource::from_buffers(&index, &buffers);
    let mut resolver = HomeResolver::new(source);
    let mut graph = CatalogGraph::default();

    for (path, file) in &snapshot.files {
        if !matches!(file.lite.kind, LiteKind::Area | LiteKind::Project { .. })
        {
            continue;
        }
        let Some(contents) = snapshot.contents(path) else {
            continue;
        };
        let home = resolver.resolve_note(path);
        graph.hosts.insert(
            path.clone(),
            CatalogHost {
                path: path.clone(),
                stem: file.lite.stem.clone(),
                link: link_destination(&index, path),
                root: (home.root_parent != "@unfiled")
                    .then(|| stem_of_path_str(&home.root_parent)),
                contents: contents.to_owned(),
            },
        );
    }

    for entry in &registry.albums {
        // Ownership is structural and independent of schema usability. A
        // schema-invalid album with a resolved owner still gets a catalog row.
        let Some(owner) = entry.parent_path.as_ref() else {
            continue;
        };
        let path = PathBuf::from(&entry.declaration.path);
        let home = resolver.resolve_note(&path);
        if home.key == "@unfiled" || home.root_parent == "@unfiled" {
            continue;
        }
        let chain = home
            .residence_chain
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let Some(owner_at) = chain.iter().position(|path| path == owner) else {
            continue;
        };
        let root_path = PathBuf::from(&home.root_parent);
        let Some(root_at) = chain.iter().position(|path| *path == root_path)
        else {
            continue;
        };
        if root_at < owner_at || !graph.hosts.contains_key(owner) {
            continue;
        }
        let hosts = chain[owner_at..=root_at]
            .iter()
            .filter(|path| graph.hosts.contains_key(path.as_path()))
            .cloned()
            .collect::<Vec<_>>();
        if !hosts.contains(owner) {
            continue;
        }
        let summary = entry
            .declaration
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(str::to_owned);
        graph.albums.insert(
            path.clone(),
            CatalogAlbum {
                path: path.clone(),
                link: link_destination(&index, &path),
                title: entry.declaration.title.clone(),
                emoji: entry.declaration.emoji.clone(),
                summary,
                owner: owner.clone(),
                hosts,
            },
        );
    }

    graph
}

fn render_rows(
    graph: &CatalogGraph,
    host: &CatalogHost,
    members: &[&CatalogAlbum],
) -> Vec<String> {
    let mut rows = Vec::new();
    let mut direct = members
        .iter()
        .copied()
        .filter(|album| album.owner == host.path)
        .collect::<Vec<_>>();
    sort_albums(&mut direct);
    rows.extend(direct.iter().map(|album| album_row(album, 0)));

    let mut owners = BTreeSet::new();
    for album in members {
        if album.owner != host.path
            && let Some(owner) = graph.hosts.get(&album.owner)
        {
            owners.insert((owner.stem.to_lowercase(), owner.path.clone()));
        }
    }
    for (_, owner_path) in owners {
        let Some(owner) = graph.hosts.get(&owner_path) else {
            continue;
        };
        rows.push(format!("> - 🧩 {}", group_link(owner)));
        let mut group = members
            .iter()
            .copied()
            .filter(|album| album.owner == owner_path)
            .collect::<Vec<_>>();
        sort_albums(&mut group);
        rows.extend(group.iter().map(|album| album_row(album, 1)));
    }
    rows
}

fn sort_albums(albums: &mut [&CatalogAlbum]) {
    albums.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
}

fn album_row(album: &CatalogAlbum, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let emoji = album
        .emoji
        .as_deref()
        .map(|emoji| format!("{} ", inert_text(emoji)))
        .unwrap_or_default();
    let mut row = format!(
        "> {indent}- {emoji}**[[{}|{}]]**",
        album.link,
        inert_text(&album.title)
    );
    if let Some(summary) = &album.summary {
        let summary = inert_text(summary);
        if !summary.is_empty() {
            row.push_str(" — ");
            row.push_str(&summary);
        }
    }
    row
}

fn group_link(owner: &CatalogHost) -> String {
    if owner.link == owner.stem {
        format!("[[{}]]", owner.stem)
    } else {
        format!("[[{}|{}]]", owner.link, inert_text(&owner.stem))
    }
}

fn link_destination(index: &NoteIndex, path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if index.resolve_detailed(None, stem).path() == Some(path) {
        return stem.to_owned();
    }
    let slashed = display_slash(path);
    slashed
        .strip_suffix(".md")
        .or_else(|| slashed.strip_suffix(".MD"))
        .unwrap_or(&slashed)
        .to_owned()
}

fn stem_of_path_str(value: &str) -> String {
    let trimmed = value.trim();
    let without_suffix = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    Path::new(without_suffix)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| without_suffix.to_owned())
}

fn catalog_issue(
    code: &'static str,
    severity: AlbumIssueSeverity,
    path: &str,
    line: Option<usize>,
    message: String,
) -> AlbumIssue {
    AlbumIssue {
        code,
        severity,
        path: path.to_owned(),
        line,
        key_path: None,
        message,
        suggestions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(vault: &Path, relative: &str, contents: &str) {
        let path = vault.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn album(
        parent: &str,
        title: &str,
        summary: &str,
        noun: &str,
        emoji: &str,
    ) -> String {
        format!(
            "---\ntype: [[album]]\nparent: \"[[{parent}]]\"\nsummary: \"{summary}\"\nemoji: \"{emoji}\"\nrecord:\n  noun: {noun}\n---\n\n# {title}\n"
        )
    }

    fn project(parent: &str) -> String {
        format!("---\ntype: [[project]]\nparent: \"[[{parent}]]\"\n---\n\n# Project\n")
    }

    fn report_plan(vault: &Path) -> CatalogPlan {
        let config = vault.join("missing-config.yml");
        let snapshot = VaultSnapshot::load_with_config(vault, &config);
        assert!(snapshot.io_errors.is_empty(), "{:?}", snapshot.io_errors);
        let registry = AlbumRegistry::from_snapshot(vault, &config, &snapshot);
        plan_catalogs(&snapshot, &registry)
    }

    #[test]
    fn rows_sort_direct_albums_then_path_qualified_owner_groups() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        write(vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
        write(
            vault,
            "program.md",
            &format!(
                "{}\n## Notebooks\n\n> [!notebooks]\n>\n> - existing\n\n## Tasks\n",
                project("body")
            ),
        );
        write(vault, "groups/alpha.md", &project("program"));
        write(vault, "other/alpha.md", &project("program"));
        write(
            vault,
            "alpha_album.md",
            &album(
                "program",
                "A *title*",
                "Read [[raw]] &amp; text.",
                "alpha_record",
                "📚",
            ),
        );
        write(
            vault,
            "a_tie.md",
            &album("program", "Tie", "First tie.", "a_tie_record", ""),
        );
        write(
            vault,
            "z_tie.md",
            &album("program", "tie", "Second tie.", "z_tie_record", ""),
        );
        write(
            vault,
            "group_album.md",
            &album(
                "groups/alpha",
                "Group Album",
                "In the first group.",
                "group_record",
                "🎬",
            ),
        );
        write(
            vault,
            "other_album.md",
            &album(
                "other/alpha",
                "Other Album",
                "In the second group.",
                "other_record",
                "",
            ),
        );

        let plan = report_plan(vault);
        let host = plan.host(Path::new("program.md")).expect("program host");
        assert_eq!(host.record.state, CatalogState::Missing);
        let after = &host.edit.as_ref().unwrap().after;
        let alpha = after
            .find("[[alpha_album|A title]]")
            .unwrap_or_else(|| panic!("missing alpha row in:\n{after}"));
        let first_tie = after.find("[[a_tie|Tie]]").unwrap();
        let second_tie = after.find("[[z_tie|tie]]").unwrap();
        let first_group = after.find("> - 🧩 [[groups/alpha|alpha]]").unwrap();
        let second_group = after.find("> - 🧩 [[other/alpha|alpha]]").unwrap();
        assert!(alpha < first_tie && first_tie < second_tie);
        assert!(second_tie < first_group && first_group < second_group);
        assert!(
            after.contains("Read \\[\\[raw\\]\\] \\&amp; text."),
            "{after}"
        );
        assert!(after.contains(">   - 🎬 **[[group_album|Group Album]]**"));
        assert!(
            after.find("## Albums").unwrap() < after.find("## Tasks").unwrap(),
            "{after}"
        );
    }

    #[test]
    fn overlay_reparent_keeps_the_old_owner_as_a_removal_plan() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        write(vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
        write(vault, "mind.md", "---\ntype: [[area]]\n---\n\n# Mind\n");
        let album_before =
            album("body", "Eating out", "Places.", "restaurant", "🍽️");
        write(vault, "eat.md", &album_before);

        let config = vault.join("missing-config.yml");
        let snapshot = VaultSnapshot::load_with_config(vault, &config);
        let registry = AlbumRegistry::from_snapshot(vault, &config, &snapshot);
        let initial = plan_catalogs(&snapshot, &registry);
        let body_after_first_sync = initial
            .host(Path::new("body.md"))
            .and_then(|host| host.edit.as_ref())
            .expect("first owner gets a catalog")
            .after
            .clone();
        let body_before =
            snapshot.contents(Path::new("body.md")).unwrap().to_owned();
        let album_after = album_before.replace("[[body]]", "[[mind]]");
        let overlay = snapshot.with_overlay([
            (PathBuf::from("body.md"), body_after_first_sync),
            (PathBuf::from("eat.md"), album_after),
        ]);
        let registry = AlbumRegistry::from_snapshot(vault, &config, &overlay);
        let moved = plan_catalogs(&overlay, &registry);

        let old_owner = moved
            .host(Path::new("body.md"))
            .expect("old owner retained");
        assert_eq!(old_owner.record.state, CatalogState::Stale);
        assert_eq!(old_owner.record.albums, 0);
        assert_eq!(old_owner.edit.as_ref().unwrap().after, body_before);
        let new_owner =
            moved.host(Path::new("mind.md")).expect("new owner added");
        assert_eq!(new_owner.record.state, CatalogState::Missing);
        assert_eq!(new_owner.record.albums, 1);
        assert!(new_owner
            .edit
            .as_ref()
            .unwrap()
            .after
            .contains("[[eat|Eating out]]"));
    }
}

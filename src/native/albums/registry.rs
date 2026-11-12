//! Vault-scoped album claims and record-type resolution.
//!
//! [`AlbumRegistry::discover`] performs the complete scan used for health and
//! reports. [`AlbumRegistry::resolve_route`] only consults that already-known
//! inventory. [`resolve_route_head`] is the bounded adapter for a caller that
//! has no inventory: it reads the requested root note's frontmatter only, and
//! performs only the requested route's bounded album lookup.
//!
//! A consumer that is producing whole-vault health should keep the discovered
//! registry and use its `albums`, `usable_types`, and `health` together. For
//! example, `AlbumInvalid` retains the album paths and findings even when no
//! `RecordType` could be built. A capture path can use
//! `resolve_route_head(vault, "movies", Some(&registry))` to reuse
//! those checks. Without an inventory, `TargetOnly` means the schema at that
//! head parsed locally, while global uniqueness and parent checks have not run.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use crate::native::{
    albums::{
        confined_album_route, parse_album_declaration, AlbumDeclaration,
        AlbumFinding, AlbumIssueSeverity,
    },
    notebooks::{
        rules,
        snapshot::{
            display_slash, LiteKind, NoteFactsLite, SnapshotFile, VaultSnapshot,
        },
    },
    pomodoro_homes::{HomeResolver, IndexedHomeSource},
    projects::wikilink_path_target,
    records::schema::{PropertyStorageKind, RecordType},
    vault_links::{LinkResolution, NoteIndex},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordTypeSource {
    Album { path: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourcedRecordType {
    pub(crate) record_type: RecordType,
    pub(crate) source: RecordTypeSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiscoveryHealth {
    Complete,
    Partial(Vec<String>),
    #[allow(dead_code)]
    TargetOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AlbumEntry {
    pub(crate) declaration: AlbumDeclaration,
    pub(crate) parent_path: Option<PathBuf>,
    pub(crate) parent_kind: Option<&'static str>,
    pub(crate) owner: Option<String>,
    pub(crate) root: Option<String>,
    pub(crate) breadcrumb: Vec<String>,
    pub(crate) findings: Vec<AlbumFinding>,
    pub(crate) usable_type: Option<SourcedRecordType>,
    /// Original album bytes from discovery, used to plan the records region.
    pub(crate) contents: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum RouteResolution {
    AlbumValid {
        record_type: SourcedRecordType,
        health: DiscoveryHealth,
    },
    AlbumInvalid {
        paths: Vec<String>,
        findings: Vec<AlbumFinding>,
        health: DiscoveryHealth,
    },
    Unclaimed {
        health: DiscoveryHealth,
    },
    DiscoveryIncomplete {
        errors: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct AlbumRegistry {
    pub(crate) albums: Vec<AlbumEntry>,
    #[allow(dead_code)]
    pub(crate) usable_types: Vec<SourcedRecordType>,
    pub(crate) health: DiscoveryHealth,
    pub(crate) vault_root: PathBuf,
    #[allow(dead_code)]
    claims: BTreeMap<String, Vec<usize>>,
}

impl AlbumRegistry {
    /// Discover album declarations from one prepared snapshot of the
    /// effective vault (`-b` included).
    pub(crate) fn from_snapshot(
        bob_dir: &Path,
        snapshot: &VaultSnapshot,
    ) -> Self {
        let index = snapshot.index();
        let files = &snapshot.files;
        let mut declarations = snapshot
            .album_declarations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        declarations.sort_by(|left, right| left.path.cmp(&right.path));

        let mut buffers = Vec::new();
        for (path, file) in files {
            buffers.push((display_slash(path), file.header.clone()));
        }
        let source = IndexedHomeSource::from_buffers(&index, &buffers);
        let mut resolver = HomeResolver::new(source);

        let mut albums = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let path = PathBuf::from(&declaration.path);
            let file = files.get(&path);
            let mut findings = declaration.findings.clone();
            let mut parent_path = None;
            let mut parent_kind = None;
            let mut owner = None;
            let mut root = None;
            let mut breadcrumb = vec![declaration.title.clone()];

            if path.components().count() > 1 {
                push_finding(
                    &mut findings,
                    "album_not_at_root",
                    AlbumIssueSeverity::Error,
                    &declaration.path,
                    None,
                    None,
                    "albums must live at the vault root; record folders use the album stem",
                );
            }
            if let Some(file) = file {
                add_checkbox_findings(
                    &mut findings,
                    &declaration.path,
                    file.contents.as_deref(),
                );
                (parent_path, parent_kind) = analyze_direct_parent(
                    &declaration,
                    &file.lite,
                    &index,
                    files,
                    &mut findings,
                );
                if parent_path.is_some()
                    && has_parent_cycle(&path, &declaration, files, &index)
                {
                    push_finding(
                        &mut findings,
                        "album_cycle",
                        AlbumIssueSeverity::Error,
                        &declaration.path,
                        file.lite.parent_line,
                        Some("parent".to_owned()),
                        "the album's parent chain cycles; break the parent loop",
                    );
                    parent_path = None;
                    parent_kind = None;
                }
                if let Some(parent) = parent_path.as_ref() {
                    owner = parent
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(str::to_owned);
                    let home = resolver.resolve_note(&path);
                    if home.key != "@unfiled" {
                        root = Some(stem_of_path(&home.root_parent));
                        breadcrumb = album_breadcrumb(
                            &home.residence_chain,
                            &home.root_parent,
                            &path,
                            files,
                            &snapshot.album_declarations,
                        );
                    }
                }
            }
            albums.push(AlbumEntry {
                declaration,
                parent_path,
                parent_kind,
                owner,
                root,
                breadcrumb,
                findings,
                usable_type: None,
                contents: file.and_then(|file| file.contents.clone()),
            });
        }

        add_child_reparent_findings(&mut albums, files, &index);
        let claims = build_claims(&albums);
        add_claim_conflicts(&mut albums, &claims);
        add_property_conflicts(&mut albums);
        let health = if snapshot.io_errors.is_empty() {
            DiscoveryHealth::Complete
        } else {
            DiscoveryHealth::Partial(snapshot.io_errors.clone())
        };

        let mut usable_types = Vec::new();
        for album in &mut albums {
            if has_errors(&album.findings) {
                continue;
            }
            let Some(record_type) = album.declaration.record_type.clone()
            else {
                continue;
            };
            let sourced = SourcedRecordType {
                record_type,
                source: RecordTypeSource::Album {
                    path: album.declaration.path.clone(),
                },
            };
            album.usable_type = Some(sourced.clone());
            usable_types.push(sourced);
        }

        // An album is the only source of a usable record schema.
        Self {
            albums,
            usable_types,
            health,
            vault_root: bob_dir.to_path_buf(),
            claims,
        }
    }

    /// Perform a full scan of the explicit effective vault.
    pub(crate) fn discover(bob_dir: &Path) -> Self {
        let snapshot = VaultSnapshot::load(bob_dir);
        Self::from_snapshot(bob_dir, &snapshot)
    }

    pub(crate) fn is_for_target(&self, bob_dir: &Path) -> bool {
        paths_equal(&self.vault_root, bob_dir)
    }

    /// Vault-relative paths of every album (valid or broken) claiming
    /// this stem or noun, case-insensitively. Album creation and
    /// The retired `records init` route refuses overlapping claims through this inventory.
    pub(crate) fn claimants(&self, key: &str) -> Vec<String> {
        let wanted = key.to_ascii_lowercase();
        let mut paths = self
            .claims
            .get(&wanted)
            .map(|indices| {
                indices
                    .iter()
                    .map(|index| self.albums[*index].declaration.path.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        paths.sort();
        paths.dedup();
        paths
    }

    /// Resolve a route against this registry's complete inventory. A
    /// conflicting or invalid album claim always refuses the route, and the
    /// returned health makes the scope of global checks explicit.
    #[allow(dead_code)]
    pub(crate) fn resolve_route(&self, route: &str) -> RouteResolution {
        let key = route.to_ascii_lowercase();
        if let Some(indices) = self.claims.get(&key) {
            let mut paths = indices
                .iter()
                .map(|index| self.albums[*index].declaration.path.clone())
                .collect::<Vec<_>>();
            paths.sort();
            paths.dedup();
            let valid = indices.iter().find_map(|index| {
                self.albums[*index].usable_type.as_ref().cloned()
            });
            if indices.len() == 1
                && let Some(record_type) = valid
            {
                return RouteResolution::AlbumValid {
                    record_type,
                    health: self.health.clone(),
                };
            }
            let mut findings = indices
                .iter()
                .flat_map(|index| self.albums[*index].findings.clone())
                .collect::<Vec<_>>();
            sort_findings(&mut findings);
            return RouteResolution::AlbumInvalid {
                paths,
                findings,
                health: self.health.clone(),
            };
        }
        RouteResolution::Unclaimed {
            health: self.health.clone(),
        }
    }
}

/// Resolve a target route without reading record folders or unrelated note
/// bodies. With a known full registry, no filesystem reads occur. Without
/// one, this reads only `<bob_dir>/<route>.md` through its frontmatter and
/// follows that album's owner chain.
#[allow(dead_code)]
pub(crate) fn resolve_route_head(
    bob_dir: &Path,
    route: &str,
    known: Option<&AlbumRegistry>,
) -> Result<RouteResolution, std::io::Error> {
    let Some(stem) = confined_album_route(route) else {
        return Ok(unsafe_route_resolution(route));
    };
    if let Some(registry) =
        known.filter(|registry| registry.is_for_target(bob_dir))
    {
        return Ok(registry.resolve_route(&stem));
    }
    let note_path = PathBuf::from(format!("{stem}.md"));
    let disk_path = bob_dir.join(&note_path);
    if let Err(message) = confine_vault_file(bob_dir, &disk_path) {
        return Ok(confined_target_resolution(&stem, &message));
    }
    let head = match read_frontmatter_head(&disk_path) {
        Ok(head) => head,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            String::new()
        }
        Err(error) => return Err(error),
    };
    if let Some(declaration) = parse_album_declaration(&note_path, &head) {
        let mut findings = declaration.findings.clone();
        match declaration.parent.as_deref() {
            None => push_finding(
                &mut findings,
                "album_parent_missing",
                AlbumIssueSeverity::Error,
                &declaration.path,
                None,
                Some("parent".to_owned()),
                "album needs exactly one parent link to an area or project",
            ),
            Some(raw) if multi_parent(raw) => push_finding(
                &mut findings,
                "album_parent_ambiguous",
                AlbumIssueSeverity::Error,
                &declaration.path,
                None,
                Some("parent".to_owned()),
                "album names multiple parents; keep exactly one area or project link",
            ),
            Some(raw) if wikilink_path_target(raw).is_none() => push_finding(
                &mut findings,
                "album_parent_missing",
                AlbumIssueSeverity::Error,
                &declaration.path,
                None,
                Some("parent".to_owned()),
                "album parent must be one wikilink to an area or project",
            ),
            Some(raw) if is_unquoted_parent(raw) => push_finding(
                &mut findings,
                "album_parent_unquoted",
                AlbumIssueSeverity::Warning,
                &declaration.path,
                None,
                Some("parent".to_owned()),
                "quote the parent as `\"[[stem]]\"`; bare `[[stem]]` parses as a nested list",
            ),
            Some(_) => {}
        }
        let valid = declaration.record_type.is_some() && !has_errors(&findings);
        if valid {
            let record_type = declaration.record_type.expect("checked above");
            return Ok(RouteResolution::AlbumValid {
                record_type: SourcedRecordType {
                    record_type,
                    source: RecordTypeSource::Album {
                        path: declaration.path,
                    },
                },
                health: DiscoveryHealth::TargetOnly,
            });
        }
        return Ok(RouteResolution::AlbumInvalid {
            paths: vec![declaration.path],
            findings,
            health: DiscoveryHealth::TargetOnly,
        });
    }

    Ok(RouteResolution::Unclaimed {
        health: DiscoveryHealth::TargetOnly,
    })
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub(crate) fn confine_vault_file(
    vault: &Path,
    path: &Path,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => {
            return Err(format!(
                "could not inspect album target {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.file_type().is_symlink() {
        return Ok(());
    }
    let vault_canon = fs::canonicalize(vault).map_err(|error| {
        format!("could not resolve vault root {}: {error}", vault.display())
    })?;
    let target = fs::canonicalize(path).map_err(|error| {
        format!("could not resolve album target {}: {error}", path.display())
    })?;
    if !target.starts_with(&vault_canon) {
        return Err(format!(
            "album target {} resolves outside the vault",
            path.display()
        ));
    }
    Ok(())
}

fn unsafe_route_resolution(route: &str) -> RouteResolution {
    RouteResolution::AlbumInvalid {
        paths: vec![route.to_owned()],
        findings: vec![AlbumFinding {
            code: "album_stem_invalid",
            severity: AlbumIssueSeverity::Error,
            path: route.to_owned(),
            line_number: None,
            key_path: None,
            message: format!(
                "album route {route:?} must be a vault-root capture stem and cannot leave the vault"
            ),
            suggestions: Vec::new(),
        }],
        health: DiscoveryHealth::TargetOnly,
    }
}

fn confined_target_resolution(stem: &str, message: &str) -> RouteResolution {
    RouteResolution::AlbumInvalid {
        paths: vec![format!("{stem}.md")],
        findings: vec![AlbumFinding {
            code: "album_stem_invalid",
            severity: AlbumIssueSeverity::Error,
            path: format!("{stem}.md"),
            line_number: None,
            key_path: None,
            message: message.to_owned(),
            suggestions: Vec::new(),
        }],
        health: DiscoveryHealth::TargetOnly,
    }
}

#[allow(dead_code)]
fn read_frontmatter_head(path: &Path) -> Result<String, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut head = Vec::new();
    let mut line = Vec::new();
    let mut first_line = true;
    let mut byte = [0u8; 1];
    loop {
        if file.read(&mut byte)? == 0 {
            if !line.is_empty() {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let opening = first_line;
                let is_delimiter = line == b"---";
                head.extend_from_slice(&line);
                head.push(b'\n');
                if (opening && !is_delimiter) || (!opening && is_delimiter) {
                    break;
                }
            }
            break;
        }
        if byte[0] != b'\n' {
            line.push(byte[0]);
            continue;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let opening = first_line;
        first_line = false;
        let is_delimiter = line == b"---";
        head.extend_from_slice(&line);
        head.push(b'\n');
        line.clear();
        if (opening && !is_delimiter) || (!opening && is_delimiter) {
            break;
        }
    }
    String::from_utf8(head).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
    })
}

fn analyze_direct_parent(
    declaration: &AlbumDeclaration,
    lite: &NoteFactsLite,
    index: &NoteIndex,
    files: &BTreeMap<PathBuf, SnapshotFile>,
    findings: &mut Vec<AlbumFinding>,
) -> (Option<PathBuf>, Option<&'static str>) {
    if lite.parent_count > 1 {
        push_finding(
            findings,
            "album_parent_ambiguous",
            AlbumIssueSeverity::Error,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "album names multiple parents; keep exactly one area or project link",
        );
        return (None, None);
    }
    let Some(raw) = lite.parent_raw.as_deref() else {
        push_finding(
            findings,
            "album_parent_missing",
            AlbumIssueSeverity::Error,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "album needs exactly one parent link to an area or project",
        );
        return (None, None);
    };
    if multi_parent(raw) {
        push_finding(
            findings,
            "album_parent_ambiguous",
            AlbumIssueSeverity::Error,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "album names multiple parents; keep exactly one area or project link",
        );
        return (None, None);
    }
    let Some(target) = wikilink_path_target(raw) else {
        push_finding(
            findings,
            "album_parent_missing",
            AlbumIssueSeverity::Error,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "album parent does not resolve to a note; use one area or project link",
        );
        return (None, None);
    };
    let path = Path::new(&declaration.path);
    let resolution = index.resolve_detailed(Some(path), &target);
    let target_path = match resolution {
        LinkResolution::Found(target_path) => target_path,
        LinkResolution::Ambiguous => {
            push_finding(
                findings,
                "album_parent_ambiguous",
                AlbumIssueSeverity::Error,
                &declaration.path,
                lite.parent_line,
                Some("parent".to_owned()),
                "album parent stem is ambiguous; use a path-qualified link",
            );
            return (None, None);
        }
        LinkResolution::Missing => {
            push_finding(
                findings,
                "album_parent_missing",
                AlbumIssueSeverity::Error,
                &declaration.path,
                lite.parent_line,
                Some("parent".to_owned()),
                "album parent note is missing; use a link to an area or project",
            );
            return (None, None);
        }
    };
    if target_path == path {
        push_finding(
            findings,
            "album_cycle",
            AlbumIssueSeverity::Error,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "album cannot be its own parent",
        );
        return (None, None);
    }
    let kind =
        files
            .get(&target_path)
            .and_then(|target| match target.lite.kind {
                LiteKind::Area => Some("area"),
                LiteKind::Project { .. } => Some("project"),
                _ => None,
            });
    let Some(kind) = kind else {
        push_finding(
            findings,
            "album_parent_kind",
            AlbumIssueSeverity::Error,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "album parent must be an area or project note",
        );
        return (None, None);
    };
    if is_unquoted_parent(raw) {
        push_finding(
            findings,
            "album_parent_unquoted",
            AlbumIssueSeverity::Warning,
            &declaration.path,
            lite.parent_line,
            Some("parent".to_owned()),
            "quote the parent as `\"[[stem]]\"`; bare `[[stem]]` parses as a nested list",
        );
    }
    (Some(target_path), Some(kind))
}

fn has_parent_cycle(
    start: &Path,
    _declaration: &AlbumDeclaration,
    files: &BTreeMap<PathBuf, SnapshotFile>,
    index: &NoteIndex,
) -> bool {
    let mut seen = BTreeSet::new();
    let mut current = start.to_path_buf();
    for _ in 0..64 {
        if !seen.insert(current.clone()) {
            return true;
        }
        let Some(file) = files.get(&current) else {
            return false;
        };
        let Some(raw) = file.lite.parent_raw.as_deref() else {
            return false;
        };
        if file.lite.parent_count != 1 || multi_parent(raw) {
            return false;
        }
        let Some(target) = wikilink_path_target(raw) else {
            return false;
        };
        match index.resolve_detailed(Some(&current), &target) {
            LinkResolution::Found(path) => current = path,
            LinkResolution::Missing | LinkResolution::Ambiguous => {
                return false
            }
        }
    }
    false
}

fn add_checkbox_findings(
    findings: &mut Vec<AlbumFinding>,
    path: &str,
    contents: Option<&str>,
) {
    let Some(contents) = contents else {
        return;
    };
    let lines = rules::physical_lines(contents);
    let body_start = rules::body_start_line(contents);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate() {
        let line_number = index + 1;
        if line_number < body_start || rules::track_fence(line, &mut fence) {
            continue;
        }
        if fence.is_none() && rules::is_checkbox_line(line) {
            push_finding(
                findings,
                "album_checkbox",
                AlbumIssueSeverity::Error,
                path,
                Some(line_number),
                None,
                "albums hold no checkbox rows; move tasks to the owner area or project",
            );
        }
    }
}

fn add_child_reparent_findings(
    albums: &mut [AlbumEntry],
    files: &BTreeMap<PathBuf, SnapshotFile>,
    index: &NoteIndex,
) {
    for album in albums {
        let album_path = PathBuf::from(&album.declaration.path);
        let expected_folder = album.declaration.stem.to_ascii_lowercase();
        for (child_path, child) in files {
            let Some(raw) = child.lite.parent_raw.as_deref() else {
                continue;
            };
            if child.lite.parent_count != 1 {
                continue;
            }
            let Some(target) = wikilink_path_target(raw) else {
                continue;
            };
            if index.resolve_detailed(Some(child_path), &target).path()
                != Some(album_path.as_path())
            {
                continue;
            }
            let in_album_folder = child_path.parent().is_some_and(|parent| {
                parent
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected_folder)
            });
            if !in_album_folder {
                album.findings.push(AlbumFinding {
                    code: "album_child_reparent",
                    severity: AlbumIssueSeverity::Info,
                    path: display_slash(child_path),
                    line_number: child.lite.parent_line,
                    key_path: Some("parent".to_owned()),
                    message: format!(
                        "this child lives outside {}/; keep [[{}]] as its owner by moving it into that folder, or change its parent",
                        album.declaration.stem, album.declaration.stem
                    ),
                    suggestions: vec![format!("{}/", album.declaration.stem)],
                });
            }
        }
    }
}

fn build_claims(albums: &[AlbumEntry]) -> BTreeMap<String, Vec<usize>> {
    let mut claims: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (index, album) in albums.iter().enumerate() {
        claims
            .entry(album.declaration.stem.to_ascii_lowercase())
            .or_default()
            .insert(index);
        if let Some(noun) = album.declaration.noun_claim.as_deref() {
            claims
                .entry(noun.to_ascii_lowercase())
                .or_default()
                .insert(index);
        }
    }
    claims
        .into_iter()
        .map(|(key, indices)| (key, indices.into_iter().collect()))
        .collect()
}

fn add_claim_conflicts(
    albums: &mut [AlbumEntry],
    claims: &BTreeMap<String, Vec<usize>>,
) {
    for (claim, indices) in claims {
        if indices.len() < 2 {
            continue;
        }
        let participants = indices
            .iter()
            .map(|index| albums[*index].declaration.path.clone())
            .collect::<Vec<_>>();
        for index in indices {
            let album = &mut albums[*index];
            let record_line = album.declaration.record_line;
            push_finding(
                &mut album.findings,
                "album_noun_duplicate",
                AlbumIssueSeverity::Error,
                &album.declaration.path.clone(),
                record_line,
                Some("record.noun".to_owned()),
                format!(
                    "album route or noun claim {claim:?} conflicts with {}",
                    participants
                        .iter()
                        .filter(|path| path.as_str()
                            != album.declaration.path.as_str())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
}

fn add_property_conflicts(albums: &mut [AlbumEntry]) {
    let mut fields: BTreeMap<String, Vec<(usize, PropertyStorageKind, bool)>> =
        BTreeMap::new();
    for (index, album) in albums.iter().enumerate() {
        let Some(record_type) = album.declaration.record_type.as_ref() else {
            continue;
        };
        for field in &record_type.fields {
            fields.entry(field.key.clone()).or_default().push((
                index,
                field.kind.storage_kind(),
                field.list,
            ));
        }
    }
    for (key, declarations) in fields {
        let identities = declarations
            .iter()
            .map(|(_, storage, list)| (*storage, *list))
            .collect::<BTreeSet<_>>();
        if identities.len() < 2 {
            continue;
        }
        let participants = declarations
            .iter()
            .map(|(index, _, _)| albums[*index].declaration.path.clone())
            .collect::<BTreeSet<_>>();
        for index in declarations
            .iter()
            .map(|(index, _, _)| *index)
            .collect::<BTreeSet<_>>()
        {
            let path = albums[index].declaration.path.clone();
            let record_line = albums[index].declaration.record_line;
            push_finding(
                &mut albums[index].findings,
                "album_property_conflict",
                AlbumIssueSeverity::Error,
                &path,
                record_line,
                Some(format!("record.fields.{key}")),
                format!(
                    "field {key:?} has incompatible property storage across {}",
                    participants
                        .iter()
                        .filter(
                            |participant| participant.as_str() != path.as_str()
                        )
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
}

fn album_breadcrumb(
    residence_chain: &[String],
    root_parent: &str,
    album_path: &Path,
    files: &BTreeMap<PathBuf, SnapshotFile>,
    declarations: &BTreeMap<PathBuf, AlbumDeclaration>,
) -> Vec<String> {
    let mut paths = residence_chain
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if let Some(root_at) = paths
        .iter()
        .position(|path| display_slash(path) == root_parent)
    {
        paths.truncate(root_at + 1);
    }
    paths.reverse();
    if !paths.iter().any(|path| path == album_path) {
        paths.push(album_path.to_path_buf());
    }
    paths
        .iter()
        .filter_map(|path| {
            if let Some(declaration) = declarations.get(path) {
                return Some(declaration.title.clone());
            }
            let file = files.get(path)?;
            if matches!(
                file.lite.kind,
                LiteKind::Area | LiteKind::Project { .. }
            ) {
                return Some(file.lite.stem.clone());
            }
            let contents = file.contents.as_deref().unwrap_or(&file.header);
            Some(crate::native::notebooks::scan::derive_title(
                contents,
                file.lite.aliases_first.as_deref(),
                &file.lite.stem,
            ))
        })
        .collect()
}

fn has_errors(findings: &[AlbumFinding]) -> bool {
    findings
        .iter()
        .any(|finding| finding.severity == AlbumIssueSeverity::Error)
}

fn push_finding(
    findings: &mut Vec<AlbumFinding>,
    code: &'static str,
    severity: AlbumIssueSeverity,
    path: &str,
    line_number: Option<usize>,
    key_path: Option<String>,
    message: impl Into<String>,
) {
    findings.push(AlbumFinding {
        code,
        severity,
        path: path.to_owned(),
        line_number,
        key_path,
        message: message.into(),
        suggestions: Vec::new(),
    });
}

#[allow(dead_code)]
fn sort_findings(findings: &mut Vec<AlbumFinding>) {
    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line_number.cmp(&right.line_number))
            .then_with(|| left.code.cmp(right.code))
    });
    findings.dedup_by(|left, right| {
        left.path == right.path
            && left.line_number == right.line_number
            && left.code == right.code
            && left.key_path == right.key_path
    });
}

fn multi_parent(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        let inner = trimmed.trim_start_matches('[').trim_end_matches(']');
        let mut quote = None;
        let mut nested = 0usize;
        for ch in inner.chars() {
            if let Some(expected) = quote {
                if ch == expected {
                    quote = None;
                }
            } else if matches!(ch, '\'' | '"') {
                quote = Some(ch);
            } else if ch == '[' {
                nested += 1;
            } else if ch == ']' {
                nested = nested.saturating_sub(1);
            } else if ch == ',' && nested == 0 {
                return true;
            }
        }
    }
    trimmed.match_indices("[[").count() > 1
}

fn is_unquoted_parent(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.starts_with("[[") && trimmed.ends_with("]]")
}

fn stem_of_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_route_head, AlbumRegistry, DiscoveryHealth, RecordTypeSource,
        RouteResolution,
    };
    use crate::native::notebooks::snapshot::{LiteKind, VaultSnapshot};
    use std::{fs, path::Path};

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(path, contents).expect("write fixture");
    }

    fn area() -> &'static str {
        "---\ntype: [[area]]\n---\n# Work\n"
    }

    fn album(stem: &str, noun: &str, fields: &str) -> String {
        let schema = if fields.trim().is_empty() {
            format!("  noun: {noun}\n")
        } else {
            format!("  noun: {noun}\n  fields:\n{fields}")
        };
        format!(
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nsummary: {stem} collection\nrecord:\n{schema}---\n# {}\n",
            stem.to_ascii_uppercase()
        )
    }

    fn fixture() -> tempfile::TempDir {
        tempfile::tempdir().expect("temporary album fixture")
    }

    #[test]
    fn snapshot_retains_album_claims_and_reclassifies_overlays() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("meal.md"),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord:\n  noun: dining\n  fields: {status: {values: []}}\n---\n# Meals\n",
        );
        write(
            &vault.join("meal/old.md"),
            "---\ntype: [[dining]]\n---\n# Old\n",
        );
        write(
            &vault.join("meal/generic.md"),
            "---\ntype: [[record]]\n---\n# Generic\n",
        );
        write(
            &vault.join("mixed.md"),
            "---\ntype: [\"[[area]]\", \"[[album]]\"]\nrecord: {noun: mixed}\n---\n",
        );

        let snapshot = VaultSnapshot::load(&vault);
        assert!(snapshot.contents(Path::new("meal.md")).is_some());
        assert_eq!(snapshot.album_schema_diagnostics.len(), 1);
        assert!(snapshot.io_errors.is_empty());
        assert_eq!(
            snapshot.files[Path::new("meal/old.md")].lite.kind,
            LiteKind::RecordInstance
        );
        assert_eq!(
            snapshot.files[Path::new("meal/generic.md")].lite.kind,
            LiteKind::RecordInstance
        );
        assert_eq!(
            snapshot.files[Path::new("mixed.md")].lite.kind,
            LiteKind::Album
        );

        let overlay = snapshot.with_overlay([(
            Path::new("meal.md").to_path_buf(),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: food}\n---\n# Meals\n".to_owned(),
        )]);
        assert_eq!(
            overlay.files[Path::new("meal/old.md")].lite.kind,
            LiteKind::Other
        );
        assert_eq!(
            overlay.files[Path::new("meal/generic.md")].lite.kind,
            LiteKind::RecordInstance
        );
        assert_eq!(
            overlay.record_ids.iter().cloned().collect::<Vec<_>>(),
            vec!["food".to_owned(), "mixed".to_owned()]
        );
    }

    #[test]
    fn broken_album_claim_does_not_hide_a_valid_album() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: restaurant, fields: {status: {values: []}}}\n---\n# Eat\n",
        );
        write(&vault.join("books.md"), &album("books", "book", ""));

        let registry = AlbumRegistry::discover(&vault);
        assert_eq!(registry.usable_types.len(), 1);
        assert!(!registry
            .usable_types
            .iter()
            .any(|sourced| sourced.record_type.id == "recipe"));
        assert_eq!(registry.usable_types[0].record_type.id, "book");
        assert!(matches!(
            registry.resolve_route("restaurant"),
            RouteResolution::AlbumInvalid { .. }
        ));
        assert!(matches!(
            registry.resolve_route("book"),
            RouteResolution::AlbumValid { .. }
        ));
    }

    #[test]
    fn duplicate_nouns_and_property_storage_conflicts_mark_every_album() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("movies.md"),
            &album("movies", "film", "    rating: {values: number}\n"),
        );
        write(
            &vault.join("shows.md"),
            &album("shows", "film", "    rating: {}\n"),
        );

        let registry = AlbumRegistry::discover(&vault);
        assert_eq!(registry.albums.len(), 2);
        for entry in &registry.albums {
            assert!(entry
                .findings
                .iter()
                .any(|finding| finding.code == "album_noun_duplicate"));
            assert!(entry
                .findings
                .iter()
                .any(|finding| finding.code == "album_property_conflict"));
            assert_eq!(entry.owner.as_deref(), Some("area"));
            assert_eq!(entry.root.as_deref(), Some("area"));
            assert_eq!(
                entry.breadcrumb.first().map(String::as_str),
                Some("area")
            );
        }
        assert!(matches!(
            registry.resolve_route("film"),
            RouteResolution::AlbumInvalid { paths, .. } if paths.len() == 2
        ));
    }

    #[test]
    fn stem_noun_and_case_insensitive_folder_collisions_invalidate_all_claims()
    {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(&vault.join("movies.md"), &album("movies", "film", ""));
        write(&vault.join("shows.md"), &album("shows", "movies", ""));
        write(&vault.join("Eat.md"), &album("Eat", "cuisine", ""));
        write(&vault.join("eat.md"), &album("eat", "meal", ""));

        let registry = AlbumRegistry::discover(&vault);
        let paths_for = |route: &str| match registry.resolve_route(route) {
            RouteResolution::AlbumInvalid { paths, .. } => paths,
            other => panic!("expected invalid claim, got {other:?}"),
        };
        assert_eq!(paths_for("movies"), vec!["movies.md", "shows.md"]);
        assert_eq!(paths_for("eat"), vec!["Eat.md", "eat.md"]);
        for entry in &registry.albums {
            if ["movies.md", "shows.md", "Eat.md", "eat.md"]
                .contains(&entry.declaration.path.as_str())
            {
                assert!(entry
                    .findings
                    .iter()
                    .any(|finding| finding.code == "album_noun_duplicate"));
                assert!(entry.usable_type.is_none());
            }
        }
    }

    #[test]
    fn malformed_album_schema_does_not_hide_valid_album() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("books.md"),
            &album("books", "book", "    status: {values: {read: Read}}\n"),
        );
        write(
            &vault.join("broken.md"),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nsummary: Broken schema\nrecord: {noun: broken, fields: {state: {values: []}}}\n---\n",
        );
        let registry = AlbumRegistry::discover(&vault);
        assert_eq!(registry.usable_types.len(), 1);
        assert_eq!(registry.usable_types[0].record_type.id, "book");
        assert!(matches!(
            registry.resolve_route("book"),
            RouteResolution::AlbumValid { .. }
        ));
    }

    #[test]
    fn parent_scope_handles_unquoted_terminal_project_cycles_and_checkboxes() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("finished.md"),
            "---\ntype: [[project]]\nparent: \"[[area]]\"\nstatus: done\n---\n# Finished\n",
        );
        write(
            &vault.join("cycle_a.md"),
            "---\ntype: [[project]]\nparent: \"[[cycle_b]]\"\n---\n",
        );
        write(
            &vault.join("cycle_b.md"),
            "---\ntype: [[project]]\nparent: \"[[cycle_a]]\"\n---\n",
        );
        write(
            &vault.join("cards.md"),
            "---\ntype: [[album]]\nparent: [[finished]]\nsummary: Cards\nrecord: {noun: card}\n---\n- [ ] this is not a task here\n```md\n- [ ] fenced example\n```\n",
        );
        write(
            &vault.join("loop.md"),
            "---\ntype: [[album]]\nparent: \"[[cycle_a]]\"\nsummary: Loop\nrecord: {noun: loop}\n---\n",
        );
        write(
            &vault.join("child.md"),
            "---\ntype: [[record]]\nparent: \"[[cards]]\"\n---\n",
        );

        let registry = AlbumRegistry::discover(&vault);
        let cards = registry
            .albums
            .iter()
            .find(|entry| entry.declaration.stem == "cards")
            .expect("cards album");
        assert_eq!(cards.parent_kind, Some("project"));
        assert_eq!(cards.owner.as_deref(), Some("finished"));
        assert_eq!(cards.root.as_deref(), Some("finished"));
        assert!(cards
            .findings
            .iter()
            .any(|finding| finding.code == "album_parent_unquoted"));
        assert_eq!(
            cards
                .findings
                .iter()
                .filter(|finding| finding.code == "album_checkbox")
                .count(),
            1
        );
        assert!(cards
            .findings
            .iter()
            .any(|finding| finding.code == "album_child_reparent"));
        let cycle = registry
            .albums
            .iter()
            .find(|entry| entry.declaration.stem == "loop")
            .expect("cycle album");
        assert!(cycle
            .findings
            .iter()
            .any(|finding| finding.code == "album_cycle"));
    }

    #[test]
    fn identical_album_stems_are_scoped_to_the_supplied_vault() {
        let temp = fixture();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        write(&first.join("area.md"), area());
        write(&second.join("area.md"), area());
        write(
            &first.join("movies.md"),
            &album("movies", "movie", "    year: {values: number}\n"),
        );
        write(
            &first.join("books.md"),
            &album("books", "book", "    author: {}\n"),
        );
        write(
            &second.join("movies.md"),
            &album("movies", "movie", "    name: {}\n"),
        );
        let first_registry = AlbumRegistry::discover(&first);
        let second_registry = AlbumRegistry::discover(&second);
        assert_eq!(first_registry.usable_types.len(), 2);
        let field = |registry: &AlbumRegistry| {
            registry
                .albums
                .iter()
                .find(|entry| entry.declaration.stem == "movies")
                .expect("movies album")
                .declaration
                .record_type
                .as_ref()
                .expect("valid type")
                .fields[0]
                .key
                .clone()
        };
        assert_eq!(field(&first_registry), "year");
        assert_eq!(field(&second_registry), "name");
    }

    #[test]
    fn album_parent_validation_and_notebook_parent_regression() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(&vault.join("album.md"), &album("album", "collection", ""));
        write(
            &vault.join("missing.md"),
            "---\ntype: [[album]]\nsummary: Missing parent\nparent: \"[[absent]]\"\nrecord: {noun: missing}\n---\n",
        );
        write(
            &vault.join("nested/not_root.md"),
            "---\ntype: [[album]]\nsummary: Nested album\nparent: \"[[area]]\"\nrecord: {noun: nested}\n---\n",
        );
        write(
            &vault.join("notebook_child.md"),
            "---\ntype: [[notebook]]\nparent: \"[[album]]\"\nsummary: Child\n---\n",
        );
        write(
            &vault.join("notebook_parent.md"),
            "---\ntype: [[notebook]]\nparent: \"[[area]]\"\nsummary: Notebook\n---\n",
        );
        write(
            &vault.join("under_notebook.md"),
            "---\ntype: [[album]]\nparent: \"[[notebook_parent]]\"\nsummary: Wrong parent kind\nrecord: {noun: wrong_kind}\n---\n",
        );
        write(
            &vault.join("multiple.md"),
            "---\ntype: [[album]]\nparent: [\"[[area]]\", \"[[notebook_parent]]\"]\nsummary: Multiple parents\nrecord: {noun: multiple}\n---\n",
        );
        write(&vault.join("nested/area.md"), "---\ntype: [[area]]\n---\n");
        write(
            &vault.join("nested/shared.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write(
            &vault.join("other/shared.md"),
            "---\ntype: [[project]]\n---\n",
        );
        write(
            &vault.join("ambiguous.md"),
            "---\ntype: [[album]]\nparent: \"[[shared]]\"\nsummary: Ambiguous parent\nrecord: {noun: ambiguous}\n---\n",
        );

        let snapshot = VaultSnapshot::load(&vault);
        let registry = AlbumRegistry::from_snapshot(&vault, &snapshot);
        let finding = |path: &str, code: &str| {
            registry
                .albums
                .iter()
                .find(|entry| entry.declaration.path == path)
                .expect("album entry")
                .findings
                .iter()
                .any(|finding| finding.code == code)
        };
        assert!(finding("missing.md", "album_parent_missing"));
        assert!(finding("nested/not_root.md", "album_not_at_root"));
        assert!(finding("under_notebook.md", "album_parent_kind"));
        assert!(finding("multiple.md", "album_parent_ambiguous"));
        assert!(finding("ambiguous.md", "album_parent_ambiguous"));

        let notebook_analysis =
            crate::native::notebooks::scan::analyze(&snapshot);
        let notebook = notebook_analysis
            .report
            .notebooks
            .iter()
            .find(|entry| entry.stem == "notebook_child")
            .expect("notebook entry retained");
        assert!(notebook
            .issues
            .iter()
            .any(|issue| issue.code == "notebook_parent_kind"));
    }

    #[test]
    fn record_recognition_uses_album_claims() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("podcasts.md"),
            &album("podcasts", "podcast", ""),
        );
        write(
            &vault.join("podcasts/item.md"),
            "---\ntype: [[record]]\nparent: \"[[podcasts]]\"\n---\n# Episode\n",
        );
        let snapshot = VaultSnapshot::load(&vault);
        assert_eq!(
            snapshot.files[Path::new("podcasts/item.md")].lite.kind,
            LiteKind::RecordInstance
        );
    }

    #[test]
    fn target_head_resolution_does_not_read_album_body_or_record_folders() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        fs::write(
            vault.join("movies.md"),
            b"---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: movie}\n---\n\xff body is invalid UTF-8",
        )
        .expect("write target with invalid body bytes");
        fs::create_dir_all(vault.join("movies")).expect("create record folder");
        fs::write(vault.join("movies/record.md"), [0xff, 0xfe])
            .expect("write unreadable unrelated record body");

        let result = resolve_route_head(&vault, "movies", None)
            .expect("frontmatter head is readable");
        assert!(matches!(
            result,
            RouteResolution::AlbumValid {
                health: DiscoveryHealth::TargetOnly,
                ..
            }
        ));
        if let RouteResolution::AlbumValid { record_type, .. } = result {
            assert!(matches!(
                record_type.source,
                RecordTypeSource::Album { .. }
            ));
        }
    }

    #[test]
    fn route_head_refuses_traversal_absolute_and_reserved_stems() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        let outside = temp.path().join("outside.md");
        fs::create_dir_all(&vault).expect("create vault");
        write(
            &outside,
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: escaped}\n---\n# Outside\n",
        );
        write(
            &vault.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: restaurant}\n---\n# Eat\n",
        );

        for route in ["../outside", "/tmp/outside", "done", "ref", ".bob"] {
            match resolve_route_head(&vault, route, None)
                .expect("unsafe routes refuse without I/O")
            {
                RouteResolution::AlbumInvalid {
                    findings, health, ..
                } => {
                    assert!(
                        findings.iter().any(|finding| finding.code == "album_stem_invalid"),
                        "route {route:?}"
                    );
                    assert_eq!(health, DiscoveryHealth::TargetOnly);
                }
                other => {
                    panic!("expected invalid route {route:?}, got {other:?}")
                }
            }
        }

        let valid =
            resolve_route_head(&vault, "eat", None).expect("bounded head");
        assert!(matches!(
            valid,
            RouteResolution::AlbumValid {
                health: DiscoveryHealth::TargetOnly,
                ..
            }
        ));
    }

    #[test]
    fn route_head_refuses_a_symlink_escape_and_foreign_known_registry() {
        let temp = fixture();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let outside = temp.path().join("outside.md");
        write(&first.join("area.md"), area());
        write(&second.join("area.md"), area());
        write(
            &first.join("movies.md"),
            &album("movies", "movie", "    year: {values: number}\n"),
        );
        write(
            &second.join("movies.md"),
            &album("movies", "movie", "    name: {}\n"),
        );
        write(
            &outside,
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: escaped}\n---\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&outside, first.join("eat.md")).expect("symlink album");
            match resolve_route_head(&first, "eat", None)
                .expect("symlink escape is a refusal")
            {
                RouteResolution::AlbumInvalid { findings, .. } => {
                    assert!(findings.iter().any(|finding| {
                        finding.code == "album_stem_invalid"
                            && finding.message.contains("outside")
                    }));
                }
                other => panic!("expected confined refusal, got {other:?}"),
            }
        }

        let first_registry = AlbumRegistry::discover(&first);
        let from_second =
            resolve_route_head(&second, "movies", Some(&first_registry))
                .expect("foreign known registry is ignored");
        match from_second {
            RouteResolution::AlbumValid { record_type, .. } => {
                assert_eq!(record_type.record_type.fields[0].key, "name");
            }
            other => panic!("expected second vault schema, got {other:?}"),
        }
    }

    #[test]
    fn broken_noun_recovery_preserves_album_claims() {
        let temp = fixture();
        let vault = temp.path().join("vault");
        write(&vault.join("area.md"), area());
        write(
            &vault.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nsummary: Nested noun only\nrecord:\n  fields:\n    thing:\n      noun: restaurant\n    broken: [\n---\n# Eat\n",
        );
        write(
            &vault.join("books.md"),
            &album("books", "book", "    author: {}\n"),
        );
        let registry = AlbumRegistry::discover(&vault);
        let ids: Vec<&str> = registry
            .usable_types
            .iter()
            .map(|sourced| sourced.record_type.id.as_str())
            .collect();
        assert!(ids.contains(&"book"));
        assert!(
            ids.contains(&"restaurant"),
            "nested noun must not suppress restaurant: {ids:?}"
        );
        let eat = registry
            .albums
            .iter()
            .find(|entry| entry.declaration.stem == "eat")
            .expect("eat album");
        assert_eq!(eat.declaration.noun_claim, None);
    }
}

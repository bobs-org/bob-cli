//! Shared, vault-aware resolution of record kinds for consumers.
//!
//! The album registry owns schema parsing and claim precedence. This adapter
//! adds the consumer-facing identity and presentation data once, so command,
//! capture, and completion surfaces can share the same result and JSON shape.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::rc::Rc;

use serde::Serialize;

use crate::native::{
    albums::{parse_album_declaration, registry},
    notebooks::{
        rules,
        scan::{derive_title, first_alias},
        snapshot::display_slash,
    },
    pomodoro_homes::{HomeResolver, HomeSource, NoteFacts},
    projects::{
        frontmatter_is_area, frontmatter_is_project, parse_frontmatter,
    },
    vault_links::{LinkResolution, VaultLinkResolver},
};

use super::schema::{RecordDiagnostic, RecordType};
use crate::native::albums::registry::{
    AlbumEntry, AlbumRegistry, DiscoveryHealth, RecordTypeSource,
    RouteResolution, SourcedRecordType,
};
use crate::native::albums::{AlbumFinding, AlbumIssueSeverity};

const PRESENTATION_HEAD_LIMIT: usize = 64 * 1024;

/// Album-facing presentation shared by all record consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AlbumRecordIdentity {
    pub(crate) stem: String,
    pub(crate) title: String,
    pub(crate) owner: Option<String>,
    pub(crate) breadcrumb: Vec<String>,
    #[serde(skip)]
    pub(crate) schema_field_lines: HashMap<String, usize>,
}

/// A resolved schema plus its immutable source and optional album identity.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedRecordKind {
    pub(crate) sourced: SourcedRecordType,
    pub(crate) album: Option<AlbumRecordIdentity>,
}

impl ResolvedRecordKind {
    pub(crate) fn record_type(&self) -> &RecordType {
        &self.sourced.record_type
    }

    pub(crate) fn source_name(&self) -> &'static str {
        match &self.sourced.source {
            RecordTypeSource::Album { .. } => "album",
            RecordTypeSource::Config { .. } => "config",
        }
    }

    /// One canonical JSON fragment for all record payload surfaces.
    /// Config-backed results omit `album`; album-backed results always use
    /// the same `{stem,title,owner,breadcrumb}` object.
    pub(crate) fn metadata_json(&self) -> serde_json::Value {
        #[derive(Serialize)]
        struct Metadata<'a> {
            source: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            album: Option<&'a AlbumRecordIdentity>,
        }

        serde_json::to_value(Metadata {
            source: self.source_name(),
            album: self.album.as_ref(),
        })
        .expect("record metadata is serializable")
    }

    pub(crate) fn render_identity(
        &self,
    ) -> super::render::RecordRenderIdentity {
        match &self.sourced.source {
            RecordTypeSource::Album { .. } => {
                super::render::RecordRenderIdentity::Album
            }
            RecordTypeSource::Config { .. } => {
                super::render::RecordRenderIdentity::Config
            }
        }
    }
}

/// Consumer-facing route result. A config fallback carries its deprecation
/// warning here; invalid album claims remain visible with their diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordKindResolution {
    Valid {
        kind: ResolvedRecordKind,
        warnings: Vec<AlbumFinding>,
        health: DiscoveryHealth,
    },
    Invalid {
        paths: Vec<String>,
        findings: Vec<AlbumFinding>,
        health: DiscoveryHealth,
    },
    Unclaimed {
        health: DiscoveryHealth,
        config_diagnostic: Option<RecordDiagnostic>,
    },
    DiscoveryIncomplete {
        errors: Vec<String>,
    },
}

/// One invocation's registry and the resolved-kind views built from it.
#[derive(Debug, Clone)]
pub(crate) struct RecordContext {
    registry: AlbumRegistry,
}

impl RecordContext {
    pub(crate) fn discover(bob_dir: &Path, config_path: &Path) -> Self {
        Self {
            registry: AlbumRegistry::discover(bob_dir, config_path),
        }
    }

    pub(crate) fn from_registry(registry: AlbumRegistry) -> Self {
        Self { registry }
    }

    pub(crate) fn registry(&self) -> &AlbumRegistry {
        &self.registry
    }

    /// Resolve either a noun or folder stem without choosing among conflicts.
    pub(crate) fn resolve(&self, selector: &str) -> RecordKindResolution {
        self.adapt_resolution(self.registry.resolve_route(selector))
    }

    pub(crate) fn lookup_noun(&self, noun: &str) -> RecordKindResolution {
        self.resolve(noun)
    }

    pub(crate) fn lookup_stem(&self, stem: &str) -> RecordKindResolution {
        self.resolve(stem)
    }

    /// Valid kinds in a complete inventory, retaining album presentation.
    pub(crate) fn usable_kinds(&self) -> Vec<ResolvedRecordKind> {
        self.registry
            .usable_types
            .iter()
            .cloned()
            .map(|sourced| self.resolve_sourced(sourced))
            .collect()
    }

    /// All claims, including broken claims retained for list/check reports.
    pub(crate) fn album_claims(&self) -> &[AlbumEntry] {
        &self.registry.albums
    }

    /// Album summary for an inventory row. Summary stays presentation data;
    /// the shared record metadata object keeps its stable transport shape.
    pub(crate) fn album_summary_for(
        &self,
        kind: &ResolvedRecordKind,
    ) -> Option<&str> {
        let RecordTypeSource::Album { path } = &kind.sourced.source else {
            return None;
        };
        self.registry
            .albums
            .iter()
            .find(|entry| entry.declaration.path == *path)
            .and_then(|entry| entry.declaration.summary.as_deref())
    }

    /// Only invalid claims, while preserving their original folders/findings.
    pub(crate) fn invalid_album_claims(&self) -> Vec<&AlbumEntry> {
        self.registry
            .albums
            .iter()
            .filter(|entry| {
                entry.usable_type.is_none()
                    && entry.findings.iter().any(|finding| {
                        finding.severity == AlbumIssueSeverity::Error
                    })
            })
            .collect()
    }

    /// Route-local resolution uses the registry when it belongs to this
    /// vault. Otherwise it reads the target album head and its owner chain,
    /// never the record folder or unrelated note bodies.
    pub(crate) fn resolve_route_head(
        bob_dir: &Path,
        config_path: &Path,
        selector: &str,
        known: Option<&Self>,
    ) -> io::Result<RecordKindResolution> {
        let matched_context = known.filter(|context| {
            context.registry.is_for_target(bob_dir, config_path)
        });
        let route = registry::resolve_route_head(
            bob_dir,
            config_path,
            selector,
            matched_context.map(|context| &context.registry),
        )?;
        let health = match &route {
            RouteResolution::AlbumValid { health, .. }
            | RouteResolution::AlbumInvalid { health, .. }
            | RouteResolution::Config { health, .. }
            | RouteResolution::Unclaimed { health, .. } => health.clone(),
            RouteResolution::DiscoveryIncomplete { errors } => {
                return Ok(RecordKindResolution::DiscoveryIncomplete {
                    errors: errors.clone(),
                });
            }
        };

        match route {
            RouteResolution::AlbumValid { record_type, .. } => {
                let full_metadata = matched_context.and_then(|context| {
                    context.album_identity_for(&record_type)
                });
                let album = match full_metadata {
                    Some(metadata) => metadata,
                    None => route_album_identity(bob_dir, &record_type)?,
                };
                Ok(RecordKindResolution::Valid {
                    kind: ResolvedRecordKind {
                        sourced: record_type,
                        album: Some(album),
                    },
                    warnings: Vec::new(),
                    health,
                })
            }
            RouteResolution::Config {
                record_type,
                finding,
                ..
            } => Ok(RecordKindResolution::Valid {
                kind: ResolvedRecordKind {
                    sourced: record_type,
                    album: None,
                },
                warnings: vec![finding],
                health,
            }),
            RouteResolution::AlbumInvalid {
                paths, findings, ..
            } => Ok(RecordKindResolution::Invalid {
                paths,
                findings,
                health,
            }),
            RouteResolution::Unclaimed {
                config_diagnostic, ..
            } => Ok(RecordKindResolution::Unclaimed {
                health,
                config_diagnostic,
            }),
            RouteResolution::DiscoveryIncomplete { .. } => {
                unreachable!("handled above")
            }
        }
    }

    fn adapt_resolution(&self, route: RouteResolution) -> RecordKindResolution {
        match route {
            RouteResolution::AlbumValid {
                record_type,
                health,
            } => RecordKindResolution::Valid {
                kind: self.resolve_sourced(record_type),
                warnings: Vec::new(),
                health,
            },
            RouteResolution::Config {
                record_type,
                finding,
                health,
            } => RecordKindResolution::Valid {
                kind: self.resolve_sourced(record_type),
                warnings: vec![finding],
                health,
            },
            RouteResolution::AlbumInvalid {
                paths,
                findings,
                health,
            } => RecordKindResolution::Invalid {
                paths,
                findings,
                health,
            },
            RouteResolution::Unclaimed {
                health,
                config_diagnostic,
            } => RecordKindResolution::Unclaimed {
                health,
                config_diagnostic,
            },
            RouteResolution::DiscoveryIncomplete { errors } => {
                RecordKindResolution::DiscoveryIncomplete { errors }
            }
        }
    }

    fn resolve_sourced(
        &self,
        sourced: SourcedRecordType,
    ) -> ResolvedRecordKind {
        let album = self.album_identity_for(&sourced);
        ResolvedRecordKind { sourced, album }
    }

    fn album_identity_for(
        &self,
        sourced: &SourcedRecordType,
    ) -> Option<AlbumRecordIdentity> {
        let RecordTypeSource::Album { path } = &sourced.source else {
            return None;
        };
        self.registry
            .albums
            .iter()
            .find(|entry| entry.declaration.path == *path)
            .map(identity_from_entry)
    }
}

fn identity_from_entry(entry: &AlbumEntry) -> AlbumRecordIdentity {
    AlbumRecordIdentity {
        stem: entry.declaration.stem.clone(),
        title: entry.declaration.title.clone(),
        owner: entry.owner.clone(),
        breadcrumb: entry.breadcrumb.clone(),
        schema_field_lines: entry.declaration.schema_field_lines.clone(),
    }
}

fn route_album_identity(
    bob_dir: &Path,
    sourced: &SourcedRecordType,
) -> io::Result<AlbumRecordIdentity> {
    route_album_identity_with_source(
        sourced,
        RoutePresentationSource::new(bob_dir),
    )
}

fn route_album_identity_with_source(
    sourced: &SourcedRecordType,
    source: RoutePresentationSource,
) -> io::Result<AlbumRecordIdentity> {
    let RecordTypeSource::Album { path } = &sourced.source else {
        unreachable!("only album routes need presentation metadata")
    };
    let relative_path = PathBuf::from(path);
    let album_head = source.read_head(&relative_path)?.unwrap_or_default();
    let declaration =
        parse_album_declaration(&relative_path, &album_head).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "album {} changed while resolving its presentation metadata",
                    relative_path.display()
                ),
            )
        })?;

    let mut resolver = HomeResolver::new(source);
    let home = resolver.resolve_note(&relative_path);
    let owner = home
        .residence_chain
        .get(1)
        .and_then(|path| Path::new(path).file_stem())
        .and_then(|stem| stem.to_str())
        .map(str::to_owned);
    let breadcrumb = if home.key == "@unfiled" {
        vec![declaration.title.clone()]
    } else {
        let mut paths = home
            .residence_chain
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if let Some(root_at) = paths
            .iter()
            .position(|candidate| display_slash(candidate) == home.root_parent)
        {
            paths.truncate(root_at + 1);
        }
        paths.reverse();
        if !paths.iter().any(|candidate| candidate == &relative_path) {
            paths.push(relative_path.clone());
        }
        paths
            .iter()
            .filter_map(|candidate| {
                if candidate == &relative_path {
                    return Some(declaration.title.clone());
                }
                let head =
                    resolver.source_mut().read_head(candidate).ok()??;
                let stem = candidate
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default();
                let frontmatter = parse_frontmatter(&head)?;
                if frontmatter_is_area(&frontmatter)
                    || frontmatter_is_project(&frontmatter)
                {
                    return Some(stem.to_owned());
                }
                Some(derive_title(&head, first_alias(&head).as_deref(), stem))
            })
            .collect()
    };

    Ok(AlbumRecordIdentity {
        stem: declaration.stem,
        title: declaration.title,
        owner,
        breadcrumb,
        schema_field_lines: declaration.schema_field_lines,
    })
}

/// A lazy, path-confined source used only for route presentation. It caches
/// the bounded note heads it reads so owner resolution and breadcrumb labels
/// do not read any target twice.
struct RoutePresentationSource {
    root: PathBuf,
    links: VaultLinkResolver,
    heads: RefCell<BTreeMap<PathBuf, Option<String>>>,
    #[cfg(test)]
    reads: Rc<RefCell<Vec<PathBuf>>>,
}

impl RoutePresentationSource {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            links: VaultLinkResolver::new(root),
            heads: RefCell::new(BTreeMap::new()),
            #[cfg(test)]
            reads: Rc::new(RefCell::new(Vec::new())),
        }
    }

    #[cfg(test)]
    fn with_read_recorder(
        root: &Path,
        reads: Rc<RefCell<Vec<PathBuf>>>,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            links: VaultLinkResolver::new(root),
            heads: RefCell::new(BTreeMap::new()),
            reads,
        }
    }

    fn read_head(&self, relative: &Path) -> io::Result<Option<String>> {
        if let Some(cached) = self.heads.borrow().get(relative) {
            return Ok(cached.clone());
        }
        let absolute = self.root.join(relative);
        #[cfg(test)]
        self.reads.borrow_mut().push(relative.to_path_buf());
        match fs::symlink_metadata(&absolute) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.heads.borrow_mut().insert(relative.to_path_buf(), None);
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
        if let Err(message) =
            registry::confine_vault_file(&self.root, &absolute)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                message,
            ));
        }
        let root = fs::canonicalize(&self.root)?;
        let target = fs::canonicalize(&absolute)?;
        if !target.starts_with(&root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "note {} resolves outside the vault",
                    absolute.display()
                ),
            ));
        }
        let result = read_presentation_head(&absolute)?;
        self.heads
            .borrow_mut()
            .insert(relative.to_path_buf(), result.clone());
        Ok(result)
    }
}

impl HomeSource for RoutePresentationSource {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution {
        self.links.resolve(current, target)
    }

    fn read_facts(&self, path: &Path) -> Option<NoteFacts> {
        let head = self.read_head(path).ok()??;
        Some(NoteFacts::parse(&head))
    }
}

fn read_presentation_head(path: &Path) -> io::Result<Option<String>> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut head = String::new();
    let mut bytes = 0usize;
    let mut raw_line = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_seen = false;
    let mut fence = None;

    loop {
        raw_line.clear();
        let read = reader.read_until(b'\n', &mut raw_line)?;
        if read == 0 {
            break;
        }
        if bytes.saturating_add(read) > PRESENTATION_HEAD_LIMIT {
            break;
        }
        bytes += read;
        let line = String::from_utf8_lossy(&raw_line);
        let line = line.as_ref();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !frontmatter_seen {
            if trimmed != "---" {
                return Ok(None);
            }
            frontmatter_seen = true;
            in_frontmatter = true;
            head.push_str(line);
            continue;
        }
        if in_frontmatter {
            head.push_str(line);
            if trimmed == "---" {
                in_frontmatter = false;
            }
            continue;
        }

        head.push_str(line);
        // The title is selected from the first H1 before an H2/column bullet.
        // Read one bounded head, keeping the shared title parser authoritative.
        if rules::track_fence(trimmed, &mut fence) || fence.is_some() {
            continue;
        }
        let heading = trimmed.trim_start();
        if heading.starts_with("# ")
            || heading == "#"
            || heading.starts_with("## ")
            || heading == "##"
        {
            break;
        }
    }

    if !frontmatter_seen || in_frontmatter {
        return Ok(None);
    }
    Ok(Some(head))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, path::Path, rc::Rc};

    use super::{
        route_album_identity_with_source, AlbumRecordIdentity, RecordContext,
        RecordKindResolution, RoutePresentationSource,
    };
    use crate::native::albums::registry::{self, RouteResolution};

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(path, text).expect("write fixture");
    }

    fn album_vault(root: &Path) {
        write(
            &root.join("dev.md"),
            "---\ntype: [[project]]\nparent: \"[[body]]\"\nroot: true\n---\n# Development\n",
        );
        write(&root.join("body.md"), "---\ntype: [[area]]\n---\n# Body\n");
        write(
            &root.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[dev]]\"\nsummary: Places\nrecord:\n  noun: restaurant\n  label: Restaurant\n  fields: {}\n---\n# Restaurants and Cafes\n",
        );
    }

    #[test]
    fn full_context_resolves_noun_and_stem_with_shared_album_metadata() {
        let temp = tempfile::tempdir().expect("fixture");
        let vault = temp.path().join("vault");
        let config = temp.path().join("config.yml");
        album_vault(&vault);

        let context = RecordContext::discover(&vault, &config);
        let by_noun = context.lookup_noun("restaurant");
        let by_stem = context.lookup_stem("eat");
        let RecordKindResolution::Valid { kind, warnings, .. } = by_noun else {
            panic!("expected album kind")
        };
        assert!(warnings.is_empty());
        assert_eq!(kind.record_type().id, "restaurant");
        assert_eq!(kind.record_type().dir, "eat");
        assert_eq!(
            kind.album,
            Some(AlbumRecordIdentity {
                stem: "eat".to_owned(),
                title: "Restaurants and Cafes".to_owned(),
                owner: Some("dev".to_owned()),
                breadcrumb: vec![
                    "dev".to_owned(),
                    "Restaurants and Cafes".to_owned()
                ],
                schema_field_lines: HashMap::new(),
            })
        );
        assert!(matches!(
            by_stem,
            RecordKindResolution::Valid { kind, .. }
                if kind.record_type().id == "restaurant"
        ));
        assert_eq!(
            kind.metadata_json(),
            serde_json::json!({
                "source": "album",
                "album": {
                    "stem": "eat",
                    "title": "Restaurants and Cafes",
                    "owner": "dev",
                    "breadcrumb": ["dev", "Restaurants and Cafes"]
                }
            })
        );
    }

    #[test]
    fn route_metadata_reads_only_the_album_and_its_owner_chain() {
        let temp = tempfile::tempdir().expect("fixture");
        let vault = temp.path().join("vault");
        let config = temp.path().join("config.yml");
        album_vault(&vault);
        fs::create_dir_all(vault.join("eat")).expect("record folder");
        fs::write(vault.join("eat/record.md"), [0xff, 0xfe])
            .expect("binary record body");
        fs::write(vault.join("unrelated.md"), [0xff, 0xfe])
            .expect("binary unrelated body");

        let RouteResolution::AlbumValid { record_type, .. } =
            registry::resolve_route_head(&vault, &config, "eat", None)
                .expect("route head")
        else {
            panic!("expected route-valid album")
        };
        let reads = Rc::new(std::cell::RefCell::new(Vec::new()));
        let source =
            RoutePresentationSource::with_read_recorder(&vault, reads.clone());
        let identity = route_album_identity_with_source(&record_type, source)
            .expect("bounded presentation metadata");

        assert_eq!(identity.title, "Restaurants and Cafes");
        assert_eq!(identity.owner.as_deref(), Some("dev"));
        assert_eq!(identity.breadcrumb, vec!["dev", "Restaurants and Cafes"]);
        let paths = reads
            .borrow()
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            paths,
            std::collections::BTreeSet::from([
                "body.md".to_owned(),
                "dev.md".to_owned(),
                "eat.md".to_owned(),
            ])
        );
    }

    #[test]
    fn contexts_keep_same_stem_schemas_scoped_to_their_vault() {
        let temp = tempfile::tempdir().expect("fixture");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let config = temp.path().join("config.yml");
        album_vault(&first);
        album_vault(&second);
        write(
            &first.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[dev]]\"\nrecord:\n  noun: restaurant\n  fields: {rating: {values: number}}\n---\n# First\n",
        );
        write(
            &second.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[dev]]\"\nrecord:\n  noun: restaurant\n  fields: {cuisine: {}}\n---\n# Second\n",
        );

        let first_kind =
            RecordContext::discover(&first, &config).resolve("eat");
        let second_kind =
            RecordContext::discover(&second, &config).resolve("eat");
        let RecordKindResolution::Valid { kind: first, .. } = first_kind else {
            panic!("first vault album should resolve")
        };
        let RecordKindResolution::Valid { kind: second, .. } = second_kind
        else {
            panic!("second vault album should resolve")
        };
        assert_eq!(first.record_type().fields[0].key, "rating");
        assert_eq!(second.record_type().fields[0].key, "cuisine");
        assert_eq!(first.album.as_ref().unwrap().title, "First");
        assert_eq!(second.album.as_ref().unwrap().title, "Second");
    }

    #[test]
    fn config_fallback_exposes_warning_without_album_metadata() {
        let temp = tempfile::tempdir().expect("fixture");
        let vault = temp.path().join("vault");
        let config = temp.path().join("config.yml");
        fs::create_dir_all(&vault).expect("vault");
        write(
            &config,
            "records:\n  book:\n    dir: books\n    label: Book\n    fields: {}\n",
        );

        let context = RecordContext::discover(&vault, &config);
        let RecordKindResolution::Valid { kind, warnings, .. } =
            context.lookup_noun("book")
        else {
            panic!("expected config kind")
        };
        assert_eq!(kind.source_name(), "config");
        assert_eq!(kind.album, None);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "records_config_deprecated");
        assert_eq!(
            kind.metadata_json(),
            serde_json::json!({"source": "config"})
        );
    }

    #[test]
    fn invalid_album_claims_remain_available_and_suppress_config_fallback() {
        let temp = tempfile::tempdir().expect("fixture");
        let vault = temp.path().join("vault");
        let config = temp.path().join("config.yml");
        write(&vault.join("body.md"), "---\ntype: [[area]]\n---\n# Body\n");
        write(
            &vault.join("eat.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nrecord: {noun: restaurant, fields: []}\n---\n# Restaurants\n",
        );
        write(
            &config,
            "records:\n  restaurant:\n    dir: eat\n    label: Restaurant\n    fields: {}\n",
        );

        let context = RecordContext::discover(&vault, &config);
        assert_eq!(context.invalid_album_claims().len(), 1);
        assert!(context.album_claims()[0].usable_type.is_none());
        assert!(matches!(
            context.resolve("restaurant"),
            RecordKindResolution::Invalid { .. }
        ));
        assert!(context
            .usable_kinds()
            .iter()
            .all(|kind| { kind.record_type().id != "restaurant" }));
    }
}

//! Guarded record-link inputs for `bob bank import`.
//!
//! Generalizes `load_restaurants` into one record-registry read. One
//! `RecordContext` supplies both restaurant and subscription records,
//! preserving root Markdown membership, every consulted album declaration,
//! parent and registry dependencies, record bytes, and both record folders'
//! membership guards. Missing folders are guarded too, so a newly
//! appearing record invalidates the plan. Subscription semantics are parsed
//! from the exact captured bytes, and the same bytes are guarded at
//! transaction apply, with no unguarded second semantic read.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::native::{
    albums::registry::{AlbumRegistry, DiscoveryHealth, RecordTypeSource},
    bank::{
        import_plan::{ReadDep, RestaurantEntry},
        subscriptions::{
            parse_subscription_snapshot,
            registry::{
                SubscriptionAlbum, SubscriptionAvailability,
                SubscriptionRegistry,
            },
        },
    },
    notebooks::snapshot::VaultSnapshot,
    projects::wikilink_path_target,
    records::{
        catalog::RecordCatalog,
        context::{RecordContext, RecordKindResolution},
        schema::RecordDiagnosticCode,
    },
    vault_rewrite::TxnFailure,
};

/// Guarded inputs for one import plan: restaurants plus subscriptions.
pub(crate) struct GuardedRecordInputs {
    pub(crate) restaurants: Vec<RestaurantEntry>,
    pub(crate) restaurant_deps: Vec<ReadDep>,
    pub(crate) restaurant_directory: Option<String>,
    pub(crate) restaurant_dir_entries: Vec<String>,
    pub(crate) subscriptions: SubscriptionRegistry,
    pub(crate) subscription_deps: Vec<ReadDep>,
    pub(crate) subscription_directory: Option<String>,
    pub(crate) subscription_dir_entries: Vec<String>,
    /// Bounded warning when the subscription declaration is invalid.
    /// Imports proceed without subscription facts in that case.
    pub(crate) subscription_warning: Option<String>,
    pub(crate) registry_dependencies: Vec<(PathBuf, Option<Vec<u8>>)>,
    pub(crate) root_md_entries: Vec<String>,
}

fn failure(
    code: &'static str,
    message: impl Into<String>,
    hint: &str,
) -> TxnFailure {
    TxnFailure::new(code, message, Some(hint.to_owned()))
}

fn directory_names(
    directory: &Path,
    extension: Option<&str>,
) -> Result<Vec<String>, TxnFailure> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(failure(
                "bank_import_incomplete_read",
                format!("could not read {}: {error}", directory.display()),
                "fix the directory permissions and retry",
            ));
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            failure(
                "bank_import_incomplete_read",
                format!("could not read {}: {error}", directory.display()),
                "fix the directory and retry",
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if extension.is_none_or(|extension| {
            Path::new(&name)
                .extension()
                .is_some_and(|actual| actual == extension)
        }) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn read_preimage(
    vault: &Path,
    relative: &Path,
    listed_root: bool,
) -> Result<Option<Vec<u8>>, TxnFailure> {
    match fs::read(vault.join(relative)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if listed_root {
                Err(failure(
                    "bank_import_registry",
                    "the records registry changed while it was being read",
                    "re-run the import against the latest registry",
                ))
            } else {
                Ok(None)
            }
        }
        Err(error) => Err(failure(
            "bank_import_registry",
            format!("could not read {}: {error}", relative.display()),
            "repair the records registry and retry",
        )),
    }
}

fn insert_preimage(
    vault: &Path,
    relative: PathBuf,
    listed_root: bool,
    seen: &mut BTreeSet<PathBuf>,
    deps: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
) -> Result<(), TxnFailure> {
    if !seen.insert(relative.clone()) {
        return Ok(());
    }
    let bytes = read_preimage(vault, &relative, listed_root)?;
    deps.push((relative, bytes));
    Ok(())
}

fn capture_registry_preimages(
    vault: &Path,
    root_md_entries: &[String],
    snapshot: &VaultSnapshot,
    context: &RecordContext,
) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>, TxnFailure> {
    let mut seen = BTreeSet::new();
    let mut deps = Vec::new();
    for name in root_md_entries {
        insert_preimage(
            vault,
            PathBuf::from(name),
            true,
            &mut seen,
            &mut deps,
        )?;
    }
    let index = snapshot.index();
    for album in &context.registry().albums {
        let album_path = PathBuf::from(&album.declaration.path);
        if album.contents.as_ref().is_some_and(|contents| {
            deps.iter().any(|(path, bytes)| {
                path == &album_path
                    && bytes
                        .as_ref()
                        .is_some_and(|bytes| bytes != contents.as_bytes())
            })
        }) {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "{} changed while the records registry was read",
                    album.declaration.path
                ),
                "re-run the import against the latest registry",
            ));
        }
        insert_preimage(vault, album_path, false, &mut seen, &mut deps)?;
        let mut current = album.parent_path.clone();
        let mut walked = BTreeSet::new();
        while let Some(path) = current {
            if !walked.insert(path.clone()) {
                break;
            }
            insert_preimage(vault, path.clone(), false, &mut seen, &mut deps)?;
            current = snapshot.files.get(&path).and_then(|file| {
                file.lite.parent_raw.as_deref().and_then(|raw| {
                    let target = wikilink_path_target(raw)?;
                    match index.resolve(Some(&path), &target) {
                        Some(resolved) => Some(resolved),
                        None => {
                            Some(PathBuf::from(if target.ends_with(".md") {
                                target
                            } else {
                                format!("{target}.md")
                            }))
                        }
                    }
                })
            });
        }
    }
    Ok(deps)
}

/// Load restaurants and subscriptions under one `RecordContext`, preserving
/// the existing `eat` exact-title behavior.
pub(crate) fn load_record_links(
    vault: &Path,
) -> Result<GuardedRecordInputs, TxnFailure> {
    let root_md_entries = directory_names(vault, Some("md"))?;
    let snapshot = VaultSnapshot::load(vault);
    if directory_names(vault, Some("md"))? != root_md_entries {
        return Err(failure(
            "bank_import_registry",
            "the records registry changed while it was being read",
            "re-run the import against the latest registry",
        ));
    }
    if !snapshot.io_errors.is_empty() {
        return Err(failure(
            "bank_import_registry",
            format!(
                "could not read the records registry: {}",
                snapshot.io_errors.join(", ")
            ),
            "repair the records registry and retry",
        ));
    }
    let context = RecordContext::from_registry(AlbumRegistry::from_snapshot(
        vault, &snapshot,
    ));
    let registry_dependencies = capture_registry_preimages(
        vault,
        &root_md_entries,
        &snapshot,
        &context,
    )?;

    if let DiscoveryHealth::Partial(errors) = &context.registry().health {
        return Err(failure(
            "bank_import_registry",
            format!(
                "could not read the records registry: {}",
                errors.join(", ")
            ),
            "repair the records registry and retry",
        ));
    }

    let (
        restaurants,
        restaurant_deps,
        restaurant_directory,
        restaurant_dir_entries,
    ) = load_restaurants_from(&context, vault)?;
    let (
        subscriptions,
        subscription_deps,
        subscription_directory,
        subscription_dir_entries,
        subscription_warning,
    ) = load_subscriptions_from(&context, vault)?;

    Ok(GuardedRecordInputs {
        restaurants,
        restaurant_deps,
        restaurant_directory,
        restaurant_dir_entries,
        subscriptions,
        subscription_deps,
        subscription_directory,
        subscription_dir_entries,
        subscription_warning,
        registry_dependencies,
        root_md_entries,
    })
}

fn load_restaurants_from(
    context: &RecordContext,
    vault: &Path,
) -> Result<
    (
        Vec<RestaurantEntry>,
        Vec<ReadDep>,
        Option<String>,
        Vec<String>,
    ),
    TxnFailure,
> {
    let kind = match context.resolve("eat") {
        RecordKindResolution::Valid { kind, .. } => Some(kind),
        RecordKindResolution::DiscoveryIncomplete { errors } => {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "could not read the restaurant registry: {}",
                    errors.join(", ")
                ),
                "repair the records registry and retry",
            ));
        }
        _ => None,
    };
    let mut entries = Vec::new();
    let mut deps = Vec::new();
    let mut restaurant_directory = None;
    let mut dir_entries = Vec::new();
    if let Some(kind) = kind {
        let record_type = kind.record_type();
        let directory = &record_type.dir;
        restaurant_directory = Some(directory.clone());
        let record_directory = vault.join(directory);
        let before_entries = directory_names(&record_directory, Some("md"))?;
        let mut catalog = RecordCatalog::new(vault);
        let folder = catalog.folder(directory);
        dir_entries = directory_names(&record_directory, Some("md"))?;
        if dir_entries != before_entries {
            return Err(failure(
                "bank_import_registry",
                format!("{directory}/ changed while its records were read"),
                "re-run the import against the latest registry",
            ));
        }
        for record in folder.notes() {
            let full_path = vault.join(&record.relative_path);
            let bytes = fs::read(&full_path).map_err(|error| {
                failure(
                    "bank_import_registry",
                    format!(
                        "could not read {}: {error}",
                        record.relative_path.display()
                    ),
                    "repair the restaurant note and retry",
                )
            })?;
            deps.push(ReadDep {
                path: record.relative_path.to_string_lossy().replace('\\', "/"),
                preimage: Some(bytes.clone()),
            });
            if record.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == RecordDiagnosticCode::NoteRead
            }) {
                return Err(failure(
                    "bank_import_registry",
                    format!(
                        "could not read {} from the restaurant registry",
                        record.relative_path.display()
                    ),
                    "repair the restaurant note and retry",
                ));
            }
            let markdown = std::str::from_utf8(&bytes).map_err(|error| {
                failure(
                    "bank_import_registry",
                    format!(
                        "{} is not readable UTF-8: {error}",
                        record.relative_path.display()
                    ),
                    "repair the restaurant note and retry",
                )
            })?;
            let current = crate::native::records::staged_record(
                record.relative_path.clone(),
                record.stem.clone(),
                markdown,
            );
            if current.title != record.title {
                return Err(failure(
                    "bank_import_registry",
                    format!(
                        "{} changed while its title was read",
                        record.relative_path.display()
                    ),
                    "re-run the import against the latest registry",
                ));
            }
            if let Some(title) = &record.title {
                let mut link_path = record.relative_path.clone();
                link_path.set_extension("");
                entries.push(RestaurantEntry {
                    title: title.clone(),
                    path: link_path.to_string_lossy().replace('\\', "/"),
                    emoji: record_type.emoji.clone(),
                });
            }
        }
    }
    Ok((entries, deps, restaurant_directory, dir_entries))
}

fn load_subscriptions_from(
    context: &RecordContext,
    vault: &Path,
) -> Result<
    (
        SubscriptionRegistry,
        Vec<ReadDep>,
        Option<String>,
        Vec<String>,
        Option<String>,
    ),
    TxnFailure,
> {
    let kind = match context.lookup_noun("subscription") {
        RecordKindResolution::Valid { kind, .. }
            if kind.record_type().id == "subscription" =>
        {
            Some(kind)
        }
        // Do not mistake an unrelated record kind named `subscription`
        // for this album; a genuinely missing album is a quiet empty
        // snapshot with no warning.
        RecordKindResolution::Valid { .. }
        | RecordKindResolution::Unclaimed { .. } => None,
        RecordKindResolution::Invalid { .. } => {
            return Ok((
                SubscriptionRegistry::missing_invalid(),
                Vec::new(),
                None,
                Vec::new(),
                Some(
                    "subscription records are unavailable; run `bob albums check`"
                        .to_owned(),
                ),
            ));
        }
        RecordKindResolution::DiscoveryIncomplete { errors } => {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "could not read the subscription registry: {}",
                    errors.join(", ")
                ),
                "repair the records registry and retry",
            ));
        }
    };
    let Some(kind) = kind else {
        return Ok((
            SubscriptionRegistry::missing(),
            Vec::new(),
            None,
            Vec::new(),
            None,
        ));
    };
    let record_type = kind.record_type();
    let directory = record_type.dir.clone();
    let record_directory = vault.join(&directory);
    let before_entries = directory_names(&record_directory, Some("md"))?;
    let mut catalog = RecordCatalog::new(vault);
    let folder = catalog.folder(&directory);
    let dir_entries = directory_names(&record_directory, Some("md"))?;
    if dir_entries != before_entries {
        return Err(failure(
            "bank_import_registry",
            format!("{directory}/ changed while its records were read"),
            "re-run the import against the latest registry",
        ));
    }
    if let Some(message) = folder.create_refusal() {
        return Err(failure(
            "bank_import_registry",
            format!("could not read {directory}/: {message}"),
            "repair the subscription records folder and retry",
        ));
    }
    if folder
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == RecordDiagnosticCode::NoteRead)
    {
        return Err(failure(
            "bank_import_registry",
            format!("could not read {directory}/ subscription records"),
            "repair the subscription records folder and retry",
        ));
    }
    let mut record_bytes = Vec::new();
    let mut deps = Vec::new();
    for indexed in folder.notes() {
        if indexed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == RecordDiagnosticCode::NoteRead)
        {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "could not read {} from the subscription registry",
                    indexed.relative_path.display()
                ),
                "repair the subscription note and retry",
            ));
        }
        let full_path = vault.join(&indexed.relative_path);
        let bytes = fs::read(&full_path).map_err(|error| {
            failure(
                "bank_import_registry",
                format!(
                    "could not read {}: {error}",
                    indexed.relative_path.display()
                ),
                "repair the subscription note and retry",
            )
        })?;
        if std::str::from_utf8(&bytes).is_err() {
            return Err(failure(
                "bank_import_registry",
                format!(
                    "{} is not readable UTF-8",
                    indexed.relative_path.display()
                ),
                "repair the subscription note and retry",
            ));
        }
        deps.push(ReadDep {
            path: indexed.relative_path.to_string_lossy().replace('\\', "/"),
            preimage: Some(bytes.clone()),
        });
        record_bytes.push((
            indexed.relative_path.clone(),
            indexed.stem.clone(),
            bytes,
        ));
    }
    // Parse semantics from the exact captured bytes; the same bytes are
    // guarded at transaction apply.
    let RecordTypeSource::Album { path } = &kind.sourced.source;
    let album = SubscriptionAlbum {
        stem: kind
            .album
            .as_ref()
            .map(|identity| identity.stem.clone())
            .unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_owned()
            }),
        path: path.clone(),
        title: kind
            .album
            .as_ref()
            .map(|identity| identity.title.clone())
            .unwrap_or_else(|| record_type.label.clone()),
        directory: record_type.dir.clone(),
        emoji: record_type.emoji.clone(),
    };
    let registry =
        parse_subscription_snapshot(album.clone(), record_type, &record_bytes);
    if registry.availability == SubscriptionAvailability::Invalid {
        return Ok((
            registry,
            deps,
            Some(directory),
            dir_entries,
            Some(
                "subscription records are unavailable; run `bob albums check`"
                    .to_owned(),
            ),
        ));
    }
    Ok((registry, deps, Some(directory), dir_entries, None))
}

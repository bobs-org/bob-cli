//! Write-once saved Markdown sources and their render-time code catalogs.
//!
//! Layout (vault-relative):
//! `.bob/ref-sources/<ref_type>/<stem>--<hash12>/source.md` plus `code.json`.
//! `hash12` is the first 12 hex digits of SHA-256(`source.md`). `source.md`
//! is write-once; `code.json` is derived data that a later render of the
//! same bytes may replace atomically. Callers hold `bob_sync.lock` across
//! [`write_store`]; readers treat a directory without `code.json` as
//! `no_catalog`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::code_catalog::{
    CatalogLink, CatalogOutcome, CodeBlock, CodeSection,
};
use super::model::{CommandError, Result};

pub(crate) const STORE_ROOT: &str = ".bob/ref-sources";
pub(crate) const STORE_VERSION: u32 = 1;

/// Allowlist line for the vault `.gitignore` so catalogs sync.
pub(crate) const CODE_JSON_ALLOWLIST: &str = "!/.bob/ref-sources/**/*.json";

/// Warning when `code.json` is gitignored on this host.
pub(crate) fn ignored_catalog_warning() -> String {
    format!(
        "warning: code catalogs are ignored by the vault .gitignore; code copy won't reach other hosts — add \"{CODE_JSON_ALLOWLIST}\" (docs/vault-git-sync.md#reference-sources)"
    )
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn hash12_for(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..12].to_string()
}

/// Vault-relative store directory for a stem and content hash.
pub(crate) fn store_dir_rel(
    ref_type: &str,
    stem: &str,
    hash12: &str,
) -> PathBuf {
    PathBuf::from(format!("{STORE_ROOT}/{ref_type}/{stem}--{hash12}"))
}

/// Vault-relative pointer value stored in `source_markdown` frontmatter.
pub(crate) fn pointer_for(ref_type: &str, stem: &str, hash12: &str) -> String {
    format!("{STORE_ROOT}/{ref_type}/{stem}--{hash12}/source.md")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredSource {
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
    pub(crate) origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_note_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredPdf {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredRenderer {
    pub(crate) bob: String,
    pub(crate) pandoc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredBlock {
    pub(crate) n: u32,
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) classes: Vec<String>,
    #[serde(default)]
    pub(crate) attrs: BTreeMap<String, String>,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) section: Option<CodeSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page_source: Option<String>,
}

impl From<&CodeBlock> for StoredBlock {
    fn from(block: &CodeBlock) -> Self {
        Self {
            n: block.n,
            kind: block.kind.clone(),
            lang: block.lang.clone(),
            classes: block.classes.clone(),
            attrs: block.attrs.clone(),
            text: block.text.clone(),
            section: block.section.clone(),
            page: block.page,
            page_end: block.page_end,
            page_source: block.page_source.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredVerified {
    pub(crate) matched: usize,
    pub(crate) total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredLink {
    pub(crate) n: u32,
    pub(crate) url: String,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) autolink: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) section: Option<CodeSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page_source: Option<String>,
}

impl From<&CatalogLink> for StoredLink {
    fn from(link: &CatalogLink) -> Self {
        Self {
            n: link.n,
            url: link.url.clone(),
            text: link.text.clone(),
            title: link.title.clone(),
            autolink: link.autolink,
            context: link.context.clone(),
            section: link.section.clone(),
            page: link.page,
            page_source: link.page_source.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredCatalog {
    pub(crate) version: u32,
    pub(crate) captured_at: String,
    pub(crate) route: String,
    pub(crate) source: StoredSource,
    pub(crate) pdf: StoredPdf,
    pub(crate) renderer: StoredRenderer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) catalog_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verified: Option<StoredVerified>,
    #[serde(default)]
    pub(crate) blocks: Vec<StoredBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) links: Option<Vec<StoredLink>>,
}

/// Outcome of [`write_store`]: the vault-relative directory and pointer,
///
/// plus whether `code.json` was (re)written and any warning for the caller
/// to print without failing the create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreWriteOutcome {
    pub(crate) dir_rel: PathBuf,
    pub(crate) pointer: String,
    pub(crate) code_written: bool,
    pub(crate) reused: bool,
    pub(crate) warning: Option<String>,
}

/// Parameters for one store write.
#[derive(Debug, Clone)]
pub(crate) struct StoreWriteParams<'a> {
    pub(crate) ref_type: &'a str,
    pub(crate) stem: &'a str,
    pub(crate) source_bytes: &'a [u8],
    pub(crate) route: &'a str,
    pub(crate) origin: String,
    pub(crate) source_note: Option<String>,
    pub(crate) source_note_sha256: Option<String>,
    pub(crate) pdf_rel: String,
    pub(crate) pdf_sha256: String,
    pub(crate) catalog: &'a CatalogOutcome,
    pub(crate) verified: Option<StoredVerified>,
}

fn vault_rel_forward(bob_dir: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(bob_dir) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    if let (Ok(path_c), Ok(bob_c)) =
        (std::fs::canonicalize(path), std::fs::canonicalize(bob_dir))
        && let Ok(rel) = path_c.strip_prefix(&bob_c)
    {
        return rel.to_string_lossy().replace('\\', "/");
    }
    path.to_string_lossy().replace('\\', "/")
}

fn pandoc_version_string() -> String {
    // Integration tests point `BOB_PANDOC_COMMAND` at a logging fake that
    // overwrites its args log on every invocation. Probing `--version`
    // after the main render would clobber the render args the test asserts
    // on, so skip the probe when that fixture is active.
    if std::env::var_os("BOB_TEST_FIXTURE_PDF").is_some() {
        return "test".to_string();
    }
    let command = super::create::pandoc_command();
    let Some(command) = command else {
        return "unknown".to_string();
    };
    let output = std::process::Command::new(&command)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            text.lines()
                .next()
                .unwrap_or("pandoc unknown")
                .trim()
                .to_string()
        }
        _ => "unknown".to_string(),
    }
}

/// Write `source.md` (write-once) and `code.json` (derived) under
/// `.bob/ref-sources`. The caller holds `bob_sync.lock`.
pub(crate) fn write_store(
    bob_dir: &Path,
    params: StoreWriteParams<'_>,
) -> Result<StoreWriteOutcome> {
    let digest = sha256_hex(params.source_bytes);
    let hash12 = digest[..12].to_string();
    let dir_rel = store_dir_rel(params.ref_type, params.stem, &hash12);
    let pointer = pointer_for(params.ref_type, params.stem, &hash12);
    let dir_abs = bob_dir.join(&dir_rel);
    let source_abs = dir_abs.join("source.md");
    let code_abs = dir_abs.join("code.json");

    if let Err(error) = fs::create_dir_all(&dir_abs).map_err(|error| {
        CommandError::new(format!(
            "create source store {}: {error}",
            dir_abs.display()
        ))
    }) {
        return Err(error);
    }

    // Write-once: an existing `source.md` with a different digest refuses
    // the write with a warning and never overwrites.
    if source_abs.is_file() {
        let existing = fs::read(&source_abs).map_err(|error| {
            CommandError::new(format!(
                "read source store {}: {error}",
                source_abs.display()
            ))
        })?;
        if sha256_hex(&existing) != digest {
            let warning = format!(
                "warning: source store {} already holds different bytes; refusing to overwrite",
                dir_rel.display()
            );
            return Ok(StoreWriteOutcome {
                dir_rel,
                pointer,
                code_written: false,
                reused: false,
                warning: Some(warning),
            });
        }
        // Same bytes: refresh `code.json` only when it differs.
        let catalog = build_stored_catalog(&params, &digest)?;
        let rendered =
            serde_json::to_string_pretty(&catalog).map_err(|error| {
                CommandError::new(format!(
                    "encode source catalog {}: {error}",
                    code_abs.display()
                ))
            })?;
        let mut code_written = false;
        let existing_code = fs::read_to_string(&code_abs).unwrap_or_default();
        if existing_code != rendered {
            super::reference_mutation::atomic_write_bytes(
                &code_abs,
                rendered.as_bytes(),
            )
            .map_err(|error| {
                CommandError::new(format!(
                    "write source catalog {}: {error}",
                    code_abs.display()
                ))
            })?;
            code_written = true;
        }
        let warning = ignored_catalog_warning_if_ignored(bob_dir, &dir_rel);
        return Ok(StoreWriteOutcome {
            dir_rel,
            pointer,
            code_written,
            reused: true,
            warning,
        });
    }

    super::reference_mutation::atomic_write_bytes(
        &source_abs,
        params.source_bytes,
    )
    .map_err(|error| {
        CommandError::new(format!(
            "write source store {}: {error}",
            source_abs.display()
        ))
    })?;
    let catalog = build_stored_catalog(&params, &digest)?;
    let rendered = serde_json::to_string_pretty(&catalog).map_err(|error| {
        CommandError::new(format!(
            "encode source catalog {}: {error}",
            code_abs.display()
        ))
    })?;
    super::reference_mutation::atomic_write_bytes(
        &code_abs,
        rendered.as_bytes(),
    )
    .map_err(|error| {
        CommandError::new(format!(
            "write source catalog {}: {error}",
            code_abs.display()
        ))
    })?;
    let warning = ignored_catalog_warning_if_ignored(bob_dir, &dir_rel);
    Ok(StoreWriteOutcome {
        dir_rel,
        pointer,
        code_written: true,
        reused: false,
        warning,
    })
}

fn build_stored_catalog(
    params: &StoreWriteParams<'_>,
    digest: &str,
) -> Result<StoredCatalog> {
    let (blocks, links, catalog_error) = match params.catalog {
        CatalogOutcome::Catalog(catalog) => (
            catalog.blocks.iter().map(StoredBlock::from).collect(),
            Some(
                catalog
                    .links
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(StoredLink::from)
                    .collect(),
            ),
            None,
        ),
        CatalogOutcome::Missing => (
            Vec::new(),
            None,
            Some("code catalog report missing".to_string()),
        ),
        CatalogOutcome::Invalid(reason) => {
            (Vec::new(), None, Some(reason.clone()))
        }
    };
    Ok(StoredCatalog {
        version: STORE_VERSION,
        captured_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        route: params.route.to_string(),
        source: StoredSource {
            sha256: digest.to_string(),
            bytes: params.source_bytes.len(),
            origin: params.origin.clone(),
            source_note: params.source_note.clone(),
            source_note_sha256: params.source_note_sha256.clone(),
        },
        pdf: StoredPdf {
            path: params.pdf_rel.clone(),
            sha256: params.pdf_sha256.clone(),
        },
        renderer: StoredRenderer {
            bob: env!("CARGO_PKG_VERSION").to_string(),
            pandoc: pandoc_version_string(),
        },
        catalog_error,
        verified: params.verified.clone(),
        blocks,
        links,
    })
}

/// Quiet probe via the shared vault-git helper.
fn is_ignored(
    bob_dir: &Path,
    child_env: &super::ob::ChildEnv,
    rel: &Path,
) -> bool {
    crate::native::vault_git::is_ignored(bob_dir, child_env, rel)
}

fn ignored_catalog_warning_if_ignored(
    bob_dir: &Path,
    dir_rel: &Path,
) -> Option<String> {
    let child_env = super::ob::child_env();
    let probe = dir_rel.join("code.json");
    if is_ignored(bob_dir, &child_env, &probe) {
        Some(ignored_catalog_warning())
    } else {
        None
    }
}

/// Shared vault-git trackability probe for store paths. Used by create
/// (warn-only) and by overwrite (commit `source.md`, keep `code.json` on
/// disk, warn, and keep `prove_commit` sound).
pub(crate) fn code_catalog_ignored(bob_dir: &Path, dir_rel: &Path) -> bool {
    let child_env = super::ob::child_env();
    is_ignored(bob_dir, &child_env, &dir_rel.join("code.json"))
}

/// True when `path` is a vault-relative `code.json` under the source store.
pub(crate) fn is_store_code_json(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.starts_with(&format!("{STORE_ROOT}/"))
        && normalized.ends_with("/code.json")
}

/// Planned store identity for an overwrite of `source_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedSourceStore {
    pub(crate) dir_rel: PathBuf,
    pub(crate) pointer: String,
    pub(crate) source_rel: String,
    pub(crate) code_rel: String,
}

impl PlannedSourceStore {
    pub(crate) fn for_bytes(
        ref_type: &str,
        stem: &str,
        source_bytes: &[u8],
    ) -> Self {
        let (dir_rel, pointer) = planned_pointer(ref_type, stem, source_bytes);
        let code_rel = dir_rel
            .join("code.json")
            .to_string_lossy()
            .replace('\\', "/");
        Self {
            dir_rel,
            source_rel: pointer.clone(),
            pointer,
            code_rel,
        }
    }
}

/// Scratch copies of `source.md` and `code.json` ready to install inside
/// the overwrite transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagedStoreFiles {
    pub(crate) planned: PlannedSourceStore,
    pub(crate) scratch_source: PathBuf,
    pub(crate) scratch_code: PathBuf,
}

/// Write `source.md` and `code.json` into `scratch` (outside the vault)
/// using the same catalog encoding as [`write_store`].
pub(crate) fn stage_store_files(
    scratch: &Path,
    params: StoreWriteParams<'_>,
) -> Result<StagedStoreFiles> {
    let planned = PlannedSourceStore::for_bytes(
        params.ref_type,
        params.stem,
        params.source_bytes,
    );
    let digest = sha256_hex(params.source_bytes);
    let catalog = build_stored_catalog(&params, &digest)?;
    let rendered = serde_json::to_string_pretty(&catalog).map_err(|error| {
        CommandError::new(format!("encode staged source catalog: {error}"))
    })?;
    fs::create_dir_all(scratch).map_err(|error| {
        CommandError::new(format!(
            "create staged source store {}: {error}",
            scratch.display()
        ))
    })?;
    let scratch_source = scratch.join("source.md");
    let scratch_code = scratch.join("code.json");
    super::reference_mutation::atomic_write_bytes(
        &scratch_source,
        params.source_bytes,
    )
    .map_err(|error| {
        CommandError::new(format!(
            "stage source store {}: {error}",
            scratch_source.display()
        ))
    })?;
    super::reference_mutation::atomic_write_bytes(
        &scratch_code,
        rendered.as_bytes(),
    )
    .map_err(|error| {
        CommandError::new(format!(
            "stage source catalog {}: {error}",
            scratch_code.display()
        ))
    })?;
    Ok(StagedStoreFiles {
        planned,
        scratch_source,
        scratch_code,
    })
}

/// Birth lookup: the single store under
/// `.bob/ref-sources/<ref_type>/<stem>--*/code.json` whose `pdf.sha256`
/// equals the incoming PDF digest. `None` when there is no match or the
/// match is not unique.
pub(crate) fn find_birth_store(
    bob_dir: &Path,
    ref_type: &str,
    stem: &str,
    pdf_sha256: &str,
) -> Option<String> {
    let parent = bob_dir.join(STORE_ROOT).join(ref_type);
    let entries = fs::read_dir(&parent).ok()?;
    let prefix = format!("{stem}--");
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) {
            continue;
        }
        let code_path = entry.path().join("code.json");
        let raw = fs::read_to_string(&code_path).ok()?;
        let stored: StoredCatalog = serde_json::from_str(&raw).ok()?;
        if stored.version != STORE_VERSION {
            continue;
        }
        if stored.pdf.sha256 == pdf_sha256 {
            matches.push(pointer_for(ref_type, stem, &name[prefix.len()..]));
        }
    }
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// Read and verify a store pointer. Returns the verified bytes and catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreStatus {
    Available,
    NoCode,
    NoCatalog,
    NoSource,
    SourceMissing,
    SourceInvalid,
}

#[derive(Debug, Clone)]
pub(crate) struct StoreRead {
    pub(crate) status: StoreStatus,
    pub(crate) detail: Option<String>,
    pub(crate) bytes: Option<Vec<u8>>,
    pub(crate) catalog: Option<StoredCatalog>,
}

pub(crate) fn read_store(bob_dir: &Path, pointer: &str) -> StoreRead {
    let trimmed = pointer.trim();
    if trimmed.is_empty() {
        return StoreRead {
            status: StoreStatus::NoSource,
            detail: Some("no saved Markdown source".to_string()),
            bytes: None,
            catalog: None,
        };
    }
    let source_abs = bob_dir.join(trimmed);
    let dir = source_abs.parent().map(Path::to_path_buf);
    let code_abs = dir.as_ref().map(|dir| dir.join("code.json"));
    let bytes = match fs::read(&source_abs) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StoreRead {
                status: StoreStatus::SourceMissing,
                detail: Some(format!(
                    "saved Markdown {trimmed} is absent on this host"
                )),
                bytes: None,
                catalog: None,
            };
        }
        Err(error) => {
            return StoreRead {
                status: StoreStatus::SourceInvalid,
                detail: Some(format!(
                    "saved Markdown {trimmed} is unreadable: {error}"
                )),
                bytes: None,
                catalog: None,
            };
        }
    };
    let digest = sha256_hex(&bytes);
    let expected_hash = source_abs
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once("--"))
        .map(|(_, hash)| hash);
    if expected_hash != Some(&digest[..12]) {
        return StoreRead {
            status: StoreStatus::SourceInvalid,
            detail: Some(
                "saved Markdown does not match its store path digest"
                    .to_string(),
            ),
            bytes: None,
            catalog: None,
        };
    }
    let Some(code_abs) = code_abs else {
        return StoreRead {
            status: StoreStatus::SourceInvalid,
            detail: Some(format!(
                "saved Markdown pointer {trimmed} has no parent directory"
            )),
            bytes: None,
            catalog: None,
        };
    };
    let raw = match fs::read_to_string(&code_abs) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StoreRead {
                status: StoreStatus::NoCatalog,
                detail: Some(
                    "saved Markdown verified but no code catalog was recorded"
                        .to_string(),
                ),
                bytes: Some(bytes),
                catalog: None,
            };
        }
        Err(error) => {
            return StoreRead {
                status: StoreStatus::SourceInvalid,
                detail: Some(format!(
                    "saved source catalog is unreadable: {error}"
                )),
                bytes: None,
                catalog: None,
            };
        }
    };
    let catalog: StoredCatalog = match serde_json::from_str(&raw) {
        Ok(catalog) => catalog,
        Err(error) => {
            return StoreRead {
                status: StoreStatus::SourceInvalid,
                detail: Some(format!(
                    "saved source catalog is unreadable: {error}"
                )),
                bytes: None,
                catalog: None,
            };
        }
    };
    if catalog.version != STORE_VERSION {
        return StoreRead {
            status: StoreStatus::SourceInvalid,
            detail: Some(format!(
                "unsupported source catalog version {}",
                catalog.version
            )),
            bytes: None,
            catalog: None,
        };
    }
    if catalog.source.sha256 != digest {
        return StoreRead {
            status: StoreStatus::SourceInvalid,
            detail: Some(
                "saved Markdown does not match its recorded digest".to_string(),
            ),
            bytes: None,
            catalog: None,
        };
    }
    if catalog.catalog_error.is_some() {
        return StoreRead {
            status: StoreStatus::NoCatalog,
            detail: Some(
                catalog
                    .catalog_error
                    .clone()
                    .unwrap_or_else(|| "code catalog missing".to_string()),
            ),
            bytes: Some(bytes),
            catalog: Some(catalog),
        };
    }
    if catalog.blocks.is_empty() {
        return StoreRead {
            status: StoreStatus::NoCode,
            detail: Some("no code blocks in this reference".to_string()),
            bytes: Some(bytes),
            catalog: Some(catalog),
        };
    }
    StoreRead {
        status: StoreStatus::Available,
        detail: None,
        bytes: Some(bytes),
        catalog: Some(catalog),
    }
}

/// One doctor finding against the write-once source store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceStoreIssue {
    Dangling { note: String, pointer: String },
    DigestMismatch { note: String, pointer: String },
    MissingCatalog { note: String, pointer: String },
    Orphan { store: String },
}

/// Read-only health of `.bob/ref-sources` versus live note pointers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SourceStoreHealth {
    pub(crate) saved: usize,
    pub(crate) code_blocks: usize,
    pub(crate) links: usize,
    pub(crate) without_links: usize,
    pub(crate) dangling: Vec<SourceStoreIssue>,
    pub(crate) digest_mismatch: Vec<SourceStoreIssue>,
    pub(crate) missing_catalog: Vec<SourceStoreIssue>,
    pub(crate) orphans: Vec<SourceStoreIssue>,
    pub(crate) ignored_catalogs: usize,
}

/// Walk notes and the store tree. Never writes. Frozen notes under
/// `ref_history/` keep their `source_markdown` pointer, so they count as
/// live pointers when classifying orphans.
pub(crate) fn inspect_source_stores(
    bob_dir: &Path,
    rows: &[crate::native::ref_library::RefRow],
) -> SourceStoreHealth {
    let mut health = SourceStoreHealth::default();
    let mut pointed: BTreeSet<String> = BTreeSet::new();
    let mut ignored: BTreeSet<PathBuf> = BTreeSet::new();
    for row in rows {
        if let Some(pointer) = row.source_markdown.as_deref() {
            consider_pointer(
                bob_dir,
                &row.path,
                pointer,
                &mut health,
                &mut pointed,
                &mut ignored,
            );
        }
    }
    walk_history_pointers(bob_dir, &mut health, &mut pointed, &mut ignored);
    let root = bob_dir.join(STORE_ROOT);
    if let Ok(types) = fs::read_dir(&root) {
        for type_entry in types.flatten() {
            if !type_entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(stores) = fs::read_dir(type_entry.path()) else {
                continue;
            };
            for store in stores.flatten() {
                if !store.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let source = store.path().join("source.md");
                if !source.is_file() {
                    continue;
                }
                let rel = store
                    .path()
                    .strip_prefix(bob_dir)
                    .unwrap_or(&store.path())
                    .join("source.md");
                let pointer = rel.to_string_lossy().replace('\\', "/");
                let dir_rel = store
                    .path()
                    .strip_prefix(bob_dir)
                    .unwrap_or(&store.path())
                    .to_path_buf();
                if !pointed.contains(&pointer) {
                    health.orphans.push(SourceStoreIssue::Orphan {
                        store: dir_rel.to_string_lossy().replace('\\', "/"),
                    });
                }
                if code_catalog_ignored(bob_dir, &dir_rel) {
                    ignored.insert(dir_rel);
                }
            }
        }
    }
    health.ignored_catalogs = ignored.len();
    health
}

fn consider_pointer(
    bob_dir: &Path,
    note: &str,
    pointer: &str,
    health: &mut SourceStoreHealth,
    pointed: &mut BTreeSet<String>,
    ignored: &mut BTreeSet<PathBuf>,
) {
    let pointer = pointer.trim();
    if pointer.is_empty() {
        return;
    }
    let normalized = pointer.replace('\\', "/");
    let first_pointer = pointed.insert(normalized.clone());
    let read = read_store(bob_dir, &normalized);
    match read.status {
        StoreStatus::SourceMissing | StoreStatus::NoSource => {
            health.dangling.push(SourceStoreIssue::Dangling {
                note: note.to_string(),
                pointer: normalized.clone(),
            });
        }
        StoreStatus::SourceInvalid => {
            health
                .digest_mismatch
                .push(SourceStoreIssue::DigestMismatch {
                    note: note.to_string(),
                    pointer: normalized.clone(),
                });
        }
        StoreStatus::NoCatalog => {
            health
                .missing_catalog
                .push(SourceStoreIssue::MissingCatalog {
                    note: note.to_string(),
                    pointer: normalized.clone(),
                });
            if first_pointer {
                health.saved += 1;
            }
        }
        StoreStatus::NoCode | StoreStatus::Available => {
            if first_pointer {
                health.saved += 1;
                if let Some(catalog) = &read.catalog {
                    health.code_blocks += catalog.blocks.len();
                    match &catalog.links {
                        Some(links) => health.links += links.len(),
                        None => health.without_links += 1,
                    }
                }
            }
        }
    }
    if let Some(dir) = Path::new(&normalized).parent()
        && code_catalog_ignored(bob_dir, dir)
    {
        ignored.insert(dir.to_path_buf());
    }
}

fn pointer_from_contents(contents: &str) -> Option<String> {
    let (front, _) = super::frontmatter::split_frontmatter(contents)?;
    for line in front {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != super::FIELD_SOURCE_MARKDOWN {
            continue;
        }
        let value = value.trim().trim_matches('"').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn walk_history_pointers(
    bob_dir: &Path,
    health: &mut SourceStoreHealth,
    pointed: &mut BTreeSet<String>,
    ignored: &mut BTreeSet<PathBuf>,
) {
    walk_history_dir(
        &bob_dir.join("ref_history"),
        bob_dir,
        health,
        pointed,
        ignored,
    );
}

fn walk_history_dir(
    dir: &Path,
    bob_dir: &Path,
    health: &mut SourceStoreHealth,
    pointed: &mut BTreeSet<String>,
    ignored: &mut BTreeSet<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().ok();
        if file_type.is_some_and(|kind| kind.is_dir()) {
            if name.ends_with(".assets") {
                continue;
            }
            walk_history_dir(&path, bob_dir, health, pointed, ignored);
            continue;
        }
        if !name.ends_with(".md") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(pointer) = pointer_from_contents(&contents) else {
            continue;
        };
        let rel = path
            .strip_prefix(bob_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        consider_pointer(bob_dir, &rel, &pointer, health, pointed, ignored);
    }
}

/// Human `code:` summary for create output.
pub(crate) fn code_summary(catalog: &CatalogOutcome) -> String {
    match catalog {
        CatalogOutcome::Catalog(catalog) if catalog.blocks.is_empty() => {
            "none".to_string()
        }
        CatalogOutcome::Catalog(catalog) => {
            let mut langs = Vec::new();
            for block in &catalog.blocks {
                if let Some(lang) = block.lang.as_deref() {
                    let lang = lang.trim();
                    if !lang.is_empty() && !langs.contains(&lang.to_string()) {
                        langs.push(lang.to_string());
                    }
                }
            }
            let count = catalog.blocks.len();
            let noun = if count == 1 { "block" } else { "blocks" };
            if langs.is_empty() {
                format!("{count} {noun}")
            } else {
                let shown: Vec<String> = langs.into_iter().take(3).collect();
                format!("{count} {noun} ({})", shown.join(", "))
            }
        }
        CatalogOutcome::Missing => "not recorded (missing report)".to_string(),
        CatalogOutcome::Invalid(reason) => {
            format!("not recorded ({reason})")
        }
    }
}

/// Site label for one link URL: lowercase host without a leading `www.`
/// for web schemes, `email` for `mailto:`, else the scheme name.
pub(crate) fn link_site(url: &str) -> String {
    if url.len() >= 7 && url[..7].eq_ignore_ascii_case("mailto:") {
        return "email".to_string();
    }
    let Some(colon) = url.find(':') else {
        return url.to_string();
    };
    let scheme = url[..colon].to_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "ftps") {
        return scheme;
    }
    let mut rest = &url[colon + 1..];
    if let Some(stripped) = rest.strip_prefix("//") {
        rest = stripped;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut host = rest[..end].to_lowercase();
    if let Some(at) = host.rfind('@') {
        host = host[at + 1..].to_string();
    }
    if let Some(port) = host.find(':') {
        host = host[..port].to_string();
    }
    if let Some(stripped) = host.strip_prefix("www.") {
        host = stripped.to_string();
    }
    if host.is_empty() {
        scheme
    } else {
        host
    }
}

/// Human `links:` summary for create output: `14 (github.com, ...)` or
/// `none`. Counts every recorded occurrence; sites rank by frequency with
/// ties in first-appearance order.
pub(crate) fn link_summary(catalog: &CatalogOutcome) -> String {
    match catalog {
        CatalogOutcome::Catalog(catalog) => {
            let links = catalog.links.as_deref().unwrap_or(&[]);
            if links.is_empty() {
                return "none".to_string();
            }
            let mut counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            let mut order: Vec<String> = Vec::new();
            for link in links {
                let site = link_site(&link.url);
                if !counts.contains_key(&site) {
                    order.push(site.clone());
                }
                *counts.entry(site).or_insert(0) += 1;
            }
            order.sort_by(|left, right| {
                let left_count = counts.get(left).copied().unwrap_or(0);
                let right_count = counts.get(right).copied().unwrap_or(0);
                right_count.cmp(&left_count)
            });
            let count = links.len();
            let noun = if count == 1 { "link" } else { "links" };
            let shown: Vec<String> = order.into_iter().take(3).collect();
            format!("{count} {noun} ({})", shown.join(", "))
        }
        CatalogOutcome::Missing => "not recorded (missing report)".to_string(),
        CatalogOutcome::Invalid(reason) => {
            format!("not recorded ({reason})")
        }
    }
}

/// Planned pointer without writing, for dry runs.
pub(crate) fn planned_pointer(
    ref_type: &str,
    stem: &str,
    source_bytes: &[u8],
) -> (PathBuf, String) {
    let hash12 = hash12_for(source_bytes);
    let dir_rel = store_dir_rel(ref_type, stem, &hash12);
    let pointer = pointer_for(ref_type, stem, &hash12);
    (dir_rel, pointer)
}

/// Resolve store identity for a create target: `(ref_type, stem, pdf_rel)`
/// where `pdf_rel` is the vault-relative library PDF path recorded in
/// `code.json`. `None` for external (`-o`) targets, which write no store.
pub(super) fn store_context_for_target(
    config: &super::model::Config,
    target_plan: &super::stamp::TargetPlan,
) -> Option<(String, String, String)> {
    use super::stamp::TargetWorkflow;
    match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => {
            let meta =
                super::io::pdf_path_metadata(config, library_destination)
                    .ok()?;
            let ref_type = meta.ref_type?;
            let pdf_rel =
                vault_rel_forward(&config.bob_dir, library_destination);
            Some((ref_type, target_plan.stem.clone(), pdf_rel))
        }
        TargetWorkflow::Library => {
            let meta =
                super::io::pdf_path_metadata(config, &target_plan.target)
                    .ok()?;
            let ref_type = meta.ref_type?;
            let pdf_rel =
                vault_rel_forward(&config.bob_dir, &target_plan.target);
            Some((ref_type, target_plan.stem.clone(), pdf_rel))
        }
        TargetWorkflow::External => None,
    }
}

/// Write the store for one create, holding `bob_sync.lock`. Returns the
/// outcome, or `None` for external targets. Store failures are returned
/// as warnings, never as errors: the reference still files.
pub(super) fn write_for_create(
    config: &super::model::Config,
    target_plan: &super::stamp::TargetPlan,
    installed_pdf: &std::path::Path,
    source_bytes: &[u8],
    route: &str,
    origin: String,
    source_note: Option<String>,
    source_note_sha256: Option<String>,
    catalog: &CatalogOutcome,
) -> Option<StoreWriteOutcome> {
    let (ref_type, stem, pdf_rel) =
        store_context_for_target(config, target_plan)?;
    let pdf_sha256 = match super::io::sha256_file(installed_pdf) {
        Ok(sha) => sha,
        Err(error) => {
            eprintln!(
                "warning: could not hash installed PDF {}: {error}; skipping source store",
                installed_pdf.display()
            );
            return None;
        }
    };
    // Hold the vault maintenance lock across both store writes so a
    // concurrent sync can never commit half a store.
    let _guard = match super::ob::acquire_lock_waiting(
        std::time::Duration::from_secs(60),
        || {
            eprintln!("waiting for vault maintenance lock to save source…");
        },
    ) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!(
                "warning: {error}; skipping source store (reference still filed)"
            );
            return None;
        }
    };
    let params = StoreWriteParams {
        ref_type: &ref_type,
        stem: &stem,
        source_bytes,
        route,
        origin,
        source_note,
        source_note_sha256,
        pdf_rel,
        pdf_sha256,
        catalog,
        verified: None,
    };
    match write_store(&config.bob_dir, params) {
        Ok(outcome) => {
            if let Some(warning) = &outcome.warning {
                eprintln!("{warning}");
            }
            Some(outcome)
        }
        Err(error) => {
            eprintln!(
                "warning: could not write source store: {error}; reference still filed"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::code_catalog::CodeCatalog;
    use super::*;
    use std::collections::BTreeMap;

    fn catalog_with_langs(langs: &[&str]) -> CatalogOutcome {
        let blocks = langs
            .iter()
            .enumerate()
            .map(|(i, lang)| CodeBlock {
                n: (i as u32) + 1,
                kind: "code".to_string(),
                lang: Some((*lang).to_string()),
                classes: vec![(*lang).to_string()],
                attrs: BTreeMap::new(),
                text: format!("code {lang}\n"),
                section: None,
                page: Some(7),
                page_end: Some(7),
                page_source: Some("anchor".to_string()),
            })
            .collect();
        CatalogOutcome::Catalog(CodeCatalog {
            version: 1,
            blocks,
            links: None,
        })
    }

    #[test]
    fn layout_pointer_and_hash_are_content_addressed() {
        let bytes = b"# Report\n\n```yaml\nrefresh: 1\n```\n";
        let digest = sha256_hex(bytes);
        assert_eq!(digest.len(), 64);
        let hash12 = hash12_for(bytes);
        assert_eq!(hash12, digest[..12].to_string());
        assert_eq!(
            store_dir_rel("chat", "ace_refresh_loop", &hash12),
            PathBuf::from(format!(
                ".bob/ref-sources/chat/ace_refresh_loop--{hash12}"
            ))
        );
        assert_eq!(
            pointer_for("chat", "ace_refresh_loop", &hash12),
            format!(
                ".bob/ref-sources/chat/ace_refresh_loop--{hash12}/source.md"
            )
        );
    }

    #[test]
    fn code_summary_covers_catalog_shapes() {
        assert_eq!(
            code_summary(&catalog_with_langs(&["yaml", "rust"])),
            "2 blocks (yaml, rust)"
        );
        assert_eq!(
            code_summary(&CatalogOutcome::Catalog(CodeCatalog {
                version: 1,
                blocks: vec![],
                links: None,
            })),
            "none"
        );
        assert_eq!(
            code_summary(&CatalogOutcome::Missing),
            "not recorded (missing report)"
        );
    }

    #[test]
    fn link_summary_ranks_sites_by_frequency() {
        use super::super::code_catalog::{CatalogLink, CodeCatalog};
        let catalog = CatalogOutcome::Catalog(CodeCatalog {
            version: 1,
            blocks: vec![],
            links: Some(vec![
                CatalogLink {
                    n: 1,
                    url: "https://github.com/a".to_string(),
                    text: "a".to_string(),
                    title: None,
                    autolink: false,
                    context: None,
                    section: None,
                    page: Some(1),
                    page_source: Some("anchor".to_string()),
                },
                CatalogLink {
                    n: 2,
                    url: "https://arxiv.org/abs/1".to_string(),
                    text: "b".to_string(),
                    title: None,
                    autolink: false,
                    context: None,
                    section: None,
                    page: Some(1),
                    page_source: Some("anchor".to_string()),
                },
                CatalogLink {
                    n: 3,
                    url: "https://github.com/b".to_string(),
                    text: "c".to_string(),
                    title: None,
                    autolink: false,
                    context: None,
                    section: None,
                    page: Some(2),
                    page_source: Some("anchor".to_string()),
                },
                CatalogLink {
                    n: 4,
                    url: "mailto:me@example.com".to_string(),
                    text: "me@example.com".to_string(),
                    title: None,
                    autolink: true,
                    context: None,
                    section: None,
                    page: Some(2),
                    page_source: Some("anchor".to_string()),
                },
            ]),
        });
        assert_eq!(
            link_summary(&catalog),
            "4 links (github.com, arxiv.org, email)"
        );
        assert_eq!(
            link_summary(&CatalogOutcome::Catalog(CodeCatalog {
                version: 1,
                blocks: vec![],
                links: Some(vec![]),
            })),
            "none"
        );
        assert_eq!(link_site("https://WWW.Example.COM/x"), "example.com");
        assert_eq!(link_site("mailto:me@example.com"), "email");
        assert_eq!(link_site("obsidian://open?vault=x"), "obsidian");
    }

    #[test]
    fn stored_catalog_tolerates_missing_links_array() {
        // Stores written before link recording have no `links` field;
        // `bob ref code` must read them identically to new stores.
        let without: StoredCatalog = serde_json::from_str(
            r#"{"version":1,"captured_at":"2026-01-01T00:00:00Z","route":"markdown","source":{"sha256":"abc","bytes":3,"origin":"x"},"pdf":{"path":"p.pdf","sha256":"def"},"renderer":{"bob":"0","pandoc":"p"},"blocks":[]}"#,
        )
        .expect("parse old store");
        assert_eq!(without.links, None);
        assert_eq!(without.blocks.len(), 0);
        let with_links: StoredCatalog = serde_json::from_str(
            r#"{"version":1,"captured_at":"2026-01-01T00:00:00Z","route":"markdown","source":{"sha256":"abc","bytes":3,"origin":"x"},"pdf":{"path":"p.pdf","sha256":"def"},"renderer":{"bob":"0","pandoc":"p"},"blocks":[],"links":[]}"#,
        )
        .expect("parse new store");
        assert_eq!(with_links.links, Some(vec![]));
        assert_eq!(with_links.blocks, without.blocks);
    }

    fn temp_bob_dir(case: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bob-source-store-{case}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp dir");
        root
    }

    #[test]
    fn write_store_is_write_once_and_birth_finds_it() {
        let bob_dir = temp_bob_dir("write-once");
        let bytes = b"# Report\n\n```rust\nfn main() {}\n```\n";
        let digest = sha256_hex(bytes);
        let hash12 = digest[..12].to_string();
        let catalog = catalog_with_langs(&["rust"]);
        let params = StoreWriteParams {
            ref_type: "chat",
            stem: "report",
            source_bytes: bytes,
            route: "markdown",
            origin: "/tmp/report.md".to_string(),
            source_note: None,
            source_note_sha256: None,
            pdf_rel: "lib/chat/report.pdf".to_string(),
            pdf_sha256: "0123456789abcdef".repeat(4)[..64].to_string(),
            catalog: &catalog,
            verified: None,
        };
        let first = write_store(&bob_dir, params.clone()).expect("first write");
        assert!(!first.reused);
        assert_eq!(
            first.pointer,
            format!(".bob/ref-sources/chat/report--{hash12}/source.md")
        );
        assert_eq!(
            fs::read(bob_dir.join(&first.pointer)).expect("read"),
            bytes
        );
        // Same bytes reuse.
        let again = write_store(&bob_dir, params).expect("reuse");
        assert!(again.reused);
        assert_eq!(again.pointer, first.pointer);
        // Corrupting the stored file then rewriting the original bytes
        // refuses instead of overwriting.
        fs::write(bob_dir.join(&first.pointer), b"# Corrupted\n")
            .expect("corrupt store");
        let refusal_catalog = catalog_with_langs(&["rust"]);
        let refusal = write_store(
            &bob_dir,
            StoreWriteParams {
                ref_type: "chat",
                stem: "report",
                source_bytes: bytes,
                route: "markdown",
                origin: "/tmp/report.md".to_string(),
                source_note: None,
                source_note_sha256: None,
                pdf_rel: "lib/chat/report.pdf".to_string(),
                pdf_sha256: "0123456789abcdef".repeat(4)[..64].to_string(),
                catalog: &refusal_catalog,
                verified: None,
            },
        )
        .expect("refusal is Ok");
        assert!(refusal.warning.is_some());
        assert_eq!(
            fs::read(bob_dir.join(&first.pointer)).expect("still corrupted"),
            b"# Corrupted\n"
        );
        let _ = fs::remove_dir_all(&bob_dir);
    }

    fn inspect_row(
        path: &str,
        pointer: Option<&str>,
    ) -> crate::native::ref_library::RefRow {
        crate::native::ref_library::RefRow {
            path: path.to_string(),
            link: format!("[[{}]]", path.trim_end_matches(".md")),
            title: "Note".to_string(),
            marker_id: None,
            origin: "external".to_string(),
            ref_type: Some("papers".to_string()),
            era: "modern".to_string(),
            status: Some("ready".to_string()),
            status_sync: "ok".to_string(),
            frontmatter_status: None,
            legacy_status: None,
            blocked: false,
            blocked_by: Vec::new(),
            reading_state: "queued".to_string(),
            reading_state_source: "ref_task:[ ]".to_string(),
            parent: None,
            task: None,
            urls: Vec::new(),
            earlier: Vec::new(),
            reading: None,
            identity: crate::native::ref_library::RefIdentity {
                keys: Vec::new(),
                arxiv: None,
                doi: None,
            },
            author: None,
            published: None,
            captured: None,
            added: None,
            added_source: None,
            added_at: None,
            finished: None,
            finished_source: None,
            source_pdf: None,
            audio: None,
            annotation_count: 0,
            comment_count: 0,
            snapshot: None,
            research_ref: None,
            source_markdown: pointer.map(str::to_string),
            superseded_by: None,
            diagnostics: Vec::new(),
            family: path.to_string(),
            has_later: false,
            id: None,
            source_block: None,
            source_path: None,
            source_id: None,
            source_blocks: Vec::new(),
            v2: false,
            task_line: None,
        }
    }

    fn write_catalog(
        path: &Path,
        digest: &str,
        bytes: usize,
        blocks: usize,
        catalog_error: Option<&str>,
    ) {
        let blocks_json: String = (0..blocks)
            .map(|i| {
                format!(
                    r#"{{"n":{},"kind":"code","text":"fn {i}() {{}}","classes":[],"attrs":{{}}}}"#,
                    i + 1
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let error = match catalog_error {
            Some(reason) => format!(r#","catalog_error":"{reason}""#),
            None => String::new(),
        };
        let json = format!(
            r#"{{"version":1,"captured_at":"2026-10-16T00:00:00Z","route":"markdown","source":{{"sha256":"{digest}","bytes":{bytes},"origin":"/tmp/n.md"}},"pdf":{{"path":"lib/papers/n.pdf","sha256":"{}"}},"renderer":{{"bob":"0","pandoc":"test"}}{error},"blocks":[{blocks_json}]}}"#,
            "0".repeat(64)
        );
        fs::write(path, json).expect("code.json");
    }

    #[test]
    fn inspect_reports_dangling_mismatch_missing_orphan_and_history() {
        let bob_dir = temp_bob_dir("inspect");
        let saved = b"# Saved\n";
        let digest = sha256_hex(saved);
        let hash12 = &digest[..12];
        let pointer =
            format!(".bob/ref-sources/papers/saved--{hash12}/source.md");
        let saved_dir =
            bob_dir.join(format!(".bob/ref-sources/papers/saved--{hash12}"));
        fs::create_dir_all(&saved_dir).expect("saved dir");
        fs::write(saved_dir.join("source.md"), saved).expect("source");
        write_catalog(
            &saved_dir.join("code.json"),
            &digest,
            saved.len(),
            2,
            None,
        );

        let missing = b"# no catalog\n";
        let missing_hash12 = &sha256_hex(missing)[..12];
        let missing_ptr = format!(
            ".bob/ref-sources/papers/missing--{missing_hash12}/source.md"
        );
        let missing_dir = bob_dir
            .join(format!(".bob/ref-sources/papers/missing--{missing_hash12}"));
        fs::create_dir_all(&missing_dir).expect("missing dir");
        fs::write(missing_dir.join("source.md"), missing).expect("source");

        let mismatch_ptr =
            ".bob/ref-sources/papers/mismatch--cccccccccccc/source.md";
        let mismatch_dir =
            bob_dir.join(".bob/ref-sources/papers/mismatch--cccccccccccc");
        fs::create_dir_all(&mismatch_dir).expect("mismatch dir");
        fs::write(mismatch_dir.join("source.md"), b"# corrupted\n")
            .expect("source");
        write_catalog(
            &mismatch_dir.join("code.json"),
            &digest,
            saved.len(),
            0,
            None,
        );

        let orphan_dir =
            bob_dir.join(".bob/ref-sources/papers/orphan--dddddddddddd");
        fs::create_dir_all(&orphan_dir).expect("orphan dir");
        fs::write(orphan_dir.join("source.md"), b"# orphan\n").expect("source");

        let history_dir = bob_dir.join("ref_history/papers");
        fs::create_dir_all(&history_dir).expect("history");
        fs::write(
            history_dir.join("saved__2026-10-16.md"),
            format!("---\nsource_markdown: {pointer}\n---\n# Frozen\n"),
        )
        .expect("frozen");

        let rows = vec![
            inspect_row("ref/papers/saved.md", Some(&pointer)),
            inspect_row(
                "ref/papers/dangling.md",
                Some(".bob/ref-sources/papers/gone--aaaaaaaaaaaa/source.md"),
            ),
            inspect_row("ref/papers/missing.md", Some(&missing_ptr)),
            inspect_row("ref/papers/mismatch.md", Some(mismatch_ptr)),
            inspect_row("ref/papers/plain.md", None),
        ];
        let health = inspect_source_stores(&bob_dir, &rows);
        assert_eq!(health.saved, 2, "saved + missing-catalog source.md");
        assert_eq!(health.code_blocks, 2);
        assert_eq!(health.dangling.len(), 1);
        assert_eq!(health.digest_mismatch.len(), 1);
        assert_eq!(health.missing_catalog.len(), 1);
        assert_eq!(
            health.orphans.len(),
            1,
            "frozen note keeps the saved pointer from being an orphan: {:?}",
            health.orphans
        );
        assert!(health.orphans.iter().any(|issue| match issue {
            SourceStoreIssue::Orphan { store } => {
                store.ends_with("orphan--dddddddddddd")
            }
            _ => false,
        }));
        let _ = fs::remove_dir_all(&bob_dir);
    }
}

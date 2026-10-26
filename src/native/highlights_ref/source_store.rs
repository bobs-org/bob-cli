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
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::code_catalog::{CatalogOutcome, CodeBlock, CodeSection};
use super::model::{CommandError, Result};

pub(crate) const STORE_ROOT: &str = ".bob/ref-sources";
pub(crate) const STORE_VERSION: u32 = 1;
#[allow(dead_code)]
pub(crate) const _FIELD_SOURCE_MARKDOWN: &str = "source_markdown";

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
pub(crate) struct StoredCatalog {
    pub(crate) version: u32,
    pub(crate) captured_at: String,
    pub(crate) route: String,
    pub(crate) source: StoredSource,
    pub(crate) pdf: StoredPdf,
    pub(crate) renderer: StoredRenderer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) catalog_error: Option<String>,
    #[serde(default)]
    pub(crate) blocks: Vec<StoredBlock>,
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

#[allow(dead_code)]
pub(crate) fn pdf_vault_rel(bob_dir: &Path, pdf_abs: &Path) -> String {
    vault_rel_forward(bob_dir, pdf_abs)
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
        let catalog = build_stored_catalog(params, &digest)?;
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
    let catalog = build_stored_catalog(params, &digest)?;
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
    params: StoreWriteParams<'_>,
    digest: &str,
) -> Result<StoredCatalog> {
    let (blocks, catalog_error) = match params.catalog {
        CatalogOutcome::Catalog(catalog) => {
            (catalog.blocks.iter().map(StoredBlock::from).collect(), None)
        }
        CatalogOutcome::Missing => {
            (Vec::new(), Some("code catalog report missing".to_string()))
        }
        CatalogOutcome::Invalid(reason) => (Vec::new(), Some(reason.clone())),
    };
    Ok(StoredCatalog {
        version: STORE_VERSION,
        captured_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        route: params.route.to_string(),
        source: StoredSource {
            sha256: digest.to_string(),
            bytes: params.source_bytes.len(),
            origin: params.origin,
            source_note: params.source_note.clone(),
            source_note_sha256: params.source_note_sha256.clone(),
        },
        pdf: StoredPdf {
            path: params.pdf_rel,
            sha256: params.pdf_sha256,
        },
        renderer: StoredRenderer {
            bob: env!("CARGO_PKG_VERSION").to_string(),
            pandoc: pandoc_version_string(),
        },
        catalog_error,
        blocks,
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
/// (warn-only) and by future attach/overwrite flows.
#[allow(dead_code)]
pub(crate) fn code_catalog_ignored(bob_dir: &Path, dir_rel: &Path) -> bool {
    let child_env = super::ob::child_env();
    is_ignored(bob_dir, &child_env, &dir_rel.join("code.json"))
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
#[allow(dead_code)]
pub(crate) enum StoreStatus {
    Available,
    NoCode,
    NoCatalog,
    NoSource,
    SourceMissing,
    SourceInvalid,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct StoreRead {
    pub(crate) status: StoreStatus,
    pub(crate) detail: Option<String>,
    pub(crate) bytes: Option<Vec<u8>>,
    pub(crate) catalog: Option<StoredCatalog>,
}

#[allow(dead_code)]
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
        Err(_) => {
            return StoreRead {
                status: StoreStatus::SourceMissing,
                detail: Some(format!(
                    "saved Markdown {trimmed} is absent on this host"
                )),
                bytes: None,
                catalog: None,
            };
        }
    };
    let digest = sha256_hex(&bytes);
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
        Err(_) => {
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
        CatalogOutcome::Catalog(CodeCatalog { version: 1, blocks })
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
            })),
            "none"
        );
        assert_eq!(
            code_summary(&CatalogOutcome::Missing),
            "not recorded (missing report)"
        );
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
}

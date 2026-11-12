//! Notebook creation: the shared renderer and the reusable planner.
//!
//! [`render_new_notebook`] writes the contract file. [`plan_create`] is pure
//! over a [`VaultSnapshot`]: it plans the new notebook in an in-memory
//! overlay, derives every affected host edit from that same post-create
//! view, and returns the new file, the host edits, and the read dependencies
//! as data. Standalone `create` stages them as one guarded transaction; a
//! later capture batch can feed a snapshot that already holds its staged
//! files and compose the same edits into its own transaction, with no second
//! commit and no nested maintenance lock.

use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use super::{
    catalog::FileEdit,
    model::{CatalogRecord, CatalogState},
    scan::{analyze, link_destination},
    snapshot::{LiteKind, VaultSnapshot},
    txn::ReadDependencies,
};
use crate::native::{
    capture_block_ids::is_valid_block_id,
    capture_project_note::{format_created_timestamp, project_note_basename},
    highlights_ref::humanize_stem,
    records::yaml_string_scalar,
};

/// Summaries over this length warn; they never refuse.
pub(crate) const SUMMARY_WARN_CHARS: usize = 140;

/// What `render_new_notebook` needs. `body` is an already-prepared body
/// (later seeded capture); it follows the H1 after one blank line.
#[derive(Debug, Clone)]
pub(crate) struct NewNotebookInput<'a> {
    /// Wikilink destination of the parent (stem or extensionless path).
    pub(crate) parent_link: &'a str,
    pub(crate) summary: &'a str,
    pub(crate) title: &'a str,
    /// The frontmatter `created` stamp.
    pub(crate) created: &'a str,
    pub(crate) body: Option<&'a str>,
}

/// The contract file: keys `parent`, `template`, `type`, `summary`,
/// `created`, a blank line, then the H1.
pub(crate) fn render_new_notebook(input: &NewNotebookInput<'_>) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!(
        "parent: {}\n",
        yaml_string_scalar(&format!("[[{}]]", input.parent_link))
    ));
    out.push_str("template: \"[[new_notebook]]\"\n");
    out.push_str("type: \"[[notebook]]\"\n");
    out.push_str(&format!("summary: {}\n", yaml_string_scalar(input.summary)));
    out.push_str(&format!("created: {}\n", input.created));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n", input.title));
    if let Some(body) = input.body.filter(|body| !body.trim().is_empty()) {
        out.push('\n');
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[derive(Debug, Clone)]
pub(crate) struct CreateRequest {
    pub(crate) parent: String,
    pub(crate) id: String,
    pub(crate) summary: String,
    pub(crate) title: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) now: NaiveDateTime,
    /// Bob's configured zone for the `created` stamp offset. `now` is
    /// already wall-clock time in this zone; `None` resolves via the host.
    pub(crate) timezone: Option<Tz>,
}

/// A refusal with a stable code and an optional fix hint. Exit code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateRefusal {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

impl CreateRefusal {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint,
        }
    }
}

/// Validated, vault-independent inputs.
#[derive(Debug, Clone)]
pub(crate) struct ValidInputs {
    pub(crate) summary: String,
    pub(crate) title: Option<String>,
    pub(crate) warnings: Vec<String>,
}

/// Validate what needs no vault: ID alphabet, summary, and title.
pub(crate) fn validate_inputs(
    id: &str,
    summary: &str,
    title: Option<&str>,
) -> Result<ValidInputs, CreateRefusal> {
    if !is_valid_block_id('^', id) {
        return Err(CreateRefusal::new(
            "notebook_id_invalid",
            format!(
                "`{id}` is not a valid notebook ID; use only A-Z, a-z, 0-9 or '-'"
            ),
            None,
        ));
    }
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(CreateRefusal::new(
            "notebook_summary_required",
            "a notebook needs a one-line summary: pass -s SUMMARY",
            None,
        ));
    }
    if summary.contains(['\n', '\r']) {
        return Err(CreateRefusal::new(
            "notebook_summary_multiline",
            "the summary must be one line",
            None,
        ));
    }
    let title = match title {
        None => None,
        Some(title) => {
            let title = title.trim();
            if title.is_empty() || title.contains(['\n', '\r']) {
                return Err(CreateRefusal::new(
                    "notebook_title_invalid",
                    "the title must be one nonblank line",
                    None,
                ));
            }
            Some(title.to_string())
        }
    };
    let mut warnings = Vec::new();
    let length = summary.chars().count();
    if length > SUMMARY_WARN_CHARS {
        warnings.push(format!(
            "notebook_summary_long: the summary is {length} characters; keep it to at most {SUMMARY_WARN_CHARS}"
        ));
    }
    Ok(ValidInputs {
        summary: summary.to_string(),
        title,
        warnings,
    })
}

/// A planned creation, as data.
#[derive(Debug, Clone)]
pub(crate) struct CreatePlan {
    /// Vault-relative path of the new notebook.
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) parent_stem: String,
    /// The new file's complete contents.
    pub(crate) contents: String,
    pub(crate) warnings: Vec<String>,
    /// Affected hosts' catalog records (the states found before writing).
    pub(crate) catalogs: Vec<CatalogRecord>,
    /// One before/after edit per affected host that must change.
    pub(crate) edits: Vec<FileEdit>,
    /// Read-only inputs to guard: the parent and the owner/root ancestry
    /// that are not themselves rewritten.
    pub(crate) deps: ReadDependencies,
}

/// Resolve and plan a creation over `snapshot`.
pub(crate) fn plan_create(
    snapshot: &VaultSnapshot,
    request: &CreateRequest,
) -> Result<CreatePlan, CreateRefusal> {
    let inputs = validate_inputs(
        &request.id,
        &request.summary,
        request.title.as_deref(),
    )?;
    let base = analyze(snapshot);
    let index = snapshot.index();

    // Parent: an area, a project of any status, or a valid notebook chain.
    let wanted = normalize_parent(&request.parent);
    let parent_path = match index.resolve_detailed(None, &wanted) {
        crate::native::vault_links::LinkResolution::Found(path) => path,
        crate::native::vault_links::LinkResolution::Ambiguous => {
            return Err(CreateRefusal::new(
                "notebook_parent_ambiguous",
                format!("parent `{wanted}` matches more than one note"),
                Some(
                    "pass a path-qualified parent such as `areas/dev`"
                        .to_string(),
                ),
            ));
        }
        crate::native::vault_links::LinkResolution::Missing => {
            return Err(CreateRefusal::new(
                "notebook_parent_missing",
                format!("parent `{wanted}` is not a note in the vault"),
                Some(
                    "name an area, a project, or an existing notebook"
                        .to_string(),
                ),
            ));
        }
    };
    let parent_kind = snapshot
        .files
        .get(&parent_path)
        .map(|file| file.lite.kind)
        .ok_or_else(|| {
            CreateRefusal::new(
                "notebook_parent_missing",
                format!("parent `{wanted}` could not be read"),
                None,
            )
        })?;
    match parent_kind {
        LiteKind::Area | LiteKind::Project { .. } => {}
        LiteKind::Notebook => {
            if !base.graph.notebooks.contains_key(&parent_path) {
                return Err(CreateRefusal::new(
                    "notebook_parent_invalid",
                    format!(
                        "notebook `{}` has no valid parent chain to an area or project (missing, ambiguous, or cyclic)",
                        parent_path.display()
                    ),
                    Some("run `bob notebooks check` and repair it first".to_string()),
                ));
            }
        }
        _ => {
            return Err(CreateRefusal::new(
                "notebook_parent_unsupported",
                format!(
                    "{} is not an area, a project, or a notebook",
                    parent_path.display()
                ),
                Some("notebooks hang under an area, a project, or another notebook".to_string()),
            ));
        }
    }

    // New root file, named through the project-note rule.
    let parent_stem = parent_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    let basename = project_note_basename(&parent_stem, &request.id);
    let path = PathBuf::from(&basename);
    if snapshot.contains_ignore_case(&path) {
        return Err(CreateRefusal::new(
            "notebook_exists",
            format!("{basename} already exists (names compare ignoring case)"),
            Some(
                "choose another ID, or write into the existing note"
                    .to_string(),
            ),
        ));
    }

    let title = inputs
        .title
        .clone()
        .unwrap_or_else(|| default_title(&request.id));
    let created = format_created_timestamp(request.now, request.timezone);
    let parent_link = link_destination(&index, &parent_path);
    let contents = render_new_notebook(&NewNotebookInput {
        parent_link: &parent_link,
        summary: &inputs.summary,
        title: &title,
        created: &created,
        body: request.body.as_deref(),
    });

    // One post-create view drives every affected host edit.
    let after = snapshot.with_overlay([(path.clone(), contents.clone())]);
    let analysis = analyze(&after);
    let Some(created_node) = analysis.graph.notebooks.get(&path) else {
        return Err(CreateRefusal::new(
            "notebook_parent_invalid",
            format!(
                "{basename} would not be placeable under {}",
                parent_path.display()
            ),
            Some(
                "run `bob notebooks check` and repair the parent chain"
                    .to_string(),
            ),
        ));
    };
    let mut catalogs = Vec::new();
    let mut edits = Vec::new();
    for host in &created_node.hosts {
        let Some(plan) = analysis.plan.host(host) else {
            continue;
        };
        if plan.record.state == CatalogState::Damaged {
            let line = plan
                .record
                .issues
                .first()
                .and_then(|issue| issue.line)
                .map(|line| format!(":{line}"))
                .unwrap_or_default();
            return Err(CreateRefusal::new(
                "notebook_catalog_damaged",
                format!(
                    "the catalog in {}{line} is damaged, so nothing was created",
                    plan.record.path
                ),
                Some("repair its notebook catalog markers by hand, then retry".to_string()),
            ));
        }
        catalogs.push(plan.record.clone());
        if let Some(edit) = &plan.edit {
            edits.push(edit.clone());
        }
    }

    // Guard the parent and ancestry, rewritten or not. Rewritten hosts are
    // guarded by their staged preimages instead.
    let rewritten: Vec<&Path> =
        edits.iter().map(|edit| edit.path.as_path()).collect();
    let ancestry = created_node
        .chain
        .iter()
        .chain(std::iter::once(&parent_path))
        .chain(created_node.hosts.iter())
        .filter(|path| !rewritten.contains(&path.as_path()))
        .map(PathBuf::as_path);
    let deps = ReadDependencies::from_paths(snapshot, ancestry);

    Ok(CreatePlan {
        stem: Path::new(&basename)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string(),
        path,
        title,
        summary: inputs.summary,
        parent_stem,
        contents,
        warnings: inputs.warnings,
        catalogs,
        edits,
        deps,
    })
}

/// `PARENT` as typed: a stem, `stem.md`, a path, or a pasted `[[wikilink]]`.
pub(crate) fn normalize_parent(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or(trimmed);
    let target = inner.split(['|', '#']).next().unwrap_or(inner).trim();
    target
        .strip_suffix(".md")
        .or_else(|| target.strip_suffix(".MD"))
        .unwrap_or(target)
        .to_string()
}

/// The humanized ID with its first letter capitalized, falling back to
/// the ID itself when nothing remains.
fn default_title(id: &str) -> String {
    let title = humanize_stem(id);
    let mut chars = title.chars();
    match chars.next() {
        None => id.to_string(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(summary: &'a str) -> NewNotebookInput<'a> {
        NewNotebookInput {
            parent_link: "dev",
            summary,
            title: "Nvim",
            created: "2026-11-09T09:14:00-0500",
            body: None,
        }
    }

    #[test]
    fn renders_contract_key_order_and_h1() {
        let text = render_new_notebook(&input("Neovim config notes."));
        assert_eq!(
            text,
            "---\nparent: \"[[dev]]\"\ntemplate: \"[[new_notebook]]\"\ntype: \"[[notebook]]\"\nsummary: Neovim config notes.\ncreated: 2026-11-09T09:14:00-0500\n---\n\n# Nvim\n"
        );
    }

    #[test]
    fn summaries_that_look_typed_or_structural_are_quoted() {
        for (summary, expected) in [
            ("yes", "\"yes\""),
            ("true", "\"true\""),
            ("42", "\"42\""),
            ("null", "\"null\""),
            ("a: b", "\"a: b\""),
            ("# heading", "\"# heading\""),
            ("- item", "\"- item\""),
            ("[a, b]", "\"[a, b]\""),
            ("says \"hi\"", "says \"hi\""),
            ("\"quoted\" start", "\"\\\"quoted\\\" start\""),
            ("2026-11-09", "\"2026-11-09\""),
            ("Yes", "\"Yes\""),
            ("plain words, with a comma.", "plain words, with a comma."),
        ] {
            let text = render_new_notebook(&input(summary));
            assert!(
                text.contains(&format!("\nsummary: {expected}\n")),
                "{summary:?} -> {text}"
            );
            let front: serde_yaml::Value = serde_yaml::from_str(
                text.split("---\n").nth(1).expect("frontmatter"),
            )
            .expect("valid yaml");
            assert_eq!(front["summary"].as_str(), Some(summary));
        }
    }

    #[test]
    fn prepared_body_follows_the_h1() {
        let mut with_body = input("S");
        with_body.body = Some("- First note\n");
        assert!(render_new_notebook(&with_body)
            .ends_with("# Nvim\n\n- First note\n"));
    }

    #[test]
    fn input_validation_refuses_and_warns() {
        assert_eq!(
            validate_inputs("bad id", "S", None).unwrap_err().code,
            "notebook_id_invalid"
        );
        assert_eq!(
            validate_inputs("ok", "  ", None).unwrap_err().code,
            "notebook_summary_required"
        );
        assert_eq!(
            validate_inputs("ok", "a\nb", None).unwrap_err().code,
            "notebook_summary_multiline"
        );
        assert_eq!(
            validate_inputs("ok", "S", Some(" ")).unwrap_err().code,
            "notebook_title_invalid"
        );
        let long = "x".repeat(141);
        let valid = validate_inputs("ok", &long, None).unwrap();
        assert_eq!(valid.warnings.len(), 1);
        assert!(valid.warnings[0].starts_with("notebook_summary_long"));
    }

    #[test]
    fn default_title_humanizes_the_id() {
        assert_eq!(default_title("nvim-config"), "Nvim config");
        assert_eq!(default_title("nvim"), "Nvim");
        assert_eq!(default_title("---"), "---");
    }

    #[test]
    fn parent_arguments_normalize() {
        for raw in ["dev", "dev.md", "[[dev]]", "[[dev|Dev]]", " dev "] {
            assert_eq!(normalize_parent(raw), "dev", "{raw}");
        }
    }
}

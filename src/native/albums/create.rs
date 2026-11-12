//! Album creation: the shared renderer and the reusable planner.
//!
//! [`render_new_album`] writes the contract file. [`plan_create`] validates
//! inputs, renders the album note and its starter Base, plans the empty
//! records index from the existing renderer, and derives every affected
//! owner catalog edit from the same post-create overlay. Standalone
//! `create` stages them as one guarded transaction; dry-run and real
//! execution share this planner and its bytes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use super::{
    catalog::{self, AlbumCatalogRecord},
    grouping::group_records,
    index::{plan_index, RecordNouns},
    registry::AlbumRegistry,
    report::{resolved_plural, resolved_singular},
    AlbumDeclaration,
};
use crate::native::{
    capture_project_note::format_created_timestamp,
    highlights_ref::humanize_stem,
    host_catalog::{txn::is_album_namespace_name, FileEdit, ReadDependencies},
    notebooks::{
        create::normalize_parent,
        scan::link_destination,
        snapshot::{LiteKind, VaultSnapshot},
    },
    records::{
        init::base_markdown,
        schema::{
            album_label, album_noun_reserved, valid_identifier, FieldKind,
            RecordDate, RecordField, RecordScalar, RecordType, RecordValue,
        },
        yaml_string_scalar,
    },
    vault_links::LinkResolution,
};

/// Summaries over this length warn; they never refuse.
pub(crate) const SUMMARY_WARN_CHARS: usize = 140;

/// What `render_new_album` needs. The parent link is already
/// path-qualified when the stem alone would be ambiguous.
#[derive(Debug, Clone)]
pub(crate) struct NewAlbumInput<'a> {
    pub(crate) parent_link: &'a str,
    pub(crate) summary: &'a str,
    pub(crate) emoji: Option<&'a str>,
    pub(crate) title: &'a str,
    pub(crate) created: &'a str,
    pub(crate) noun: &'a str,
    pub(crate) label: &'a str,
}

/// The contract file: canonical key order `parent`, `type`, `summary`,
/// optional `emoji`, `created`, then `record` with `noun` and `label`
/// (`record.fields` stays omitted until real values settle); an H1; one
/// intro line teaching capture and linking the starter Base.
pub(crate) fn render_new_album(input: &NewAlbumInput<'_>) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!(
        "parent: {}\n",
        yaml_string_scalar(&format!("[[{}]]", input.parent_link))
    ));
    out.push_str(&format!("type: {}\n", yaml_string_scalar("[[album]]")));
    out.push_str(&format!("summary: {}\n", yaml_string_scalar(input.summary)));
    if let Some(emoji) = input.emoji.filter(|emoji| !emoji.is_empty()) {
        out.push_str(&format!("emoji: {}\n", yaml_string_scalar(emoji)));
    }
    out.push_str(&format!("created: {}\n", input.created));
    out.push_str(&render_album_record_schema(
        input.noun,
        input.label,
        None,
        false,
        &[],
        &[],
    ));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n", input.title));
    out
}

/// Render the semantic `record:` mapping in Obsidian's block-YAML style.
/// In particular, sequence items stay indented under their key so a native
/// Properties edit to a neighboring scalar preserves this block's bytes.
pub(crate) fn render_album_record_schema(
    noun: &str,
    label: &str,
    plural: Option<&str>,
    dated_notes: bool,
    fields: &[RecordField],
    legacy: &[String],
) -> String {
    let mut out = String::from("record:\n");
    out.push_str(&format!("  noun: {}\n", yaml_string_scalar(noun)));
    out.push_str(&format!("  label: {}\n", yaml_string_scalar(label)));
    if let Some(plural) = plural {
        out.push_str(&format!("  plural: {}\n", yaml_string_scalar(plural)));
    }
    if dated_notes {
        out.push_str("  dated_notes: true\n");
    }
    if !fields.is_empty() {
        out.push_str("  fields:\n");
        for field in fields {
            out.push_str(&format!("    {}:\n", field.key));
            out.push_str(&format!(
                "      label: {}\n",
                yaml_string_scalar(&field.label)
            ));
            if field.required {
                out.push_str("      required: true\n");
            }
            if field.list {
                out.push_str("      list: true\n");
            }
            if field.suggest {
                out.push_str("      suggest: true\n");
            }
            if let Some(default) = &field.default {
                render_schema_default(&mut out, default);
            }
            if let Some(min) = field.min {
                out.push_str(&format!("      min: {min}\n"));
            }
            if let Some(max) = field.max {
                out.push_str(&format!("      max: {max}\n"));
            }
            match &field.kind {
                FieldKind::Text => {}
                FieldKind::Choices(choices) => {
                    out.push_str("      values:\n");
                    for choice in choices {
                        out.push_str(&format!(
                            "        {}: {}\n",
                            yaml_string_scalar(&choice.value),
                            yaml_string_scalar(&choice.label)
                        ));
                    }
                }
                kind => out
                    .push_str(&format!("      values: {}\n", kind.kind_name())),
            }
        }
    }
    if !legacy.is_empty() {
        out.push_str("  legacy:\n");
        for pattern in legacy {
            out.push_str(&format!("    - {}\n", yaml_string_scalar(pattern)));
        }
    }
    out
}

fn render_schema_default(out: &mut String, default: &RecordValue) {
    match default {
        RecordValue::Scalar(value) => out.push_str(&format!(
            "      default: {}\n",
            render_schema_scalar(value)
        )),
        RecordValue::List(values) => {
            out.push_str("      default:\n");
            for value in values {
                out.push_str(&format!(
                    "        - {}\n",
                    render_schema_scalar(value)
                ));
            }
        }
    }
}

fn render_schema_scalar(value: &RecordScalar) -> String {
    match value {
        RecordScalar::Text(value)
        | RecordScalar::Choice(value)
        | RecordScalar::Link(value) => yaml_string_scalar(value),
        RecordScalar::Date(RecordDate::Resolved(value)) => {
            yaml_string_scalar(&value.format("%Y-%m-%d").to_string())
        }
        RecordScalar::Date(RecordDate::Today) => yaml_string_scalar("today"),
        RecordScalar::Date(RecordDate::Yesterday) => {
            yaml_string_scalar("yesterday")
        }
        RecordScalar::Url(value) => yaml_string_scalar(value.as_str()),
        RecordScalar::Number(value) => value.to_string(),
    }
}

/// The intro line under the H1: teaches the capture route and links the
/// starter Base. The empty index below repeats the route on its own.
pub(crate) fn render_intro(plural: &str, stem: &str) -> String {
    format!(
        "Track {plural} with `Name @{stem}/`; browse every record in [[{stem}.base]].\n"
    )
}

/// Humanized stem with its first letter capitalized, falling back to the
/// stem itself when nothing remains.
fn default_title(stem: &str) -> String {
    let title = humanize_stem(stem);
    let mut chars = title.chars();
    match chars.next() {
        None => stem.to_string(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CreateRequest {
    pub(crate) parent: String,
    pub(crate) stem: String,
    pub(crate) noun: String,
    pub(crate) summary: String,
    pub(crate) title: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) emoji: Option<String>,
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
    pub(crate) stem: String,
    pub(crate) noun: String,
    pub(crate) summary: String,
    pub(crate) title: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) emoji: Option<String>,
    pub(crate) warnings: Vec<String>,
}

/// Validate what needs no vault: stem, noun, summary, title, label, emoji.
pub(crate) fn validate_inputs(
    stem: &str,
    noun: &str,
    summary: &str,
    title: Option<&str>,
    label: Option<&str>,
    emoji: Option<&str>,
) -> Result<ValidInputs, CreateRefusal> {
    let stem = stem.trim();
    let Some(stem) = super::confined_album_route(stem) else {
        return Err(CreateRefusal::new(
            "album_stem_invalid",
            format!(
                "stem {stem:?} must be a lower-case vault-root capture route and cannot be a reserved folder"
            ),
            Some(
                "use only A-Z, a-z, 0-9, '_' or '-', never `done` or `ref`"
                    .to_string(),
            ),
        ));
    };
    let noun = noun.trim();
    if !valid_identifier(noun) {
        return Err(CreateRefusal::new(
            "album_noun_invalid",
            if noun.is_empty() {
                "a record noun is required: pass -n NOUN matching [a-z][a-z0-9_]*"
                    .to_string()
            } else {
                format!("noun {noun:?} must match [a-z][a-z0-9_]*")
            },
            None,
        ));
    }
    if album_noun_reserved(noun) {
        return Err(CreateRefusal::new(
            "album_noun_reserved",
            format!("noun {noun:?} is reserved for a built-in note type"),
            Some(
                "pick a collection-specific noun such as `eatery`".to_string(),
            ),
        ));
    }
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(CreateRefusal::new(
            "album_summary_required",
            "an album needs a one-line summary: pass -s SUMMARY",
            None,
        ));
    }
    if summary.contains(['\n', '\r']) {
        return Err(CreateRefusal::new(
            "album_summary_multiline",
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
                    "album_title_invalid",
                    "the title must be one nonblank line",
                    None,
                ));
            }
            Some(title.to_string())
        }
    };
    // A blank label or emoji is the same as omitting it: the label falls
    // back to the noun-derived rule and the emoji stays absent.
    let label = match label {
        None => None,
        Some(label) => {
            let label = label.trim();
            if label.contains(['\n', '\r']) {
                return Err(CreateRefusal::new(
                    "album_label_invalid",
                    "the label must be one line",
                    None,
                ));
            }
            (!label.is_empty()).then(|| label.to_string())
        }
    };
    let emoji = match emoji {
        None => None,
        Some(emoji) => {
            let emoji = emoji.trim();
            if emoji.contains(['\n', '\r']) {
                return Err(CreateRefusal::new(
                    "album_emoji_invalid",
                    "the emoji must be one line",
                    None,
                ));
            }
            (!emoji.is_empty()).then(|| emoji.to_string())
        }
    };
    let mut warnings = Vec::new();
    let length = summary.chars().count();
    if length > SUMMARY_WARN_CHARS {
        warnings.push(format!(
            "album_summary_long: the summary is {length} characters; keep it to at most {SUMMARY_WARN_CHARS}"
        ));
    }
    Ok(ValidInputs {
        stem,
        noun: noun.to_string(),
        summary: summary.to_string(),
        title,
        label,
        emoji,
        warnings,
    })
}

/// A planned creation, as data.
#[derive(Debug, Clone)]
pub(crate) struct CreatePlan {
    /// Vault-relative path of the new album (`<stem>.md`).
    pub(crate) path: PathBuf,
    /// Vault-relative path of the starter Base (`<stem>.base`).
    pub(crate) base_path: PathBuf,
    pub(crate) stem: String,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) parent_stem: String,
    pub(crate) noun: String,
    pub(crate) label: String,
    /// The new album file's complete contents.
    pub(crate) contents: String,
    /// The starter Base's complete contents.
    pub(crate) base_contents: String,
    pub(crate) warnings: Vec<String>,
    /// Affected hosts' catalog records (the states found before writing).
    pub(crate) catalogs: Vec<AlbumCatalogRecord>,
    /// One before/after edit per affected host that must change.
    pub(crate) edits: Vec<FileEdit>,
    /// Read-only inputs to guard: absent destinations, the empty record
    /// folder, and the owner/ancestor/claim preimages catalog rows use.
    pub(crate) deps: ReadDependencies,
}

/// Vault-root names as `ReadDependencies::insert_directory` will re-list them:
/// `DirEntry` names, no symlink follow, every kind of entry.
fn vault_root_entry_names(vault: &Path) -> Result<Vec<String>, CreateRefusal> {
    let mut names = Vec::new();
    let entries = fs::read_dir(vault).map_err(|error| {
        CreateRefusal::new(
            "album_namespace_unreadable",
            format!("could not inspect the vault root: {error}"),
            None,
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CreateRefusal::new(
                "album_namespace_unreadable",
                format!("could not read a vault-root entry: {error}"),
                None,
            )
        })?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn colliding_root_name(names: &[String], stem: &str) -> Option<String> {
    let md = format!("{stem}.md");
    let base = format!("{stem}.base");
    names
        .iter()
        .find(|name| {
            name.eq_ignore_ascii_case(&md) || name.eq_ignore_ascii_case(&base)
        })
        .cloned()
}

/// Resolve and plan a creation over `snapshot`. The base `registry` feeds
/// claim-collision checks; catalog edits come from the post-create overlay
/// so only the new album's owner chain is ever staged.
pub(crate) fn plan_create(
    vault: &Path,
    snapshot: &VaultSnapshot,
    request: &CreateRequest,
) -> Result<CreatePlan, CreateRefusal> {
    let inputs = validate_inputs(
        &request.stem,
        &request.noun,
        &request.summary,
        request.title.as_deref(),
        request.label.as_deref(),
        request.emoji.as_deref(),
    )?;
    let stem = inputs.stem;
    let md_path = PathBuf::from(format!("{stem}.md"));
    let base_path = PathBuf::from(format!("{stem}.base"));

    // Either destination already claims the name, whatever lives there:
    // a note, a directory, or a symlink (names compare ignoring case).
    // The Markdown snapshot cannot see `.base` files or non-file entries.
    let root_entries = vault_root_entry_names(vault)?;
    if let Some(existing) = colliding_root_name(&root_entries, &stem) {
        return Err(CreateRefusal::new(
            "album_exists",
            format!("{existing} already exists (names compare ignoring case)"),
            Some(
                "choose another stem, or write into the existing note"
                    .to_string(),
            ),
        ));
    }
    // Never claim a populated record folder silently; the first record
    // creates the folder, so anything already there predates the album.
    match fs::symlink_metadata(vault.join(&stem)) {
        Ok(_) => {
            return Err(CreateRefusal::new(
                "album_folder_exists",
                format!(
                    "record folder {stem}/ already exists; the first record creates it"
                ),
                Some(
                    "remove the folder or choose another stem".to_string(),
                ),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CreateRefusal::new(
                "album_folder_unreadable",
                format!("could not inspect record folder {stem}/: {error}"),
                None,
            ));
        }
    }

    // Any existing album claim participates, including broken ones; a
    // matching config kind never blocks creation (it is superseded).
    let base_registry = AlbumRegistry::from_snapshot(vault, snapshot);
    for key in [&stem, &inputs.noun] {
        let conflicts = base_registry.claimants(key);
        if !conflicts.is_empty() {
            return Err(CreateRefusal::new(
                "album_noun_duplicate",
                format!(
                    "album route or noun claim {key:?} conflicts with {}",
                    conflicts.join(", ")
                ),
                Some(
                    "choose another stem or noun, or edit the conflicting album to release the claim"
                        .to_string(),
                ),
            ));
        }
    }

    // PARENT names an area or project of any status, path-aware.
    let wanted = normalize_parent(&request.parent);
    let index = snapshot.index();
    let parent_path = match index.resolve_detailed(None, &wanted) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous => {
            return Err(CreateRefusal::new(
                "album_parent_ambiguous",
                format!("parent `{wanted}` matches more than one note"),
                Some(
                    "pass a path-qualified parent such as `areas/dev`"
                        .to_string(),
                ),
            ));
        }
        LinkResolution::Missing => {
            return Err(CreateRefusal::new(
                "album_parent_missing",
                format!("parent `{wanted}` is not a note in the vault"),
                Some("name an area or a project".to_string()),
            ));
        }
    };
    let parent_kind = snapshot
        .files
        .get(&parent_path)
        .map(|file| file.lite.kind)
        .ok_or_else(|| {
            CreateRefusal::new(
                "album_parent_missing",
                format!("parent `{wanted}` could not be read"),
                None,
            )
        })?;
    match parent_kind {
        LiteKind::Area | LiteKind::Project { .. } => {}
        _ => {
            return Err(CreateRefusal::new(
                "album_parent_unsupported",
                format!(
                    "{} is not an area or a project",
                    parent_path.display()
                ),
                Some(
                    "albums hang directly under an area or a project"
                        .to_string(),
                ),
            ));
        }
    }

    let title = inputs.title.clone().unwrap_or_else(|| default_title(&stem));
    let created = format_created_timestamp(request.now, request.timezone);
    let parent_link = link_destination(&index, &parent_path);
    let label = inputs
        .label
        .clone()
        .unwrap_or_else(|| album_label(&inputs.noun));
    let head = render_new_album(&NewAlbumInput {
        parent_link: &parent_link,
        summary: &inputs.summary,
        emoji: inputs.emoji.as_deref(),
        title: &title,
        created: &created,
        noun: &inputs.noun,
        label: &label,
    });

    // The declaration proves the rendered bytes before anything else reads
    // them; the plural follows the registry's display rule.
    let declaration: AlbumDeclaration =
        super::parse_album_declaration(&md_path, &head).ok_or_else(|| {
            CreateRefusal::new(
                "album_invalid",
                "the planned album note did not parse as an album",
                None,
            )
        })?;
    let plural = resolved_plural(&declaration);
    let singular = resolved_singular(&declaration);
    let nouns = RecordNouns {
        singular: &singular,
        plural: &plural,
    };
    let grouping = group_records(&[], None, &[], &[]);
    let intro = render_intro(&plural, &stem);
    let before = format!("{head}\n{intro}");
    let index_plan = plan_index(
        &md_path.to_string_lossy().replace('\\', "/"),
        Some(&before),
        &stem,
        &nouns,
        &[],
        &grouping,
        None,
        0,
        true,
    );
    let contents = match index_plan.edit {
        Some(edit) => edit.after,
        None if index_plan.state == super::index::IndexState::Current => before,
        None => {
            return Err(CreateRefusal::new(
                "album_invalid",
                "the planned album index could not be rendered",
                None,
            ));
        }
    };

    let base_contents = base_markdown(&RecordType {
        id: "record".to_owned(),
        dir: stem.clone(),
        label: label.clone(),
        emoji: inputs.emoji.clone(),
        parent: None,
        dated_notes: false,
        fields: Vec::new(),
        legacy: Vec::new(),
    });

    // One post-create view drives every affected host edit and validates
    // the complete ownership chain, including cycles.
    let overlay = snapshot.with_overlay([
        (md_path.clone(), contents.clone()),
        (base_path.clone(), base_contents.clone()),
    ]);
    let overlay_registry = AlbumRegistry::from_snapshot(vault, &overlay);
    let entry = overlay_registry
        .albums
        .iter()
        .find(|entry| Path::new(&entry.declaration.path) == md_path);
    let Some(entry) = entry else {
        return Err(CreateRefusal::new(
            "album_invalid",
            "the planned album is missing from its own overlay",
            None,
        ));
    };
    if entry
        .findings
        .iter()
        .any(|finding| finding.code == "album_cycle")
        || entry.parent_path.is_none()
    {
        return Err(CreateRefusal::new(
            "album_parent_invalid",
            format!(
                "parent `{wanted}` has no valid chain to an area or project root (missing, ambiguous, or cyclic)"
            ),
            Some("run `bob albums check` and repair the parent chain first".to_string()),
        ));
    }
    if entry.owner.is_none() || entry.root.is_none() {
        return Err(CreateRefusal::new(
            "album_parent_invalid",
            format!(
                "parent `{wanted}` reaches no area or project root, so the new album would have no catalog rows"
            ),
            Some("run `bob albums check` and repair the parent chain first".to_string()),
        ));
    }
    if let Some(finding) = entry
        .findings
        .iter()
        .find(|finding| finding.severity == super::AlbumIssueSeverity::Error)
    {
        return Err(CreateRefusal::new(
            "album_invalid",
            format!("the planned album is invalid: {}", finding.message),
            None,
        ));
    }

    let catalog_plan = catalog::plan_catalogs(&overlay, &overlay_registry);
    let affected = catalog_plan.hosts_for_album(&md_path);
    if affected.is_empty() {
        return Err(CreateRefusal::new(
            "album_parent_invalid",
            format!(
                "parent `{wanted}` gains no catalog rows for the new album"
            ),
            Some(
                "run `bob albums check` and repair the parent chain first"
                    .to_string(),
            ),
        ));
    }
    let mut catalogs = Vec::new();
    let mut edits = Vec::new();
    for host in &affected {
        let Some(plan) = catalog_plan.host(host) else {
            continue;
        };
        if plan.record.state == catalog::CatalogState::Damaged {
            return Err(CreateRefusal::new(
                "album_catalog_damaged",
                format!(
                    "the catalog in {} is damaged, so nothing was created",
                    plan.record.path
                ),
                Some(
                    "repair its album catalog markers by hand, then retry"
                        .to_string(),
                ),
            ));
        }
        catalogs.push(plan.record.clone());
        if let Some(edit) = &plan.edit {
            edits.push(edit.clone());
        }
    }

    // Guard absent destinations and the empty record folder, then the
    // owner/ancestor/claim preimages catalog rows actually use. The
    // membership evidence comes from the base snapshot: the guarded
    // install validates these inputs against the disk before writing, so
    // overlay bytes (which include the staged files) would always refuse.
    // Vault-root names (every kind) make a later uppercase `.base` or
    // directory invalidate the same plan.
    let mut deps = catalog::catalog_read_dependencies(snapshot, &base_registry);
    deps.insert(md_path.clone(), None);
    deps.insert(base_path.clone(), None);
    deps.insert_directory_membership(PathBuf::from(&stem), None);
    let namespace = root_entries
        .into_iter()
        .filter(|name| is_album_namespace_name(name))
        .collect();
    deps.insert_namespace(PathBuf::from("."), namespace);

    let parent_stem = parent_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    Ok(CreatePlan {
        path: md_path,
        base_path,
        stem,
        title,
        summary: inputs.summary,
        parent_stem,
        noun: inputs.noun,
        label,
        contents,
        base_contents,
        warnings: inputs.warnings,
        catalogs,
        edits,
        deps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::schema::RecordChoice;

    #[test]
    fn album_schema_emitter_uses_indented_yaml_sequences() {
        let fields = [RecordField {
            key: "status".to_owned(),
            label: "Status".to_owned(),
            kind: FieldKind::Choices(vec![
                RecordChoice {
                    value: "to_try".to_owned(),
                    label: "To try".to_owned(),
                },
                RecordChoice {
                    value: "liked".to_owned(),
                    label: "Liked".to_owned(),
                },
            ]),
            required: true,
            list: false,
            default: None,
            suggest: false,
            min: None,
            max: None,
        }];
        let legacy = ["lid".to_owned(), "source_*".to_owned()];
        let rendered = render_album_record_schema(
            "restaurant",
            "Restaurant",
            Some("restaurants"),
            true,
            &fields,
            &legacy,
        );
        assert_eq!(
            rendered,
            concat!(
                "record:\n",
                "  noun: restaurant\n",
                "  label: Restaurant\n",
                "  plural: restaurants\n",
                "  dated_notes: true\n",
                "  fields:\n",
                "    status:\n",
                "      label: Status\n",
                "      required: true\n",
                "      values:\n",
                "        to_try: To try\n",
                "        liked: Liked\n",
                "  legacy:\n",
                "    - lid\n",
                "    - source_*\n",
            )
        );
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&rendered).expect("rendered schema parses");
        let legacy_items = parsed["record"]["legacy"]
            .as_sequence()
            .expect("legacy sequence");
        assert_eq!(legacy_items[0].as_str(), Some("lid"));
        assert_eq!(legacy_items[1].as_str(), Some("source_*"));
    }

    fn refusal(
        stem: &str,
        noun: &str,
        summary: &str,
        title: Option<&str>,
        label: Option<&str>,
        emoji: Option<&str>,
    ) -> &'static str {
        validate_inputs(stem, noun, summary, title, label, emoji)
            .unwrap_err()
            .code
    }

    #[test]
    fn input_validation_refuses_and_warns() {
        assert_eq!(
            refusal("bad stem", "ok", "S", None, None, None),
            "album_stem_invalid"
        );
        assert_eq!(
            refusal("DONE", "ok", "S", None, None, None),
            "album_stem_invalid"
        );
        assert_eq!(
            refusal("eat", "", "S", None, None, None),
            "album_noun_invalid"
        );
        assert_eq!(
            refusal("eat", "Bad", "S", None, None, None),
            "album_noun_invalid"
        );
        assert_eq!(
            refusal("eat", "album", "S", None, None, None),
            "album_noun_reserved"
        );
        assert_eq!(
            refusal("eat", "record", "S", None, None, None),
            "album_noun_reserved"
        );
        assert_eq!(
            refusal("eat", "ok", "  ", None, None, None),
            "album_summary_required"
        );
        assert_eq!(
            refusal("eat", "ok", "a\nb", None, None, None),
            "album_summary_multiline"
        );
        assert_eq!(
            refusal("eat", "ok", "S", Some(" "), None, None),
            "album_title_invalid"
        );
        assert_eq!(
            refusal("eat", "ok", "S", None, Some("a\nb"), None),
            "album_label_invalid"
        );
        assert_eq!(
            refusal("eat", "ok", "S", None, None, Some("a\nb")),
            "album_emoji_invalid"
        );
        let long = "x".repeat(141);
        let valid =
            validate_inputs("eat", "ok", &long, None, None, None).unwrap();
        assert_eq!(valid.warnings.len(), 1);
        assert!(valid.warnings[0].starts_with("album_summary_long"));
    }

    #[test]
    fn stems_normalize_to_lowercase_routes() {
        let valid =
            validate_inputs("Eat", "ok", "S", None, None, None).unwrap();
        assert_eq!(valid.stem, "eat");
    }

    #[test]
    fn renders_canonical_key_order_h1_intro_and_label() {
        let text = render_new_album(&NewAlbumInput {
            parent_link: "body",
            summary: "Places to eat.",
            emoji: Some("🍽️"),
            title: "Restaurants",
            created: "2026-11-09T09:14:00+0000",
            noun: "restaurant",
            label: "Restaurant",
        });
        assert_eq!(
            text,
            "---\nparent: \"[[body]]\"\ntype: \"[[album]]\"\nsummary: Places to eat.\nemoji: 🍽️\ncreated: 2026-11-09T09:14:00+0000\nrecord:\n  noun: restaurant\n  label: Restaurant\n---\n\n# Restaurants\n"
        );
        assert!(render_intro("restaurants", "eat").contains("`Name @eat/`"));
        assert!(render_intro("restaurants", "eat").contains("[[eat.base]]"));
    }

    #[test]
    fn default_title_humanizes_the_stem() {
        assert_eq!(default_title("eat"), "Eat");
        assert_eq!(default_title("tv_shows"), "Tv shows");
    }

    fn plan_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        std::fs::create_dir_all(&vault).expect("vault");
        std::fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n",
        )
        .expect("area");
        (temp, vault)
    }

    fn plan_request() -> CreateRequest {
        CreateRequest {
            parent: "body".to_owned(),
            stem: "eat".to_owned(),
            noun: "restaurant".to_owned(),
            summary: "Places to eat.".to_owned(),
            title: None,
            label: None,
            emoji: None,
            now: chrono::NaiveDate::from_ymd_opt(2026, 11, 9)
                .unwrap()
                .and_hms_opt(9, 14, 0)
                .unwrap(),
            timezone: None,
        }
    }

    fn plan_over(
        vault: &Path,
        mutate: impl FnOnce(&mut CreateRequest),
    ) -> Result<CreatePlan, CreateRefusal> {
        let snapshot = VaultSnapshot::load(vault);
        assert!(snapshot.io_errors.is_empty(), "{:?}", snapshot.io_errors);
        let mut request = plan_request();
        mutate(&mut request);
        plan_create(vault, &snapshot, &request)
    }

    #[test]
    fn plans_the_album_base_and_owner_catalog_from_one_overlay() {
        let (_temp, vault) = plan_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        assert_eq!(plan.path, PathBuf::from("eat.md"));
        assert_eq!(plan.base_path, PathBuf::from("eat.base"));
        assert_eq!(plan.title, "Eat");
        assert_eq!(plan.label, "Restaurant");
        assert!(plan.contents.contains("parent: \"[[body]]\""));
        assert!(plan.contents.contains("# Eat\n"));
        assert!(plan.contents.contains("`Name @eat/`"));
        assert!(plan.contents.contains("## Records"));
        assert!(plan.contents.contains("> [!records] No restaurants yet"));
        assert!(
            plan.base_contents.contains("file.inFolder(\"eat\")"),
            "{}",
            plan.base_contents
        );
        assert!(
            plan.base_contents.contains("note.type == link(\"record\")"),
            "{}",
            plan.base_contents
        );
        assert_eq!(plan.catalogs.len(), 1);
        assert_eq!(plan.catalogs[0].path, "body.md");
        assert!(plan
            .edits
            .iter()
            .any(|edit| edit.path == Path::new("body.md")));
        assert!(plan
            .deps
            .iter()
            .any(|(path, _)| path == Path::new("eat.md")));
    }

    #[test]
    fn plan_refuses_existing_destinations_claims_parents_and_folders() {
        let (_temp, vault) = plan_fixture();
        std::fs::write(vault.join("taken.md"), "# Taken\n").expect("taken");
        let code =
            plan_over(&vault, |request| request.stem = "taken".to_owned())
                .unwrap_err()
                .code;
        assert_eq!(code, "album_exists");

        std::fs::create_dir_all(vault.join("full")).expect("folder");
        std::fs::write(vault.join("full/note.md"), "# Note\n").expect("note");
        let code =
            plan_over(&vault, |request| request.stem = "full".to_owned())
                .unwrap_err()
                .code;
        assert_eq!(code, "album_folder_exists");

        std::fs::write(vault.join("eat.md"), "# Plain\n").expect("plain");
        for (parent, code) in [
            ("missing", "album_parent_missing"),
            ("eat", "album_parent_unsupported"),
        ] {
            let refusal = plan_over(&vault, |request| {
                request.parent = parent.to_owned();
                request.stem = "other".to_owned();
            })
            .unwrap_err();
            assert_eq!(refusal.code, code, "{parent}");
        }
    }

    #[test]
    fn plan_refuses_case_insensitive_non_file_destinations() {
        let (_temp, vault) = plan_fixture();
        std::fs::write(vault.join("EAT.base"), "filters: {}\n").expect("base");
        let refusal = plan_over(&vault, |_| {}).unwrap_err();
        assert_eq!(refusal.code, "album_exists");
        assert!(refusal.message.contains("EAT.base"), "{}", refusal.message);

        std::fs::remove_file(vault.join("EAT.base")).expect("remove base");
        std::fs::create_dir_all(vault.join("EAT.md")).expect("dir dest");
        let refusal = plan_over(&vault, |_| {}).unwrap_err();
        assert_eq!(refusal.code, "album_exists");
        assert!(refusal.message.contains("EAT.md"), "{}", refusal.message);
    }

    #[cfg(unix)]
    #[test]
    fn plan_refuses_dangling_symlink_destinations() {
        let (_temp, vault) = plan_fixture();
        std::os::unix::fs::symlink("missing-target", vault.join("EAT.base"))
            .expect("dangling symlink");
        let refusal = plan_over(&vault, |_| {}).unwrap_err();
        assert_eq!(refusal.code, "album_exists");
        assert!(refusal.message.contains("EAT.base"), "{}", refusal.message);
    }

    #[test]
    fn stale_plan_notices_a_collision_introduced_after_planning() {
        let (_temp, vault) = plan_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        std::fs::write(vault.join("EAT.base"), "filters: {}\n")
            .expect("plant base");
        let error = plan.deps.validate(&vault).expect_err("namespace changed");
        assert!(error.message.contains("namespace"), "{}", error.message);
    }

    #[test]
    fn stale_plan_notices_an_ordinary_note_becoming_a_claim() {
        let (_temp, vault) = plan_fixture();
        std::fs::write(vault.join("plain.md"), "# Plain\n").expect("plain");
        let plan = plan_over(&vault, |_| {}).expect("plan");
        std::fs::write(
            vault.join("plain.md"),
            "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Stolen.\nrecord:\n  noun: restaurant\n---\n\n# Stolen\n",
        )
        .expect("claim");
        let error = plan.deps.validate(&vault).expect_err("claim appeared");
        assert!(error.message.contains("plain.md"), "{}", error.message);
    }

    #[test]
    fn stale_plan_notices_a_nested_identity_and_host_preimage_change() {
        let (_temp, vault) = plan_fixture();
        std::fs::create_dir_all(vault.join("areas")).expect("areas");
        std::fs::write(
            vault.join("areas/twin.md"),
            "---\ntype: [[area]]\n---\n\n# Twin\n",
        )
        .expect("twin");
        let plan = plan_over(&vault, |_| {}).expect("plan");
        std::fs::write(
            vault.join("areas/body.md"),
            "---\ntype: [[area]]\n---\n\n# Nested Body\n",
        )
        .expect("nested body");
        let error = plan
            .deps
            .validate(&vault)
            .expect_err("nested identity changed");
        assert!(
            error.message.contains("Markdown membership"),
            "{}",
            error.message
        );

        let (_temp, vault) = plan_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        std::fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body edited\n",
        )
        .expect("host");
        let error = plan.deps.validate(&vault).expect_err("host changed");
        assert!(error.message.contains("body.md"), "{}", error.message);
    }

    fn git_fixture() -> (tempfile::TempDir, PathBuf) {
        let (temp, vault) = plan_fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&vault)
                .args(args)
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        (temp, vault)
    }

    fn staged_from_plan(
        vault: &Path,
        plan: &CreatePlan,
    ) -> Vec<crate::native::capture::StagedTextFile> {
        let mut files = vec![
            crate::native::capture::StagedTextFile {
                target: vault.join(&plan.path),
                target_existed: false,
                original_target: String::new(),
                updated_target: plan.contents.clone(),
            },
            crate::native::capture::StagedTextFile {
                target: vault.join(&plan.base_path),
                target_existed: false,
                original_target: String::new(),
                updated_target: plan.base_contents.clone(),
            },
        ];
        files.extend(crate::native::host_catalog::staged_rewrites(
            vault,
            &plan.edits,
        ));
        files
    }

    #[test]
    fn apply_refuses_a_stale_plan_without_writing() {
        let (_temp, vault) = git_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        std::fs::write(vault.join("plain.md"), "# Plain\n").expect("plain");
        let txn = crate::native::host_catalog::PlannedTxn {
            files: staged_from_plan(&vault, &plan),
            appends: Vec::new(),
            deps: plan.deps.clone(),
            subject: "albums: create eat.md".to_owned(),
            verify: Box::new(|| Ok(())),
            context: (),
            kind: "album",
        };
        let error = crate::native::host_catalog::apply_and_commit(
            &vault,
            &Vec::new(),
            txn,
        )
        .expect_err("stale");
        assert_eq!(error.code, "stale_note");
        assert!(!vault.join("eat.md").exists());
        assert!(!vault.join("eat.base").exists());
        assert_eq!(
            std::fs::read_to_string(vault.join("body.md")).unwrap(),
            "---\ntype: [[area]]\n---\n\n# Body\n"
        );
    }

    #[test]
    fn verify_failure_rolls_back_album_base_and_owner() {
        let (_temp, vault) = git_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        let body_before =
            std::fs::read_to_string(vault.join("body.md")).unwrap();
        let txn = crate::native::host_catalog::PlannedTxn {
            files: staged_from_plan(&vault, &plan),
            appends: Vec::new(),
            deps: plan.deps.clone(),
            subject: "albums: create eat.md".to_owned(),
            verify: Box::new(|| Err("forced verifier failure".to_owned())),
            context: (),
            kind: "album",
        };
        let error = crate::native::host_catalog::apply_and_commit(
            &vault,
            &Vec::new(),
            txn,
        )
        .expect_err("verify");
        assert_eq!(error.code, "verify_failed");
        assert!(!vault.join("eat.md").exists());
        assert!(!vault.join("eat.base").exists());
        assert_eq!(
            std::fs::read_to_string(vault.join("body.md")).unwrap(),
            body_before
        );
    }

    #[test]
    fn rollback_preserves_an_intervening_owner_edit() {
        let (_temp, vault) = git_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        let owner = vault.join("body.md");
        let txn = crate::native::host_catalog::PlannedTxn {
            files: staged_from_plan(&vault, &plan),
            appends: Vec::new(),
            deps: plan.deps.clone(),
            subject: "albums: create eat.md".to_owned(),
            verify: Box::new({
                let owner = owner.clone();
                move || {
                    std::fs::write(&owner, "later user edit\n")
                        .expect("intervening edit");
                    Err("forced verifier failure".to_owned())
                }
            }),
            context: (),
            kind: "album",
        };
        let error = crate::native::host_catalog::apply_and_commit(
            &vault,
            &Vec::new(),
            txn,
        )
        .expect_err("verify");
        assert_eq!(error.code, "verify_failed");
        assert!(!vault.join("eat.md").exists());
        assert!(!vault.join("eat.base").exists());
        assert_eq!(
            std::fs::read_to_string(&owner).unwrap(),
            "later user edit\n"
        );
    }

    #[test]
    fn verify_rejects_a_damaged_required_host() {
        let (_temp, vault) = plan_fixture();
        let plan = plan_over(&vault, |_| {}).expect("plan");
        std::fs::write(vault.join(&plan.path), &plan.contents).expect("album");
        std::fs::write(vault.join(&plan.base_path), &plan.base_contents)
            .expect("base");
        std::fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums\n> - partial\n",
        )
        .expect("damage");
        let error = super::super::create_run::verify_installed_create(
            &vault,
            &plan,
            &[PathBuf::from("body.md")],
        )
        .expect_err("damaged");
        assert!(error.contains("body.md"), "{error}");
    }
}

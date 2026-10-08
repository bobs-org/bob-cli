//! Shared target planning, marker composition, and stamp-plus-install helpers.
//!
//! `create` writes one marker-stamped PDF into the Highlights
//! intake (`xlib/<ref_type>/<stem>.pdf`); this module owns the reusable parts:
//! resolving the target, refusing collisions, composing the marker (including
//! the optional web-provenance keys), and stamping plus atomically installing
//! a rendered PDF.
use super::*;
use lopdf::dictionary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TargetWorkflow {
    Intake { library_destination: PathBuf },
    Library,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TargetPlan {
    pub(super) target: PathBuf,
    pub(super) sidecar: PathBuf,
    pub(super) workflow: TargetWorkflow,
    pub(super) stem: String,
    pub(super) renamed_from: Option<RenamedFrom>,
}

/// The base stem plus the first occupant that forced a default-target
/// rename. Rendered as
/// `renamed: <base>.pdf is taken by <occupant>; using <final>.pdf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenamedFrom {
    pub(super) base_stem: String,
    pub(super) occupant: PathBuf,
}

/// Route-supplied identity for the default-target walk.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) enum DefaultTargetIdentity<'a> {
    Url {
        keys: Vec<String>,
        recorded: &'a [super::sources::RecordedSource],
    },
    Title(String),
    AlwaysDifferent,
}

impl TargetPlan {
    pub(super) fn renamed_line(&self) -> Option<String> {
        let renamed = self.renamed_from.as_ref()?;
        Some(format!(
            "renamed: {}.pdf is taken by {}; using {}.pdf",
            renamed.base_stem,
            renamed.occupant.display(),
            self.stem,
        ))
    }
}

/// Optional document Info fields set by [`stamp_and_install`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PdfInfo {
    pub(super) title: Option<String>,
    pub(super) author: Option<String>,
}

/// Canonical insertion order for the optional provenance marker keys.
pub(super) const MARKER_EXTRA_ORDER: &[&str] = &[
    "source_url",
    "author",
    "published",
    "captured",
    "return_links",
];

/// Plan the default intake target `xlib/<ref_type>/<stem>.pdf`.
///
/// Walks `<stem>`, `<stem>_2`, … up to `<stem>_999`: a free candidate is
/// chosen, a candidate occupied by the same reference refuses (or is
/// overwritten with `--force` when it is the intake PDF), and a candidate
/// occupied by a different reference is skipped with the next suffix.
/// `--force` never selects a candidate owned by a different reference.
pub(super) fn plan_default_target(
    config: &Config,
    stem: &OsStr,
    ref_type: &str,
    force: bool,
    identity: &DefaultTargetIdentity<'_>,
) -> Result<TargetPlan> {
    validate_ref_type(ref_type)?;
    let base = stem
        .to_str()
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| {
            CommandError::new(
                "output filename stem is not a nonempty UTF-8 name",
            )
        })?
        .to_string();
    let mut base_first_occupant: Option<PathBuf> = None;
    for index in 1..=999 {
        let candidate = if index == 1 {
            base.clone()
        } else {
            format!("{base}_{index}")
        };
        let target = config
            .xlib_dir
            .join(ref_type)
            .join(&candidate)
            .with_extension("pdf");
        let library_destination = config
            .lib_dir
            .join(ref_type)
            .join(&candidate)
            .with_extension("pdf");
        let workflow = TargetWorkflow::Intake {
            library_destination: library_destination.clone(),
        };
        let sidecar = target.with_extension("md");
        let ref_note = config
            .ref_dir
            .join(ref_type)
            .join(&candidate)
            .with_extension("md");
        let occupants = default_stem_occupants(config, ref_type, &candidate);
        if index == 1
            && let Some(first) = occupants.first().cloned()
        {
            base_first_occupant = Some(first);
        }
        if occupants.is_empty() {
            let plan = TargetPlan {
                target: target.clone(),
                sidecar: sidecar.clone(),
                workflow: workflow.clone(),
                stem: candidate.clone(),
                renamed_from: None,
            };
            // Free candidates pass trivially; keep the strict checks as
            // the single source of truth.
            refuse_target_collisions(&target, &sidecar, &workflow, force)?;
            if candidate == base {
                return Ok(plan);
            }
            return Ok(TargetPlan {
                renamed_from: base_first_occupant.map(|occupant| RenamedFrom {
                    base_stem: base.clone(),
                    occupant,
                }),
                ..plan
            });
        }
        let same = candidate_is_same_reference(
            &target,
            &library_destination,
            &ref_note,
            identity,
        );
        if !same {
            continue;
        }
        // Same reference: library or ref-note occupants always refuse;
        // an intake-only occupant refuses without --force and overwrites
        // with --force (keeping the library/sidecar checks).
        let has_library_or_note = occupants.iter().any(|occupant| {
            occupant == &library_destination
                || occupant == &ref_note
                || library_destination_sidecars(&library_destination)
                    .contains(occupant)
        });
        if has_library_or_note {
            let hint = format!(
                "to add audio to that capture, run bob ref create {} --listen",
                library_destination.display()
            );
            return Err(CommandError::new(format!(
                "already captured as {} ({}); pass -N/--name to capture under a different name\nhint: {hint}",
                occupants
                    .iter()
                    .find(|occupant| *occupant == &library_destination
                        || *occupant == &ref_note
                        || library_destination_sidecars(&library_destination)
                            .contains(occupant))
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| library_destination.display().to_string()),
                normalize_title_for_report(identity),
            )));
        }
        if !force {
            let hint = format!(
                "to add audio to that capture, run bob ref create {} --listen",
                library_destination.display()
            );
            return Err(CommandError::new(format!(
                "already queued in {}; pass --force to overwrite it\nhint: {hint}",
                target.display(),
            )));
        }
        // --force same-intake overwrite: the library/sidecar checks stay.
        refuse_target_collisions(&target, &sidecar, &workflow, force)?;
        if candidate == base {
            return Ok(TargetPlan {
                target,
                sidecar,
                workflow,
                stem: candidate,
                renamed_from: None,
            });
        }
        return Ok(TargetPlan {
            target,
            sidecar,
            workflow,
            stem: candidate,
            renamed_from: base_first_occupant.map(|occupant| RenamedFrom {
                base_stem: base.clone(),
                occupant,
            }),
        });
    }
    Err(CommandError::new(format!(
        "no free default filename for {base}; pass -N/--name or -o/--output"
    )))
}

/// Every path that blocks a default stem: intake PDF and Highlights
/// sidecars, library PDF and sidecars, and the ref note `scan` would
/// write. Companion audio is deliberately excluded.
pub(super) fn default_stem_occupants(
    config: &Config,
    ref_type: &str,
    stem: &str,
) -> Vec<PathBuf> {
    let intake_pdf = config
        .xlib_dir
        .join(ref_type)
        .join(stem)
        .with_extension("pdf");
    let intake_md = config
        .xlib_dir
        .join(ref_type)
        .join(stem)
        .with_extension("md");
    let intake_bundle = config
        .xlib_dir
        .join(ref_type)
        .join(stem)
        .with_extension("textbundle");
    let library_pdf = config
        .lib_dir
        .join(ref_type)
        .join(stem)
        .with_extension("pdf");
    let library_sidecars = library_destination_sidecars(&library_pdf);
    let ref_note = config
        .ref_dir
        .join(ref_type)
        .join(stem)
        .with_extension("md");
    let mut occupants = Vec::new();
    for path in [
        intake_pdf,
        intake_md,
        intake_bundle,
        library_pdf,
        library_sidecars[0].clone(),
        library_sidecars[1].clone(),
        ref_note,
    ] {
        if path.exists() {
            occupants.push(path);
        }
    }
    occupants
}

fn normalize_title_for_report(identity: &DefaultTargetIdentity<'_>) -> String {
    match identity {
        DefaultTargetIdentity::Title(title) => {
            format!("title {}", title.trim())
        }
        DefaultTargetIdentity::Url { keys, .. } => {
            keys.first().cloned().unwrap_or_else(|| "url".to_string())
        }
        DefaultTargetIdentity::AlwaysDifferent => "stem".to_string(),
    }
}

/// Per-candidate identity: only intake/library PDFs (local title routes)
/// and, for URL routes, recorded ref notes count. Sidecar-only,
/// ref-note-only (local), and unmarked-PDF occupants are always a
/// different reference.
fn candidate_is_same_reference(
    intake_pdf: &Path,
    library_pdf: &Path,
    ref_note: &Path,
    identity: &DefaultTargetIdentity<'_>,
) -> bool {
    match identity {
        DefaultTargetIdentity::AlwaysDifferent => false,
        DefaultTargetIdentity::Title(planned) => {
            title_matches_pdf(planned, intake_pdf)
                || title_matches_pdf(planned, library_pdf)
        }
        DefaultTargetIdentity::Url { keys, recorded } => {
            recorded.iter().any(|hit| {
                keys.contains(&hit.dedupe_key)
                    && (normalize_lexically(&hit.path)
                        == normalize_lexically(intake_pdf)
                        || (hit.is_ref_note
                            && hit.source_pdf.is_some()
                            && normalize_lexically(&hit.path)
                                == normalize_lexically(ref_note)))
            })
        }
    }
}

/// Trim, collapse whitespace, and lowercase for title comparisons.
pub(super) fn normalize_title(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// True when the PDF at `path` carries the planned title as its page-1
/// marker `title` or its raw Info `Title`.
fn title_matches_pdf(planned: &str, path: &Path) -> bool {
    let wanted = normalize_title(planned);
    if wanted.is_empty() || !path.is_file() {
        return false;
    }
    if let Ok(marker) = super::marker::read_pdf_marker(path)
        && let Ok(projection) = super::marker::parse_marker(&marker.contents)
        && let Some(title) =
            projection.get("title").and_then(|value| value.as_string())
        && normalize_title(title) == wanted
    {
        return true;
    }
    if let Some(info) = super::pdf_meta::raw_info_title(path)
        && normalize_title(&info) == wanted
    {
        return true;
    }
    false
}

/// Plan an exact `--output` target path.
pub(super) fn plan_exact_output(
    config: &Config,
    output: &Path,
    force: bool,
) -> Result<TargetPlan> {
    let target = resolve_exact_output_path(output)?;
    validate_pdf_output_path(&target)?;
    let workflow = classify_target(config, &target)?;
    let sidecar = target.with_extension("md");
    refuse_target_collisions(&target, &sidecar, &workflow, force)?;
    let stem = target
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();
    Ok(TargetPlan {
        target,
        sidecar,
        workflow,
        stem,
        renamed_from: None,
    })
}

pub(super) fn refuse_target_collisions(
    target: &Path,
    sidecar: &Path,
    workflow: &TargetWorkflow,
    force: bool,
) -> Result<()> {
    if sidecar.exists() {
        return Err(CommandError::new(format!(
            "refusing to create {} because Highlights would treat the existing Markdown file as its sidecar: {}",
            target.display(),
            sidecar.display()
        )));
    }
    if let TargetWorkflow::Intake {
        library_destination,
    } = workflow
    {
        if library_destination.exists() {
            return Err(CommandError::new(format!(
                "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob ref scan would refuse to move the new PDF over it)\nhint: to add audio to that capture, run bob ref create {} --listen",
                target.display(),
                library_destination.display(),
                library_destination.display()
            )));
        }
        for library_sidecar in library_destination_sidecars(library_destination)
        {
            if library_sidecar.exists() {
                return Err(CommandError::new(format!(
                    "refusing to create {} because the library destination sidecar already exists: {}; remove or rename the archived sidecar before recreating it (bob ref scan would refuse to move the new PDF sidecar over it)",
                    target.display(),
                    library_sidecar.display()
                )));
            }
        }
    }
    if target.exists() && !force {
        let hint = match workflow {
            TargetWorkflow::Intake {
                library_destination,
            } => format!(
                "to add audio to that capture, run bob ref create {} --listen",
                library_destination.display()
            ),
            _ => format!(
                "to add audio to that capture, run bob ref create {} --listen",
                target.display()
            ),
        };
        return Err(CommandError::new(format!(
            "target PDF already exists: {}; pass --force to overwrite it\nhint: {hint}",
            target.display()
        )));
    }
    Ok(())
}

pub(super) fn resolve_exact_output_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(CommandError::new(
            "output path must include a nonempty filename with a .pdf extension",
        ));
    }
    let cwd = env::current_dir().map_err(|error| {
        CommandError::new(format!("resolve current directory: {error}"))
    })?;
    Ok(resolve_exact_output_path_from(path, &cwd))
}

pub(super) fn resolve_exact_output_path_from(
    path: &Path,
    cwd: &Path,
) -> PathBuf {
    let expanded = bob_env::expand_tilde(path);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    normalize_lexically(&absolute)
}

pub(super) fn validate_pdf_output_path(path: &Path) -> Result<()> {
    let file_name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CommandError::new(format!(
                "output path must include a nonempty filename: {}",
                path.display()
            ))
        })?;
    let is_pdf = Path::new(file_name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return Err(CommandError::new(format!(
            "output path must have a .pdf extension: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn classify_target(
    config: &Config,
    target: &Path,
) -> Result<TargetWorkflow> {
    let intake = resolve_exact_output_path(&config.xlib_dir)?;
    let library = resolve_exact_output_path(&config.lib_dir)?;
    if let Some(relative) = relative_inside(target, &intake) {
        return Ok(TargetWorkflow::Intake {
            library_destination: library.join(relative),
        });
    }
    if path_is_inside(target, &library) {
        return Ok(TargetWorkflow::Library);
    }
    Ok(TargetWorkflow::External)
}

pub(super) fn relative_inside(child: &Path, parent: &Path) -> Option<PathBuf> {
    let child = normalize_lexically(child);
    let parent = normalize_lexically(parent);
    child.strip_prefix(parent).ok().and_then(|relative| {
        (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf())
    })
}

pub(super) fn path_is_inside(child: &Path, parent: &Path) -> bool {
    relative_inside(child, parent).is_some()
}

/// Containment against canonicalized directories: resolves symlinks and
/// relative vault paths (`-b vault`) so a relative or symlinked vault
/// still matches. Falls back to lexical comparison when canonicalization
/// fails.
pub(super) fn relative_inside_canonical(
    child: &Path,
    parent: &Path,
) -> Option<PathBuf> {
    if let (Ok(child_c), Ok(parent_c)) =
        (std::fs::canonicalize(child), std::fs::canonicalize(parent))
    {
        if let Some(rel) = relative_inside(&child_c, &parent_c) {
            return Some(rel);
        }
    }
    relative_inside(child, parent)
}

pub(super) fn path_is_inside_canonical(child: &Path, parent: &Path) -> bool {
    relative_inside_canonical(child, parent).is_some()
}

pub(super) fn normalize_lexically(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match components.last() {
                Some(Component::Normal(_)) => {
                    components.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => components.push(component),
            },
            other => components.push(other),
        }
    }
    components.iter().collect()
}

pub(super) fn library_destination_sidecars(
    library_destination: &Path,
) -> [PathBuf; 2] {
    [
        library_destination.with_extension("md"),
        library_destination.with_extension("textbundle"),
    ]
}

pub(super) fn validate_ref_type(ref_type: &str) -> Result<()> {
    let path = Path::new(ref_type);
    let mut components = path.components();
    let valid = matches!(
        components.next(),
        Some(Component::Normal(value)) if !value.is_empty()
    ) && components.next().is_none();
    if !valid {
        return Err(CommandError::new(format!(
            "ref type must be one directory name, not a path: {ref_type:?}"
        )));
    }
    Ok(())
}

/// Compose a page-1 marker with the required keys plus the optional
/// provenance extras (`source_url`, `author`, `published`, `captured`,
/// `return_links`). Extras are inserted in [`MARKER_EXTRA_ORDER`] order;
/// empty strings are skipped. Values keep their [`MarkerValue`] shape so
/// `return_links` rides as a boolean and renders as `- return_links: true`.
pub(super) fn compose_marker(
    status: &str,
    parent: &str,
    title: &str,
    id: Option<&str>,
    extras: &[(&str, MarkerValue)],
) -> Result<String> {
    validate_marker_parent_value(
        parent,
        2,
        &MarkerValue::String(parent.to_string()),
    )?;
    let mut projection = Projection::new();
    projection.insert(
        FIELD_STATUS.to_string(),
        MarkerValue::String(status.to_string()),
    );
    projection.insert(
        FIELD_PARENT.to_string(),
        MarkerValue::String(format!("[[{parent}]]")),
    );
    projection
        .insert("title".to_string(), MarkerValue::String(title.to_string()));
    if let Some(id) = id {
        projection
            .insert(FIELD_ID.to_string(), MarkerValue::String(id.to_string()));
    }
    let mut ordered: Vec<(&str, &MarkerValue)> = extras
        .iter()
        .map(|(key, value)| (*key, value))
        .filter(|(_, value)| {
            !matches!(value, MarkerValue::String(text) if text.is_empty())
        })
        .collect();
    ordered.sort_by_key(|(key, _)| {
        MARKER_EXTRA_ORDER
            .iter()
            .position(|ordered| ordered == key)
            .unwrap_or(MARKER_EXTRA_ORDER.len())
    });
    for (key, value) in ordered {
        projection.insert(key.to_string(), value.clone());
    }
    validate_required_marker_keys(&projection, "create marker")?;
    let marker = render_marker(&projection)?;
    let normalized = parse_marker_with_normalization(&marker)?;
    validate_required_marker_keys(&normalized.projection, "create marker")?;
    render_marker(&normalized.projection)
}

pub(super) fn embed_marker(
    document: &mut Document,
    marker: &str,
) -> Result<()> {
    let first_page_id = document
        .page_iter()
        .next()
        .ok_or_else(|| CommandError::new("rendered PDF has no first page"))?;
    let annotation_id = document.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(24),
            Object::Integer(24),
        ],
        "Contents" => pdf_text_string(marker),
    });
    let annotation = Object::Reference(annotation_id);
    let annots = document
        .get_dictionary(first_page_id)
        .ok()
        .and_then(|page| page.get(b"Annots").ok())
        .cloned();

    match annots {
        None => document
            .get_object_mut(first_page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| {
                CommandError::new(format!(
                    "read rendered PDF first page: {error}"
                ))
            })?
            .set("Annots", Object::Array(vec![annotation])),
        Some(Object::Array(_)) => {
            let array = document
                .get_object_mut(first_page_id)
                .and_then(Object::as_dict_mut)
                .and_then(|page| page.get_mut(b"Annots"))
                .and_then(Object::as_array_mut)
                .map_err(|error| {
                    CommandError::new(format!(
                        "read rendered PDF page annotations: {error}"
                    ))
                })?;
            array.insert(0, annotation);
        }
        Some(Object::Reference(id)) => {
            let array = document
                .get_object_mut(id)
                .and_then(Object::as_array_mut)
                .map_err(|error| {
                    CommandError::new(format!(
                        "read rendered PDF annotation array: {error}"
                    ))
                })?;
            array.insert(0, annotation);
        }
        Some(_) => {
            return Err(CommandError::new(
                "rendered PDF first-page /Annots value is not an array",
            ));
        }
    }
    Ok(())
}

/// Load a rendered PDF, count its pages, embed the marker, stamp the
/// optional document Info fields, and atomically install it at `target`.
/// Returns the page count.
pub(super) fn stamp_and_install(
    rendered_pdf: &Path,
    target: &Path,
    marker: &str,
    info: &PdfInfo,
) -> Result<usize> {
    let mut document = Document::load(rendered_pdf).map_err(|error| {
        CommandError::new(format!(
            "read rendered PDF {}: {error}",
            rendered_pdf.display()
        ))
    })?;
    let page_count = document.get_pages().len();
    embed_marker(&mut document, marker)?;
    if info.title.is_some() || info.author.is_some() {
        set_pdf_info(&mut document, info)?;
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create output directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    atomic_save_pdf(target, &mut document)?;
    Ok(page_count)
}

pub(super) fn set_pdf_info_for_route(
    document: &mut Document,
    info: &PdfInfo,
) -> Result<()> {
    set_pdf_info(document, info)
}

fn set_pdf_info(document: &mut Document, info: &PdfInfo) -> Result<()> {
    let info_id = match document.trailer.get(b"Info") {
        Ok(Object::Reference(id)) => *id,
        Ok(Object::Dictionary(_)) => {
            let id = document.add_object(dictionary! {});
            copy_inline_info(document, id)?;
            id
        }
        _ => {
            let id = document.add_object(dictionary! {});
            document.trailer.set("Info", Object::Reference(id));
            id
        }
    };
    let fields: [(&str, Option<&String>); 2] = [
        ("Title", info.title.as_ref()),
        ("Author", info.author.as_ref()),
    ];
    let info_dict = document
        .get_object_mut(info_id)
        .and_then(Object::as_dict_mut)
        .map_err(|error| {
            CommandError::new(format!("read PDF document Info: {error}"))
        })?;
    for (key, value) in fields {
        if let Some(value) = value {
            info_dict.set(key, pdf_text_string(value));
        }
    }
    Ok(())
}

/// Move an inline (non-indirect) trailer Info dictionary into a fresh
/// indirect object so [`set_pdf_info`] has one place to write fields.
fn copy_inline_info(document: &mut Document, info_id: ObjectId) -> Result<()> {
    let inline = match document.trailer.get(b"Info") {
        Ok(Object::Dictionary(inline)) => inline.clone(),
        _ => {
            return Err(CommandError::new(
                "read PDF document Info: expected a dictionary",
            ));
        }
    };
    document
        .get_object_mut(info_id)
        .and_then(Object::as_dict_mut)
        .map_err(|error| {
            CommandError::new(format!("read PDF document Info: {error}"))
        })?
        .extend(&inline);
    document.trailer.set("Info", Object::Reference(info_id));
    Ok(())
}

pub(super) fn print_next_step(plan: &TargetPlan) {
    match &plan.workflow {
        TargetWorkflow::Intake { .. } | TargetWorkflow::Library => {
            println!("next: bob ref scan");
        }
        TargetWorkflow::External => {
            println!(
                "scan: recursive scan will not discover this PDF because it is outside the configured library and intake directories"
            );
            println!("next: bob ref sync {}", plan.target.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Stream;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-highlights-stamp-{name}-{}",
                process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn config(root: &Path) -> Config {
        Config {
            bob_dir: root.to_path_buf(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        }
    }

    fn intake_destination(plan: &TargetPlan) -> &Path {
        match &plan.workflow {
            TargetWorkflow::Intake {
                library_destination,
            } => library_destination,
            other => panic!("expected intake workflow, got {other:?}"),
        }
    }

    /// Minimal one-page PDF fixture. When `with_annots` is set, page 1
    /// already carries an `/Annots` array holding one standalone note.
    fn write_minimal_pdf(path: &Path, with_annots: bool) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let content_id =
            document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
        };
        if with_annots {
            let existing = document.add_object(dictionary! {
                "Type" => "Annot",
                "Subtype" => "Text",
                "Rect" => vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(24),
                    Object::Integer(24),
                ],
                "Contents" => pdf_text_string("existing note"),
            });
            page.set(
                "Annots",
                Object::Array(vec![Object::Reference(existing)]),
            );
        }
        let page_id = document.add_object(page);
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).expect("save fixture PDF");
    }

    fn annotation_count(path: &Path) -> usize {
        let document = Document::load(path).expect("load stamped PDF");
        let first_page_id = document.page_iter().next().expect("first page");
        annotation_ids_for_page(&document, first_page_id)
            .expect("annotation ids")
            .len()
    }

    fn info_string(path: &Path, key: &[u8]) -> Option<String> {
        let document = Document::load(path).expect("load stamped PDF");
        let info = document.trailer.get(b"Info").ok()?;
        let dictionary = match info {
            Object::Reference(id) => document.get_dictionary(*id).ok()?,
            Object::Dictionary(dictionary) => dictionary,
            _ => return None,
        };
        dictionary
            .get(key)
            .ok()
            .and_then(|value| decode_text_string(value).ok())
    }

    fn always_different() -> DefaultTargetIdentity<'static> {
        DefaultTargetIdentity::AlwaysDifferent
    }

    fn title_identity(title: &str) -> DefaultTargetIdentity<'static> {
        DefaultTargetIdentity::Title(title.to_string())
    }

    fn write_stamped_pdf(target: &Path, title: &str) {
        let rendered = target.with_extension("render.pdf");
        write_minimal_pdf(&rendered, false);
        let marker = compose_marker("ready", "obsidian_ref", title, None, &[])
            .expect("marker");
        stamp_and_install(
            &rendered,
            target,
            &marker,
            &PdfInfo {
                title: Some(title.to_string()),
                author: None,
            },
        )
        .expect("stamp occupant");
        let _ = fs::remove_file(&rendered);
    }

    fn write_info_only_pdf(target: &Path, info_title: &str) {
        write_minimal_pdf(target, false);
        let mut document = Document::load(target).expect("load info-only PDF");
        set_pdf_info(
            &mut document,
            &PdfInfo {
                title: Some(info_title.to_string()),
                author: None,
            },
        )
        .expect("set info title");
        document.save(target).expect("save info-only PDF");
    }

    #[test]
    fn default_target_derives_ref_type_output_and_valid_marker() {
        let temp = TempDir::new("plan");
        let plan = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "books",
            false,
            &always_different(),
        )
        .expect("plan");

        assert_eq!(plan.target, temp.path.join("xlib/books/report.pdf"));
        assert_eq!(plan.sidecar, temp.path.join("xlib/books/report.md"));
        assert_eq!(plan.stem, "report");
        assert_eq!(plan.renamed_from, None);
        assert_eq!(
            intake_destination(&plan),
            temp.path.join("lib/books/report.pdf")
        );
        let marker =
            compose_marker("ready", "obsidian_ref", "Report", None, &[])
                .expect("marker");
        let parsed = parse_marker_with_normalization(&marker).expect("marker");
        assert_eq!(
            parsed.projection.get(FIELD_PARENT),
            Some(&MarkerValue::String("[[obsidian_ref]]".to_string()))
        );
        assert_eq!(
            parsed.projection.get(FIELD_STATUS),
            Some(&MarkerValue::String("ready".to_string()))
        );
        assert!(!parsed.projection.contains_key(FIELD_ID));
    }

    #[test]
    fn default_target_rejects_bad_stem_and_ref_type() {
        let temp = TempDir::new("plan-invalid");
        let config = config(&temp.path);
        assert!(plan_default_target(
            &config,
            OsStr::new(""),
            "chat",
            false,
            &always_different(),
        )
        .is_err());
        assert!(plan_default_target(
            &config,
            OsStr::new("report"),
            "papers/deep",
            false,
            &always_different(),
        )
        .is_err());
    }

    #[test]
    fn default_target_free_base_has_no_rename() {
        let temp = TempDir::new("free");
        let plan = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("Report"),
        )
        .expect("free base");
        assert_eq!(plan.stem, "report");
        assert_eq!(plan.renamed_from, None);
    }

    #[test]
    fn default_target_suffixes_each_occupant_kind() {
        // Every occupant kind counts as occupied; with a different
        // reference each bumps to `_2` with a `renamed:` record.
        let cases: &[(&str, &str)] = &[
            ("intake-pdf", "xlib/chat/report.pdf"),
            ("intake-md", "xlib/chat/report.md"),
            ("intake-bundle", "xlib/chat/report.textbundle"),
            ("library-pdf", "lib/chat/report.pdf"),
            ("library-md", "lib/chat/report.md"),
            ("library-bundle", "lib/chat/report.textbundle"),
            ("ref-note", "ref/chat/report.md"),
        ];
        for (name, relative) in cases {
            let temp = TempDir::new(&format!("suffix-{name}"));
            let occupant = temp.path.join(relative);
            if relative.ends_with(".textbundle") {
                fs::create_dir_all(&occupant).expect("create bundle dir");
            } else {
                fs::create_dir_all(occupant.parent().expect("occupant parent"))
                    .expect("create occupant parent");
                fs::write(&occupant, b"existing").expect("write occupant");
            }
            let plan = plan_default_target(
                &config(&temp.path),
                OsStr::new("report"),
                "chat",
                false,
                &title_identity("A Different Title"),
            )
            .unwrap_or_else(|error| {
                panic!("{name} must suffix, got error: {error}")
            });
            assert_eq!(plan.stem, "report_2", "{name}");
            assert_eq!(
                plan.target,
                temp.path.join("xlib/chat/report_2.pdf"),
                "{name}"
            );
            let renamed = plan.renamed_from.as_ref().expect("renamed");
            assert_eq!(renamed.base_stem, "report", "{name}");
            assert_eq!(renamed.occupant, occupant, "{name}");
            let line = plan.renamed_line().expect("renamed line");
            assert!(
                line == format!(
                    "renamed: report.pdf is taken by {}; using report_2.pdf",
                    occupant.display()
                ),
                "{name}: {line}"
            );
        }
    }

    #[test]
    fn default_target_chains_to_3_when_2_is_taken() {
        let temp = TempDir::new("chain");
        for stem in ["report", "report_2"] {
            let occupant = temp.path.join(format!("xlib/chat/{stem}.pdf"));
            fs::create_dir_all(occupant.parent().expect("parent"))
                .expect("create parent");
            fs::write(&occupant, b"existing").expect("write occupant");
        }
        let plan = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("Different"),
        )
        .expect("chain to _3");
        assert_eq!(plan.stem, "report_3");
        assert_eq!(plan.target, temp.path.join("xlib/chat/report_3.pdf"));
    }

    #[test]
    fn default_target_same_intake_refuses_then_overwrites_with_force() {
        let temp = TempDir::new("same-intake");
        let target = temp.path.join("xlib/chat/report.pdf");
        fs::create_dir_all(target.parent().expect("parent"))
            .expect("create parent");
        write_stamped_pdf(&target, "Report");

        let error = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("Report"),
        )
        .expect_err("same intake must refuse without force");
        let message = error.to_string();
        assert!(message.contains("already queued"), "{message}");
        assert!(message.contains("--force"), "{message}");
        assert!(message.contains("--listen"), "{message}");

        let plan = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            true,
            &title_identity("Report"),
        )
        .expect("same intake overwrites with force");
        assert_eq!(plan.stem, "report");
        assert_eq!(plan.target, target);
        assert_eq!(plan.renamed_from, None);
    }

    #[test]
    fn default_target_same_library_refuses_with_and_without_force() {
        for force in [false, true] {
            let temp = TempDir::new(&format!("same-lib-{force}"));
            let library = temp.path.join("lib/chat/report.pdf");
            fs::create_dir_all(library.parent().expect("parent"))
                .expect("create parent");
            write_stamped_pdf(&library, "Report");

            let error = plan_default_target(
                &config(&temp.path),
                OsStr::new("report"),
                "chat",
                force,
                &title_identity("Report"),
            )
            .expect_err("same library must refuse");
            let message = error.to_string();
            assert!(message.contains("already captured"), "{message}");
            assert!(message.contains("Report"), "{message}");
            assert!(message.contains("--listen"), "{message}");
        }
    }

    #[test]
    fn default_target_different_library_suffixes() {
        let temp = TempDir::new("diff-lib");
        let library = temp.path.join("lib/chat/report.pdf");
        fs::create_dir_all(library.parent().expect("parent"))
            .expect("create parent");
        write_stamped_pdf(&library, "Archived Report");

        let plan = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("Report"),
        )
        .expect("different library suffixes");
        assert_eq!(plan.stem, "report_2");
    }

    #[test]
    fn default_target_does_not_skip_past_same_at_2() {
        let temp = TempDir::new("same-at-2");
        let different = temp.path.join("xlib/chat/report.pdf");
        fs::create_dir_all(different.parent().expect("parent"))
            .expect("create parent");
        fs::write(&different, b"existing").expect("write different");
        let same = temp.path.join("xlib/chat/report_2.pdf");
        write_stamped_pdf(&same, "Report");

        let error = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("Report"),
        )
        .expect_err("same at _2 must refuse");
        assert!(error.to_string().contains("already queued"), "{error}");
    }

    #[test]
    fn default_target_title_matches_marker_or_info_normalized() {
        let temp = TempDir::new("title-match");
        let target = temp.path.join("xlib/chat/report.pdf");
        fs::create_dir_all(target.parent().expect("parent"))
            .expect("create parent");
        write_stamped_pdf(&target, "The Real Title");
        // Whitespace and case are normalized.
        assert!(plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("  the   REAL title "),
        )
        .is_err());

        let info_temp = TempDir::new("title-info");
        let info_target = info_temp.path.join("xlib/chat/report.pdf");
        fs::create_dir_all(info_target.parent().expect("parent"))
            .expect("create parent");
        write_info_only_pdf(&info_target, "Info Title");
        assert!(plan_default_target(
            &config(&info_temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("info title"),
        )
        .is_err());
        // A different title suffixes instead.
        let plan = plan_default_target(
            &config(&info_temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &title_identity("Something Else"),
        )
        .expect("different title suffixes");
        assert_eq!(plan.stem, "report_2");
    }

    #[test]
    fn default_target_url_identity_matches_intake_or_ref_note() {
        use super::super::sources::RecordedSource;
        let temp = TempDir::new("url-identity");
        let config = config(&temp.path);
        let intake = temp.path.join("xlib/blogs/post.pdf");
        fs::create_dir_all(intake.parent().expect("parent"))
            .expect("create parent");
        fs::write(&intake, b"existing").expect("write intake");
        let recorded = vec![RecordedSource {
            dedupe_key: "https://example.com/post".to_string(),
            path: intake.clone(),
            is_ref_note: false,
            source_pdf: None,
            has_audio: false,
        }];
        let identity = DefaultTargetIdentity::Url {
            keys: vec!["https://example.com/post".to_string()],
            recorded: &recorded,
        };
        // Same URL at the base refuses without force.
        assert!(plan_default_target(
            &config,
            OsStr::new("post"),
            "blogs",
            false,
            &identity,
        )
        .is_err());
        // A different URL suffixes.
        let other = DefaultTargetIdentity::Url {
            keys: vec!["https://example.com/other".to_string()],
            recorded: &recorded,
        };
        let plan = plan_default_target(
            &config,
            OsStr::new("post"),
            "blogs",
            false,
            &other,
        )
        .expect("different URL suffixes");
        assert_eq!(plan.stem, "post_2");
    }

    #[test]
    fn default_target_cap_reports_no_free_name() {
        let temp = TempDir::new("cap");
        for index in 1..=999 {
            let stem = if index == 1 {
                "report".to_string()
            } else {
                format!("report_{index}")
            };
            let occupant = temp.path.join(format!("xlib/chat/{stem}.pdf"));
            fs::create_dir_all(occupant.parent().expect("parent"))
                .expect("create parent");
            fs::write(&occupant, b"x").expect("write occupant");
        }
        let error = plan_default_target(
            &config(&temp.path),
            OsStr::new("report"),
            "chat",
            false,
            &always_different(),
        )
        .expect_err("cap must fail");
        assert!(
            error
                .to_string()
                .contains("no free default filename for report"),
            "{error}"
        );
    }

    #[test]
    fn exact_output_keeps_nested_path_and_filename() {
        let temp = TempDir::new("exact-nested");
        let output = temp.path.join("xlib/books/deep/custom-name.pdf");

        let plan = plan_exact_output(&config(&temp.path), &output, false)
            .expect("plan");

        assert_eq!(plan.target, output);
        assert_eq!(
            plan.sidecar,
            temp.path.join("xlib/books/deep/custom-name.md")
        );
        assert_eq!(
            intake_destination(&plan),
            temp.path.join("lib/books/deep/custom-name.pdf")
        );
    }

    #[test]
    fn exact_output_accepts_uppercase_pdf_extension() {
        let temp = TempDir::new("exact-uppercase");
        let output = temp.path.join("out/Report.PDF");

        let plan = plan_exact_output(&config(&temp.path), &output, false)
            .expect("plan");

        assert_eq!(plan.target, output);
        assert_eq!(plan.workflow, TargetWorkflow::External);
    }

    #[test]
    fn exact_output_resolves_relative_and_tilde_paths() {
        let cwd = Path::new("/tmp/bob-cli-create-cwd");
        assert_eq!(
            resolve_exact_output_path_from(Path::new("nested/out.pdf"), cwd),
            PathBuf::from("/tmp/bob-cli-create-cwd/nested/out.pdf")
        );
        assert_eq!(
            resolve_exact_output_path_from(Path::new("./a/../b.pdf"), cwd),
            PathBuf::from("/tmp/bob-cli-create-cwd/b.pdf")
        );
        assert_eq!(
            resolve_exact_output_path_from(Path::new("~/vault/out.pdf"), cwd),
            bob_env::home_dir().join("vault/out.pdf")
        );
        assert_eq!(
            resolve_exact_output_path_from(Path::new("~"), cwd),
            bob_env::home_dir()
        );
    }

    #[test]
    fn exact_output_rejects_non_pdf_paths() {
        let temp = TempDir::new("exact-invalid");
        let config = config(&temp.path);

        for output in [
            temp.path.join("out/report.txt"),
            temp.path.join("out/report.pdf.md"),
            temp.path.join("out"),
            PathBuf::new(),
        ] {
            let error = plan_exact_output(&config, &output, false)
                .expect_err("must reject non-PDF output");
            let message = error.to_string();
            assert!(
                message.contains(".pdf")
                    || message.contains("nonempty filename"),
                "output {}: {message}",
                output.display()
            );
        }
    }

    #[test]
    fn exact_output_does_not_treat_sibling_prefix_as_managed() {
        let temp = TempDir::new("exact-prefix");
        let output = temp.path.join("xlib-extra/report.pdf");

        let plan = plan_exact_output(&config(&temp.path), &output, false)
            .expect("plan");

        assert_eq!(plan.target, output);
        assert_eq!(plan.workflow, TargetWorkflow::External);
    }

    #[test]
    fn exact_output_classifies_direct_library_target() {
        let temp = TempDir::new("exact-library");
        let output = temp.path.join("lib/chat/report.pdf");

        let plan = plan_exact_output(&config(&temp.path), &output, false)
            .expect("plan");

        assert_eq!(plan.target, output);
        assert_eq!(plan.workflow, TargetWorkflow::Library);
    }

    #[test]
    fn exact_library_target_requires_force_and_skips_mirrored_check() {
        let temp = TempDir::new("exact-library-force");
        let output = temp.path.join("lib/chat/report.pdf");
        fs::create_dir_all(output.parent().expect("output parent"))
            .expect("create library parent");
        fs::write(&output, b"existing").expect("write library pdf");

        let error = plan_exact_output(&config(&temp.path), &output, false)
            .expect_err("must require force");
        assert!(error.to_string().contains("--force"), "{error}");

        let plan = plan_exact_output(&config(&temp.path), &output, true)
            .expect("forced");
        assert_eq!(plan.workflow, TargetWorkflow::Library);
    }

    #[test]
    fn exact_intake_still_refuses_mirrored_library_pdf_with_force() {
        let temp = TempDir::new("exact-intake-library");
        let output = temp.path.join("xlib/papers/deep/report.pdf");
        let library_destination = temp.path.join("lib/papers/deep/report.pdf");
        fs::create_dir_all(
            library_destination
                .parent()
                .expect("library destination parent"),
        )
        .expect("create library destination parent");
        fs::write(&library_destination, b"existing")
            .expect("write library destination");

        let error = plan_exact_output(&config(&temp.path), &output, true)
            .expect_err("must refuse archived library destination");
        let message = error.to_string();
        assert!(
            message.contains("library destination already exists"),
            "{message}"
        );
        assert!(message.contains("xlib/papers/deep/report.pdf"), "{message}");
        assert!(message.contains("lib/papers/deep/report.pdf"), "{message}");
    }

    #[test]
    fn exact_intake_refuses_mirrored_library_sidecar() {
        let temp = TempDir::new("exact-intake-sidecar");
        let output = temp.path.join("xlib/papers/deep/report.pdf");
        let library_sidecar = temp.path.join("lib/papers/deep/report.md");
        fs::create_dir_all(library_sidecar.parent().expect("sidecar parent"))
            .expect("create sidecar parent");
        fs::write(&library_sidecar, "# Sidecar\n").expect("write sidecar");

        let error = plan_exact_output(&config(&temp.path), &output, false)
            .expect_err("must refuse archived library sidecar");
        let message = error.to_string();
        assert!(message.contains("library destination sidecar"), "{message}");
        assert!(message.contains("lib/papers/deep/report.md"), "{message}");
    }

    #[test]
    fn exact_output_refuses_same_stem_markdown_sidecar_even_with_force() {
        let temp = TempDir::new("exact-sidecar");
        let output = temp.path.join("out/custom.pdf");
        let sidecar = temp.path.join("out/custom.md");
        fs::create_dir_all(sidecar.parent().expect("sidecar parent"))
            .expect("create sidecar parent");
        fs::write(&sidecar, "# Sidecar\n").expect("write sidecar");

        let error = plan_exact_output(&config(&temp.path), &output, true)
            .expect_err("must refuse sidecar collision");
        assert!(error.to_string().contains("sidecar"), "{error}");
    }

    #[test]
    fn exact_external_target_does_not_invent_library_destination() {
        let temp = TempDir::new("exact-external");
        let output = temp.path.join("outside/custom.pdf");
        let unrelated_library = temp.path.join("lib/chat/report.pdf");
        fs::create_dir_all(unrelated_library.parent().expect("library parent"))
            .expect("create library parent");
        fs::write(&unrelated_library, b"existing")
            .expect("write unrelated library pdf");

        let plan = plan_exact_output(&config(&temp.path), &output, false)
            .expect("plan");

        assert_eq!(plan.target, output);
        assert_eq!(plan.workflow, TargetWorkflow::External);
    }

    #[test]
    fn normalize_lexically_drops_dot_and_parent_components() {
        assert_eq!(
            normalize_lexically(Path::new("/vault/xlib/../lib/a.pdf")),
            PathBuf::from("/vault/lib/a.pdf")
        );
        assert_eq!(
            normalize_lexically(Path::new("/vault/./xlib/chat/./a.pdf")),
            PathBuf::from("/vault/xlib/chat/a.pdf")
        );
        assert_eq!(
            normalize_lexically(Path::new("/../a.pdf")),
            PathBuf::from("/a.pdf")
        );
    }

    #[test]
    fn marker_rejects_wikilink_parent_and_unknown_status() {
        assert!(compose_marker(
            "ready",
            "[[obsidian_ref]]",
            "Report",
            None,
            &[]
        )
        .is_err());
        assert!(
            compose_marker("unknown", "obsidian_ref", "Report", None, &[])
                .is_err()
        );
    }

    #[test]
    fn marker_extras_insert_in_canonical_order_and_skip_empty() {
        let marker = compose_marker(
            "ready",
            "obsidian_ref",
            "Clipped Article",
            Some("clipped_article"),
            &[
                ("captured", MarkerValue::String("2026-10-01".to_string())),
                (
                    "source_url",
                    MarkerValue::String(
                        "https://example.com/article".to_string(),
                    ),
                ),
                ("author", MarkerValue::String(String::new())),
                ("published", MarkerValue::String("2026-04-27".to_string())),
            ],
        )
        .expect("marker");
        assert_eq!(
            marker,
            "- status: ready\n\
             - parent: obsidian_ref\n\
             - title: Clipped Article\n\
             - id: clipped_article\n\
             - source_url: https://example.com/article\n\
             - published: 2026-04-27\n\
             - captured: 2026-10-01\n"
        );
        let parsed = parse_marker_with_normalization(&marker).expect("marker");
        assert_eq!(
            parsed.projection.get("source_url"),
            Some(&MarkerValue::String(
                "https://example.com/article".to_string()
            ))
        );
        assert!(!parsed.projection.contains_key("author"));
    }

    #[test]
    fn marker_return_links_key_orders_after_captured_and_round_trips() {
        let marker = compose_marker(
            "ready",
            "obsidian_ref",
            "Report",
            None,
            &[
                ("captured", MarkerValue::String("2026-10-01".to_string())),
                ("return_links", MarkerValue::Bool(true)),
            ],
        )
        .expect("marker");
        assert_eq!(
            marker,
            "- status: ready\n\
             - parent: obsidian_ref\n\
             - title: Report\n\
             - captured: 2026-10-01\n\
             - return_links: true\n"
        );
        let parsed = parse_marker_with_normalization(&marker).expect("marker");
        assert_eq!(
            parsed.projection.get("return_links"),
            Some(&MarkerValue::Bool(true))
        );
        assert!(is_standard_user_field("return_links"));
        assert!(unknown_synced_fields(&parsed.projection).is_empty());
        assert_eq!(
            MarkerValue::Bool(true).as_frontmatter_value(),
            "true".to_string()
        );
    }

    #[test]
    fn stamp_install_embeds_marker_and_info_without_annots() {
        let temp = TempDir::new("stamp-install");
        let rendered = temp.path.join("render.pdf");
        write_minimal_pdf(&rendered, false);
        let target = temp.path.join("nested/dir/out.pdf");
        let marker = compose_marker(
            "ready",
            "obsidian_ref",
            "Clipped Article",
            Some("clipped_article"),
            &[
                (
                    "source_url",
                    MarkerValue::String(
                        "https://example.com/article".to_string(),
                    ),
                ),
                ("author", MarkerValue::String("Jane Doe".to_string())),
                ("published", MarkerValue::String("2026-04-27".to_string())),
                ("captured", MarkerValue::String("2026-10-01".to_string())),
            ],
        )
        .expect("marker");

        let pages = stamp_and_install(
            &rendered,
            &target,
            &marker,
            &PdfInfo {
                title: Some("Clipped Article".to_string()),
                author: Some("Jane Doe".to_string()),
            },
        )
        .expect("stamp and install");

        assert_eq!(pages, 1);
        assert_eq!(annotation_count(&target), 1);
        let stored = read_pdf_marker(&target).expect("read marker");
        assert_eq!(stored.contents, marker);
        assert_eq!(
            info_string(&target, b"Title").as_deref(),
            Some("Clipped Article")
        );
        assert_eq!(
            info_string(&target, b"Author").as_deref(),
            Some("Jane Doe")
        );
    }

    #[test]
    fn stamp_install_appends_when_annots_exist_and_sets_partial_info() {
        let temp = TempDir::new("stamp-install-annots");
        let rendered = temp.path.join("render.pdf");
        write_minimal_pdf(&rendered, true);
        let target = temp.path.join("out.pdf");
        let marker =
            compose_marker("ready", "obsidian_ref", "Title", None, &[])
                .expect("marker");

        let pages = stamp_and_install(
            &rendered,
            &target,
            &marker,
            &PdfInfo {
                title: Some("Title".to_string()),
                author: None,
            },
        )
        .expect("stamp and install");

        assert_eq!(pages, 1);
        assert_eq!(annotation_count(&target), 2);
        assert_eq!(info_string(&target, b"Title").as_deref(), Some("Title"));
        assert_eq!(info_string(&target, b"Author"), None);
    }

    #[test]
    fn stamp_install_without_info_leaves_trailer_info_absent() {
        let temp = TempDir::new("stamp-install-no-info");
        let rendered = temp.path.join("render.pdf");
        write_minimal_pdf(&rendered, false);
        let target = temp.path.join("out.pdf");
        let marker =
            compose_marker("ready", "obsidian_ref", "Title", None, &[])
                .expect("marker");

        stamp_and_install(&rendered, &target, &marker, &PdfInfo::default())
            .expect("stamp and install");

        let document = Document::load(&target).expect("load stamped PDF");
        assert!(document.trailer.get(b"Info").is_err());
    }
}

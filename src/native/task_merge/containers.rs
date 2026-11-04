use std::path::PathBuf;

use super::{
    super::{
        capture, capture_drawers, capture_language, capture_project_note,
        projects,
        task_locator::{self, LocatedTask, TaskLocator},
        task_move::{transplant, VaultSnapshot},
        vault_links::LinkResolution,
    },
    types::{
        locator_text, MergeContainer, MergeContext, MergeError,
        PreparedContainer, PreparedContainerKind,
    },
};

pub(super) struct ResolvedDestination {
    pub(super) container: PreparedContainer,
    pub(super) contents: String,
    pub(super) title_indent: String,
    pub(super) child_indent: String,
    pub(super) child_unit: String,
    pub(super) warning: Option<String>,
}

pub(super) fn resolve_destination(
    snapshot: &VaultSnapshot,
    container: &MergeContainer,
    context: &MergeContext<'_>,
) -> Result<ResolvedDestination, MergeError> {
    match container {
        MergeContainer::Task(locator) => {
            let target = resolve_task(snapshot, locator, context)?;
            task_container(snapshot, target)
        }
        MergeContainer::Drawer(locator, selector) => {
            let target = resolve_task(snapshot, locator, context)?;
            if !target.task.status_type.is_open() {
                return Err(MergeError::new(
                    "merge_target_closed",
                    format!("^{} is closed", locator.block_id),
                ));
            }
            drawer_container(snapshot, target, selector, context)
        }
        MergeContainer::Section { note, selector } => {
            section_container(snapshot, note, selector, context)
        }
    }
}

pub(super) fn resolve_task(
    snapshot: &VaultSnapshot,
    locator: &TaskLocator,
    context: &MergeContext<'_>,
) -> Result<LocatedTask, MergeError> {
    task_locator::resolve_task(
        &snapshot.notes,
        &locator_text(locator),
        context.task_settings,
    )
    .map_err(|error| MergeError::new(error.code, error.message))
}

fn task_container(
    snapshot: &VaultSnapshot,
    target: LocatedTask,
) -> Result<ResolvedDestination, MergeError> {
    if !target.task.status_type.is_open() {
        return Err(MergeError::new(
            "merge_target_closed",
            format!(
                "^{} is closed",
                target.task.block_id.as_deref().unwrap_or("?")
            ),
        ));
    }
    let contents = note_contents(snapshot, &target.path)?;
    let lines = capture::line_spans(contents);
    let title_indent = capture::first_child_indentation(
        &lines,
        target.task.line_index,
        target.task.block_end,
        &target.task.indentation,
    )
    .unwrap_or_else(|| {
        format!(
            "{}{}",
            target.task.indentation,
            capture::dominant_indent_unit(&lines).unwrap_or("\t")
        )
    });
    let child_unit = title_indent
        .strip_prefix(&target.task.indentation)
        .filter(|unit| !unit.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            capture::dominant_indent_unit(&lines)
                .unwrap_or("\t")
                .to_string()
        });
    let insertion_offset = capture::first_direct_managed_log_start(
        &lines,
        target.task.line_index,
        target.task.block_end,
    )
    .unwrap_or(target.task.block_end);
    let container = PreparedContainer {
        path: target.path.clone(),
        kind: PreparedContainerKind::Task,
        block_id: target.task.block_id.clone(),
        title: Some(target.task.description.clone()),
        insertion_offset,
        empty_heading: false,
    };
    Ok(ResolvedDestination {
        container,
        contents: contents.to_string(),
        child_indent: format!("{title_indent}{child_unit}"),
        title_indent,
        child_unit,
        warning: None,
    })
}

fn drawer_container(
    snapshot: &VaultSnapshot,
    target: LocatedTask,
    selector: &capture_language::DrawerSelector,
    context: &MergeContext<'_>,
) -> Result<ResolvedDestination, MergeError> {
    let contents = note_contents(snapshot, &target.path)?;
    let route = target
        .path
        .with_extension("")
        .to_string_lossy()
        .into_owned();
    let (resolved, scope) =
        if capture_drawers::is_prj_parent(contents, &target.task) {
            let drawers = capture_drawers::project_h2_drawers(contents);
            let resolved = capture_drawers::resolve_h2_for_write(
                &drawers,
                &selector.text,
                selector.exact,
                context.drawer_catalog,
                &route,
                target.task.block_id.as_deref(),
            )
            .map_err(|error| MergeError::new(error.code, error.message))?;
            (resolved, capture_drawers::DrawerScope::ProjectSection)
        } else {
            let drawers = capture_drawers::drawers(contents, &target.task);
            let resolved = capture_drawers::resolve_drawer_for_write(
                &drawers,
                &selector.text,
                selector.exact,
                context.drawer_catalog,
                &route,
                target.task.block_id.as_deref(),
                capture_drawers::DrawerScope::Task,
            )
            .map_err(|error| MergeError::new(error.code, error.message))?;
            (resolved, capture_drawers::DrawerScope::Task)
        };
    let warning = resolved.warning.clone();
    let (contents, drawer, created) =
        capture_drawers::ensure_drawer(contents, &target.task, &resolved)
            .map_err(|message| {
                MergeError::new("merge_container_failed", message)
            })?;

    let (
        title_indent,
        child_indent,
        child_unit,
        insertion_offset,
        empty_heading,
    ) = if scope == capture_drawers::DrawerScope::ProjectSection {
        let (offset, empty) =
            capture_drawers::h2_insert_position(&contents, &drawer);
        let unit =
            capture::dominant_indent_unit(&capture::line_spans(&contents))
                .unwrap_or("\t")
                .to_string();
        (String::new(), unit.clone(), unit, offset, empty)
    } else {
        let insertion = capture_drawers::drawer_insertion(&contents, &drawer);
        let unit = insertion
            .indentation
            .strip_prefix(&drawer.indentation)
            .filter(|unit| !unit.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                capture::dominant_indent_unit(&capture::line_spans(&contents))
                    .unwrap_or("\t")
                    .to_string()
            });
        (
            insertion.indentation.clone(),
            format!("{}{unit}", insertion.indentation),
            unit,
            insertion.offset,
            false,
        )
    };
    let container = PreparedContainer {
        path: target.path.clone(),
        kind: PreparedContainerKind::Drawer {
            title: drawer.title,
            created,
        },
        block_id: target.task.block_id,
        title: Some(target.task.description),
        insertion_offset,
        empty_heading,
    };
    Ok(ResolvedDestination {
        container,
        contents,
        title_indent,
        child_indent,
        child_unit,
        warning,
    })
}

fn section_container(
    snapshot: &VaultSnapshot,
    note: &str,
    selector: &capture_language::DrawerSelector,
    context: &MergeContext<'_>,
) -> Result<ResolvedDestination, MergeError> {
    let index = context
        .note_index
        .with_additional(snapshot.notes.keys().cloned());
    let path = match index.resolve_detailed(None, note) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous => {
            return Err(MergeError::new(
                "ambiguous_note",
                format!("`{note}` matches more than one note"),
            ));
        }
        LinkResolution::Missing => {
            return Err(MergeError::new(
                "task_not_found",
                format!("no note matches `{note}`"),
            ));
        }
    };
    let original = note_contents(snapshot, &path)?;
    let frontmatter =
        projects::parse_frontmatter(original).ok_or_else(|| {
            MergeError::new(
                "merge_section_reserved",
                "sections can only be merged into area or project notes",
            )
        })?;
    if !projects::frontmatter_is_area(&frontmatter)
        && !projects::frontmatter_is_project(&frontmatter)
    {
        return Err(MergeError::new(
            "merge_section_reserved",
            "sections can only be merged into area or project notes",
        ));
    }

    let drawers = capture_drawers::project_h2_drawers(original);
    let title = resolve_section_title(&drawers, selector)?;
    let mut contents = original.to_string();
    let mut created = false;
    let drawer = if let Some(drawer) = title.existing {
        drawer
    } else {
        created = true;
        contents = append_section(original, &title.title);
        capture_drawers::project_h2_drawers(&contents)
            .into_iter()
            .find(|drawer| drawer.title == title.title)
            .ok_or_else(|| {
                MergeError::new(
                    "merge_container_failed",
                    format!("failed to create section ## {}", title.title),
                )
            })?
    };
    let (insertion_offset, empty_heading) =
        capture_drawers::h2_insert_position(&contents, &drawer);
    let unit = capture::dominant_indent_unit(&capture::line_spans(&contents))
        .unwrap_or("\t")
        .to_string();
    Ok(ResolvedDestination {
        container: PreparedContainer {
            path,
            kind: PreparedContainerKind::Section {
                title: drawer.title.clone(),
                created,
            },
            block_id: None,
            title: Some(drawer.title),
            insertion_offset,
            empty_heading,
        },
        contents,
        title_indent: String::new(),
        child_indent: unit.clone(),
        child_unit: unit,
        warning: None,
    })
}

#[derive(Debug)]
struct SectionTitle {
    title: String,
    existing: Option<capture_drawers::Drawer>,
}

fn resolve_section_title(
    drawers: &[capture_drawers::Drawer],
    selector: &capture_language::DrawerSelector,
) -> Result<SectionTitle, MergeError> {
    let key = capture_drawers::drawer_key(&selector.text);
    if capture_drawers::is_reserved_drawer_key(&key)
        || capture_drawers::is_structural_h2_key(&key)
    {
        return Err(MergeError::new(
            "merge_section_reserved",
            format!("`{}` is a reserved section heading", selector.text),
        ));
    }
    if selector.text.trim().is_empty() || selector.text.contains(['\n', '\r']) {
        return Err(MergeError::new(
            "merge_section_reserved",
            "section title must be a nonempty single line",
        ));
    }

    let matches: Vec<&capture_drawers::Drawer> = if selector.exact {
        drawers
            .iter()
            .filter(|drawer| drawer.title.eq_ignore_ascii_case(&selector.text))
            .collect()
    } else {
        let slug = capture_language::selector_slug(&selector.text);
        let plain_key = selector.text.to_ascii_uppercase();
        let spaced_key = capture_language::normalize_task_text(
            &selector.text.replace('-', " "),
        )
        .to_ascii_uppercase();
        drawers
            .iter()
            .filter(|drawer| {
                drawer.slug.eq_ignore_ascii_case(&slug)
                    || drawer.key == plain_key
                    || drawer.key == spaced_key
            })
            .collect()
    };
    match matches.as_slice() {
        [drawer] => {
            return Ok(SectionTitle {
                title: drawer.title.clone(),
                existing: Some((*drawer).clone()),
            });
        }
        [_, _, ..] => {
            return Err(MergeError::new(
                "ambiguous_section",
                format!(
                    "section `{}` matches more than one heading",
                    selector.text
                ),
            ));
        }
        [] => {}
    }

    if !selector.exact
        && let Some(near) =
            capture_drawers::find_near_duplicate_drawer(drawers, &selector.text)
    {
        return Err(MergeError::new(
            "merge_section_near_duplicate",
            format!("did you mean `## {}`?", near.title),
        ));
    }

    let title = if selector.exact {
        selector.text.clone()
    } else {
        let normalized = capture_language::normalize_task_text(
            &selector.text.replace('-', " "),
        );
        capture_project_note::format_project_section_title(&normalized)
    };
    let new_key = capture_drawers::drawer_key(&title);
    if capture_drawers::is_reserved_drawer_key(&new_key)
        || capture_drawers::is_structural_h2_key(&new_key)
    {
        return Err(MergeError::new(
            "merge_section_reserved",
            format!("`{title}` is a reserved section heading"),
        ));
    }
    Ok(SectionTitle {
        title,
        existing: None,
    })
}

fn append_section(contents: &str, title: &str) -> String {
    let ending = transplant::document_line_ending(contents);
    let double = format!("{ending}{ending}");
    let separator = if contents.is_empty() || contents.ends_with(&double) {
        String::new()
    } else if contents.ends_with(ending) {
        ending.to_string()
    } else {
        double
    };
    format!("{contents}{separator}## {title}{ending}")
}

fn note_contents<'a>(
    snapshot: &'a VaultSnapshot,
    path: &PathBuf,
) -> Result<&'a str, MergeError> {
    snapshot.notes.get(path).map(String::as_str).ok_or_else(|| {
        MergeError::new(
            "task_not_found",
            format!("no note matches `{}`", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_slug_matching_and_creation_are_strict() {
        let existing = capture_drawers::project_h2_drawers(
            "---\ntype: [[area]]\n---\n## Future Work\n- Existing\n",
        );
        assert_eq!(
            resolve_section_title(
                &existing,
                &capture_language::DrawerSelector {
                    text: "future-work".to_string(),
                    exact: false,
                },
            )
            .expect("exact slug")
            .title,
            "Future Work"
        );
        assert_eq!(
            resolve_section_title(
                &existing,
                &capture_language::DrawerSelector {
                    text: "future".to_string(),
                    exact: false,
                },
            )
            .expect_err("strict prefix")
            .code,
            "merge_section_near_duplicate"
        );
        assert_eq!(
            resolve_section_title(
                &existing,
                &capture_language::DrawerSelector {
                    text: "Exact Title".to_string(),
                    exact: true,
                },
            )
            .expect("literal create")
            .title,
            "Exact Title"
        );
    }
}

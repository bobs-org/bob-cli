use super::super::projects::{
    frontmatter_is_area, frontmatter_is_project, frontmatter_scalar,
    frontmatter_value, parse_frontmatter, trim_yaml_scalar,
    wikilink_path_target, FrontmatterScalar,
};
use super::super::task_fields::canonical_theme_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteKind {
    Area,
    Project,
    Other,
}

/// A `theme:` frontmatter value on a note. Absent/blank inherit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteTheme {
    Name(String),
    Off,
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteFacts {
    pub(crate) kind: NoteKind,
    pub(crate) parent: Option<String>,
    pub(crate) color: Option<String>,
    pub(crate) root: bool,
    pub(crate) theme: Option<NoteTheme>,
}

impl NoteFacts {
    pub(crate) fn parse(contents: &str) -> Self {
        let frontmatter = parse_frontmatter(contents);
        let kind =
            frontmatter.as_ref().map_or(NoteKind::Other, |frontmatter| {
                if frontmatter_is_area(frontmatter) {
                    NoteKind::Area
                } else if frontmatter_is_project(frontmatter) {
                    NoteKind::Project
                } else {
                    NoteKind::Other
                }
            });
        let parent = frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter_value(frontmatter, "parent"))
            .and_then(wikilink_path_target);
        let color = frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter_value(frontmatter, "color"))
            .map(|value| trim_yaml_scalar(value).to_string());
        let root = frontmatter
            .as_ref()
            .and_then(|frontmatter| frontmatter_value(frontmatter, "root"))
            .is_some_and(|value| {
                trim_yaml_scalar(value).eq_ignore_ascii_case("true")
            });
        let theme = frontmatter.as_ref().and_then(parse_note_theme);
        Self {
            kind,
            parent,
            color,
            root,
            theme,
        }
    }
}

fn parse_note_theme(
    frontmatter: &super::super::projects::Frontmatter<'_>,
) -> Option<NoteTheme> {
    match frontmatter_scalar(frontmatter, "theme") {
        FrontmatterScalar::Absent | FrontmatterScalar::Blank => None,
        FrontmatterScalar::NonScalar(raw) => Some(NoteTheme::Invalid(raw)),
        FrontmatterScalar::Value(raw) => {
            if raw.eq_ignore_ascii_case("off") {
                Some(NoteTheme::Off)
            } else {
                match canonical_theme_name(&raw) {
                    Some(name) => Some(NoteTheme::Name(name)),
                    None => Some(NoteTheme::Invalid(raw)),
                }
            }
        }
    }
}

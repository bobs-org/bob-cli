use super::super::projects::{
    frontmatter_is_area, frontmatter_is_project, frontmatter_value,
    parse_frontmatter, trim_yaml_scalar, wikilink_path_target,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteKind {
    Area,
    Project,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteFacts {
    pub(crate) kind: NoteKind,
    pub(crate) parent: Option<String>,
    pub(crate) color: Option<String>,
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
        Self {
            kind,
            parent,
            color,
        }
    }
}

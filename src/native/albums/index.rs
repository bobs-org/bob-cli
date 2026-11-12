//! Pure records-index renderer and host_catalog region planner.
//!
//! Rendering is a function of one album declaration plus already-grouped
//! folder members. Region planning patches the owned `[!records]` callout
//! without scanning the vault again.

use std::path::PathBuf;

use super::{
    grouping::{display_value, GroupingRecord, RecordGroup, RecordGrouping},
    output::{lowercase_rest, split_icon},
    AlbumFinding, AlbumIssueSeverity,
};
use crate::native::{
    host_catalog::{
        self, canonical_callout, FileEdit, HostOutcome, Placement, RegionSpec,
    },
    notebooks::catalog::inert_text,
    records::{
        schema::{FieldKind, RecordField},
        IndexedFieldValue,
    },
};

const ALIAS_DROP: &[char] = &['|', '[', ']', '#', '^'];

/// Documented JSON/report states for one album's records index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexState {
    Current,
    Missing,
    Stale,
    Damaged,
}

impl IndexState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Damaged => "damaged",
        }
    }
}

/// Shared plan consumed by reports, later record writes, and albums sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexPlan {
    pub(crate) state: IndexState,
    pub(crate) callout: Vec<String>,
    pub(crate) edit: Option<FileEdit>,
    pub(crate) findings: Vec<AlbumFinding>,
}

/// `[!records]` region: heading `Records`, first write before the first H2.
pub(crate) fn records_spec(head_title: &str) -> RegionSpec<'_> {
    RegionSpec {
        callout: "records",
        heading: "Records",
        head_title: Some(head_title),
        placement: Placement::BeforeFirstH2,
        legacy: None,
        strict_bare: false,
    }
}

/// Borrowed display nouns for the records index tally.
pub(crate) struct RecordNouns<'a> {
    pub(crate) singular: &'a str,
    pub(crate) plural: &'a str,
}

/// Render the owned callout lines, including the `[!records]` head.
pub(crate) fn render_callout(
    album_stem: &str,
    nouns: &RecordNouns<'_>,
    fields: &[RecordField],
    grouping: &RecordGrouping,
    index_by: Option<&str>,
    total: usize,
) -> Vec<String> {
    let tally = tally_text(nouns, grouping, total);
    let spec = records_spec(&tally);
    let mut rows = vec![">".to_string()];
    if total == 0 {
        rows.push(format!("> Add one with `Name @{album_stem}/`."));
        return canonical_callout(&spec, &rows);
    }
    if grouping.group_field.is_none() {
        for record in &grouping.ungrouped {
            rows.push(record_row(
                0,
                album_stem,
                record,
                &row_facets(record, fields, grouping, index_by, false),
            ));
        }
        return canonical_callout(&spec, &rows);
    }
    for group in &grouping.groups {
        rows.extend(render_group(
            album_stem, fields, grouping, index_by, group,
        ));
    }
    if let Some(needs) = &grouping.needs {
        rows.extend(render_group(
            album_stem, fields, grouping, index_by, needs,
        ));
    }
    canonical_callout(&spec, &rows)
}

/// Plan the records region for one album from already-loaded text.
pub(crate) fn plan_index(
    path: &str,
    contents: Option<&str>,
    album_stem: &str,
    nouns: &RecordNouns<'_>,
    fields: &[RecordField],
    grouping: &RecordGrouping,
    index_by: Option<&str>,
    total: usize,
    folder_complete: bool,
) -> IndexPlan {
    let callout =
        render_callout(album_stem, nouns, fields, grouping, index_by, total);
    if !folder_complete {
        return IndexPlan {
            state: IndexState::Damaged,
            callout,
            edit: None,
            findings: vec![index_error(
                path,
                None,
                format!(
                    "{path}: record folder could not be read completely; the records index was left unchanged"
                ),
            )],
        };
    }
    let Some(text) = contents else {
        return IndexPlan {
            state: IndexState::Damaged,
            callout,
            edit: None,
            findings: vec![index_error(
                path,
                None,
                format!(
                    "{path}: album text was not retained for index planning"
                ),
            )],
        };
    };
    let tally = tally_text(nouns, grouping, total);
    let spec = records_spec(&tally);
    let planned = host_catalog::plan_host_region(
        PathBuf::from(path),
        text,
        &spec,
        &callout,
    );
    match planned.outcome {
        HostOutcome::Nothing => IndexPlan {
            state: IndexState::Damaged,
            callout,
            edit: None,
            findings: vec![index_error(
                path,
                None,
                format!("{path}: records index planning produced no region"),
            )],
        },
        HostOutcome::Current => IndexPlan {
            state: IndexState::Current,
            callout,
            edit: None,
            findings: Vec::new(),
        },
        HostOutcome::Update { state, after: _ } => {
            let index_state = match state {
                host_catalog::EditState::Missing => IndexState::Missing,
                host_catalog::EditState::Stale => IndexState::Stale,
            };
            IndexPlan {
                state: index_state,
                callout,
                edit: planned.edit,
                findings: vec![index_stale(path, index_state)],
            }
        }
        HostOutcome::Damaged(damage) => IndexPlan {
            state: IndexState::Damaged,
            callout,
            edit: None,
            findings: vec![index_error(
                path,
                Some(damage.line),
                format!("{path}: {}", damage.message),
            )],
        },
    }
}

fn render_group(
    album_stem: &str,
    fields: &[RecordField],
    grouping: &RecordGrouping,
    index_by: Option<&str>,
    group: &RecordGroup,
) -> Vec<String> {
    let mut lines = vec![heading_row(0, &group.label, group.count)];
    let grouped = !group.subgroups.is_empty();
    if grouped {
        for subgroup in &group.subgroups {
            lines.push(heading_row(
                1,
                &subgroup.display,
                subgroup.members.len(),
            ));
            for record in &subgroup.members {
                lines.push(record_row(
                    2,
                    album_stem,
                    record,
                    &row_facets(record, fields, grouping, index_by, true),
                ));
            }
        }
    } else {
        for record in &group.members {
            lines.push(record_row(
                1,
                album_stem,
                record,
                &row_facets(record, fields, grouping, index_by, false),
            ));
        }
    }
    lines
}

fn heading_row(indent: usize, label: &str, count: usize) -> String {
    let pad = "  ".repeat(indent);
    format!("> {pad}- **{}** · {count}", inert_text(label))
}

fn record_row(
    indent: usize,
    album_stem: &str,
    record: &GroupingRecord,
    facets: &[String],
) -> String {
    let pad = "  ".repeat(indent);
    let target = format!("{album_stem}/{}", record.stem);
    let alias = alias_text(&record.title);
    let mut row = format!("> {pad}- [[{target}|{alias}]]");
    if !facets.is_empty() {
        row.push_str(" — ");
        row.push_str(&facets.join(" · "));
    }
    row
}

fn alias_text(title: &str) -> String {
    let stripped: String = title
        .chars()
        .filter(|ch| !ALIAS_DROP.contains(ch))
        .collect();
    inert_text(&stripped)
}

fn row_facets(
    record: &GroupingRecord,
    fields: &[RecordField],
    grouping: &RecordGrouping,
    index_by: Option<&str>,
    grouped: bool,
) -> Vec<String> {
    grouping
        .index_facets
        .iter()
        .filter_map(|key| {
            if grouped
                && !grouping.explicit_facets
                && index_by == Some(key.as_str())
            {
                return None;
            }
            let field = fields.iter().find(|field| field.key == *key)?;
            let text = format_facet(record, field)?;
            let escaped = inert_text(&text);
            if escaped.is_empty() {
                None
            } else {
                Some(escaped)
            }
        })
        .collect()
}

fn format_facet(
    record: &GroupingRecord,
    field: &RecordField,
) -> Option<String> {
    let value = record
        .fields
        .iter()
        .find(|(key, _)| key == &field.key)
        .map(|(_, value)| value)?;
    match value {
        IndexedFieldValue::Null | IndexedFieldValue::Unsupported => None,
        IndexedFieldValue::Scalar(text) if text.trim().is_empty() => None,
        IndexedFieldValue::Scalar(text) => Some(format_scalar(field, text)),
        IndexedFieldValue::List(items) => {
            let parts: Vec<String> = items
                .iter()
                .filter(|item| !item.trim().is_empty())
                .map(|item| format_scalar(field, item))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(", "))
            }
        }
    }
}

fn format_scalar(field: &RecordField, raw: &str) -> String {
    match &field.kind {
        FieldKind::Choices(choices) => choices
            .iter()
            .find(|choice| choice.value == raw.trim())
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| display_value(raw)),
        FieldKind::Link => display_value(raw),
        FieldKind::Number => raw.trim().to_owned(),
        _ => display_value(raw),
    }
}

fn tally_text(
    nouns: &RecordNouns<'_>,
    grouping: &RecordGrouping,
    total: usize,
) -> String {
    let safe_plural = inert_text(nouns.plural);
    let safe_singular = inert_text(nouns.singular);
    if total == 0 {
        return format!("No {safe_plural} yet");
    }
    if total == 1 {
        let mut parts = vec![format!("1 {safe_singular}")];
        for group in &grouping.groups {
            parts.push(tally_fragment(&group.label, group.count));
        }
        if grouping.needs_status > 0 {
            parts.push(needs_status_fragment(grouping.needs_status));
        }
        return parts.join(" · ");
    }
    let mut parts = vec![format!("{total} {safe_plural}")];
    for group in &grouping.groups {
        parts.push(tally_fragment(&group.label, group.count));
    }
    if grouping.needs_status > 0 {
        parts.push(needs_status_fragment(grouping.needs_status));
    }
    parts.join(" · ")
}

fn needs_status_fragment(count: usize) -> String {
    if count == 1 {
        "⚠️ 1 needs a status".to_owned()
    } else {
        format!("⚠️ {count} need a status")
    }
}

fn tally_fragment(label: &str, count: usize) -> String {
    let (icon, rest) = split_icon(label);
    let text = inert_text(&lowercase_rest(rest));
    match icon {
        Some(icon) if !text.is_empty() => format!("{icon} {count} {text}"),
        Some(icon) => format!("{icon} {count}"),
        None if !text.is_empty() => format!("{count} {text}"),
        None => count.to_string(),
    }
}

fn index_stale(path: &str, state: IndexState) -> AlbumFinding {
    AlbumFinding {
        code: "album_index_stale",
        severity: AlbumIssueSeverity::Warning,
        path: path.to_owned(),
        line_number: None,
        key_path: None,
        message: format!(
            "the generated records index in {path} is {}; run `bob albums sync`",
            state.as_str()
        ),
        suggestions: vec!["bob albums sync".to_owned()],
    }
}

fn index_error(
    path: &str,
    line: Option<usize>,
    message: String,
) -> AlbumFinding {
    AlbumFinding {
        code: "album_index_damaged",
        severity: AlbumIssueSeverity::Error,
        path: path.to_owned(),
        line_number: line,
        key_path: None,
        message,
        suggestions: Vec::new(),
    }
}

/// `index_by` actually supplies subgroup headings for this group.
#[cfg(test)]
fn group_is_large(count: usize) -> bool {
    count > super::grouping::LARGE_GROUP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::albums::grouping::group_records;
    use crate::native::records::schema::{RecordChoice, RecordField};

    fn choices(key: &str, values: &[(&str, &str)]) -> RecordField {
        RecordField {
            key: key.to_owned(),
            label: key.to_owned(),
            kind: FieldKind::Choices(
                values
                    .iter()
                    .map(|(value, label)| RecordChoice {
                        value: (*value).to_owned(),
                        label: (*label).to_owned(),
                    })
                    .collect(),
            ),
            required: false,
            list: false,
            default: None,
            suggest: false,
            min: None,
            max: None,
        }
    }

    fn text(key: &str, suggest: bool) -> RecordField {
        RecordField {
            key: key.to_owned(),
            label: key.to_owned(),
            kind: FieldKind::Text,
            required: false,
            list: false,
            default: None,
            suggest,
            min: None,
            max: None,
        }
    }

    fn number(key: &str) -> RecordField {
        RecordField {
            key: key.to_owned(),
            label: key.to_owned(),
            kind: FieldKind::Number,
            required: false,
            list: false,
            default: None,
            suggest: true,
            min: None,
            max: None,
        }
    }

    fn link(key: &str) -> RecordField {
        RecordField {
            key: key.to_owned(),
            label: key.to_owned(),
            kind: FieldKind::Link,
            required: false,
            list: false,
            default: None,
            suggest: true,
            min: None,
            max: None,
        }
    }

    fn record(
        path: &str,
        title: &str,
        fields: &[(&str, IndexedFieldValue)],
    ) -> GroupingRecord {
        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(path)
            .to_owned();
        GroupingRecord {
            path: path.to_owned(),
            stem,
            title: title.to_owned(),
            created: None,
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        }
    }

    fn scalar(value: &str) -> IndexedFieldValue {
        IndexedFieldValue::Scalar(value.to_owned())
    }

    fn join(lines: &[String]) -> String {
        lines.join("\n")
    }

    fn nouns<'a>(singular: &'a str, plural: &'a str) -> RecordNouns<'a> {
        RecordNouns { singular, plural }
    }

    fn eat_fields() -> Vec<RecordField> {
        vec![
            choices(
                "status",
                &[
                    ("to_try", "🔖 To try"),
                    ("liked", "💚 Liked"),
                    ("not_liked", "🚫 Not liked"),
                ],
            ),
            text("category", true),
            text("location", true),
        ]
    }

    #[test]
    fn empty_album_renders_the_teaching_index() {
        let fields = eat_fields();
        let grouping = group_records(&fields, Some("location"), &[], &[]);
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
        );
        assert_eq!(
            join(&lines),
            "> [!records] No restaurants yet\n>\n> Add one with `Name @eat/`."
        );
    }

    #[test]
    fn eat_shaped_index_is_byte_exact() {
        let fields = eat_fields();
        let records = vec![
            record(
                "eat/ani_ramen_house.md",
                "Ani Ramen House",
                &[
                    ("status", scalar("to_try")),
                    ("category", scalar("Ramen")),
                    ("location", scalar("Clark")),
                ],
            ),
            record(
                "eat/bubble_tea.md",
                "Bubble Tea",
                &[
                    ("status", scalar("to_try")),
                    ("category", scalar("Bubble Tea")),
                    ("location", scalar("Cranford")),
                ],
            ),
            record(
                "eat/cafe.md",
                "Cafe",
                &[
                    ("status", scalar("liked")),
                    ("category", scalar("Ramen")),
                    ("location", scalar("Clark")),
                ],
            ),
            record(
                "eat/unknown.md",
                "Unknown",
                &[
                    ("status", scalar("nope")),
                    ("category", scalar("Ramen")),
                    ("location", scalar("Clark")),
                ],
            ),
        ];
        let grouping = group_records(&fields, Some("location"), &[], &records);
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            records.len(),
        );
        assert_eq!(
            join(&lines),
            "\
> [!records] 4 restaurants · 🔖 2 to try · 💚 1 liked · ⚠️ 1 needs a status
>
> - **🔖 To try** · 2
>   - [[eat/ani_ramen_house|Ani Ramen House]] — Ramen · Clark
>   - [[eat/bubble_tea|Bubble Tea]] — Bubble Tea · Cranford
> - **💚 Liked** · 1
>   - [[eat/cafe|Cafe]] — Ramen · Clark
> - **⚠️ Needs a status** · 1
>   - [[eat/unknown|Unknown]] — Ramen · Clark"
        );
        assert_eq!(
            grouping.groups[0].peek,
            grouping.groups[0]
                .members
                .iter()
                .take(3)
                .map(|record| record.title.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn podcasts_shaped_index_uses_icon_free_labels() {
        let fields = vec![
            choices("status", &[("queued", "Queued"), ("done", "Done")]),
            text("topic", true),
        ];
        let records = vec![
            record(
                "podcasts/practical_ai.md",
                "Practical AI",
                &[("status", scalar("queued")), ("topic", scalar("ai"))],
            ),
            record(
                "podcasts/corepy.md",
                "Core.py",
                &[("status", scalar("done")), ("topic", scalar("python"))],
            ),
        ];
        let grouping = group_records(&fields, None, &[], &records);
        let lines = render_callout(
            "podcasts",
            &nouns("podcast", "podcasts"),
            &fields,
            &grouping,
            None,
            records.len(),
        );
        assert_eq!(
            join(&lines),
            "\
> [!records] 2 podcasts · 1 queued · 1 done
>
> - **Queued** · 1
>   - [[podcasts/practical_ai|Practical AI]] — ai
> - **Done** · 1
>   - [[podcasts/corepy|Core.py]] — python"
        );
    }

    #[test]
    fn movies_shaped_flat_index_keeps_numeric_facets() {
        let fields = vec![number("year")];
        let records = vec![
            record("movies/dune.md", "Dune", &[("year", scalar("2021"))]),
            record("movies/heat.md", "Heat", &[("year", scalar("1995"))]),
        ];
        let grouping = group_records(&fields, None, &[], &records);
        let lines = render_callout(
            "movies",
            &nouns("movie", "movies"),
            &fields,
            &grouping,
            None,
            records.len(),
        );
        assert_eq!(
            join(&lines),
            "\
> [!records] 2 movies
>
> - [[movies/dune|Dune]] — 2021
> - [[movies/heat|Heat]] — 1995"
        );
    }

    #[test]
    fn tv_shaped_index_keeps_acronym_singular_and_plural() {
        let fields = vec![choices(
            "status",
            &[("to_watch", "To watch"), ("watched", "Watched")],
        )];
        let records = vec![record(
            "tv/severance.md",
            "Severance",
            &[("status", scalar("to_watch"))],
        )];
        let grouping = group_records(&fields, None, &[], &records);
        let lines = render_callout(
            "tv",
            &nouns("TV show", "TV shows"),
            &fields,
            &grouping,
            None,
            1,
        );
        assert!(join(&lines).starts_with("> [!records] 1 TV show · 1 to watch"));
        let two = vec![
            record(
                "tv/severance.md",
                "Severance",
                &[("status", scalar("to_watch"))],
            ),
            record("tv/silo.md", "Silo", &[("status", scalar("watched"))]),
        ];
        let grouping_two = group_records(&fields, None, &[], &two);
        let two_lines = render_callout(
            "tv",
            &nouns("TV show", "TV shows"),
            &fields,
            &grouping_two,
            None,
            two.len(),
        );
        assert!(join(&two_lines)
            .starts_with("> [!records] 2 TV shows · 1 to watch"));
    }

    #[test]
    fn malformed_member_keeps_filename_fallback_in_needs_status() {
        let fields = eat_fields();
        let records = vec![record("eat/mystery_spot.md", "mystery spot", &[])];
        let grouping = group_records(&fields, Some("location"), &[], &records);
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            1,
        );
        let text = join(&lines);
        assert!(text.contains("⚠️ 1 needs a status"));
        assert!(text.contains("[[eat/mystery_spot|mystery spot]]"));
        let two = vec![
            record("eat/mystery_spot.md", "mystery spot", &[]),
            record("eat/other_spot.md", "other spot", &[]),
        ];
        let grouping_two = group_records(&fields, Some("location"), &[], &two);
        let two_lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping_two,
            Some("location"),
            two.len(),
        );
        assert!(join(&two_lines).contains("⚠️ 2 need a status"));
    }

    #[test]
    fn hostile_title_and_facets_are_escaped() {
        let fields = vec![
            choices("status", &[("to_try", "🔖 To try")]),
            text("category", true),
        ];
        let records = vec![record(
            "eat/weird.md",
            "Foo | Bar [x]#^",
            &[
                ("status", scalar("to_try")),
                ("category", scalar("Ramen *x*")),
            ],
        )];
        let grouping = group_records(&fields, None, &[], &records);
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            None,
            1,
        );
        let text = join(&lines);
        assert!(text.contains("[[eat/weird|Foo Bar x]]"));
        assert!(text.contains(r"Ramen \*x\*"));
        assert!(!text.contains("|Foo |"));
    }

    #[test]
    fn lists_and_links_and_empty_facets() {
        let mut companions = link("companions");
        companions.list = true;
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            companions,
            text("notes", true),
        ];
        let records = vec![record(
            "eat/a.md",
            "A",
            &[
                ("status", scalar("to_try")),
                (
                    "companions",
                    IndexedFieldValue::List(vec![
                        "[[kelly|Kelly]]".to_owned(),
                        "Pat".to_owned(),
                    ]),
                ),
                ("notes", scalar("")),
            ],
        )];
        let grouping = group_records(
            &fields,
            None,
            &["companions".to_owned(), "notes".to_owned()],
            &records,
        );
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            None,
            1,
        );
        assert!(join(&lines).contains("[[eat/a|A]] — Kelly, Pat"));
        assert!(!join(&lines).contains("Kelly, Pat ·"));
    }

    #[test]
    fn grouped_rows_drop_the_repeated_location_facet() {
        let fields = eat_fields();
        let mut records = Vec::new();
        for index in 0..13 {
            let location = if index < 7 { "Clark" } else { "Cranford" };
            records.push(record(
                &format!("eat/place_{index:02}.md"),
                &format!("Place {index:02}"),
                &[
                    ("status", scalar("to_try")),
                    ("category", scalar("Ramen")),
                    ("location", scalar(location)),
                ],
            ));
        }
        let grouping = group_records(&fields, Some("location"), &[], &records);
        assert!(!grouping.groups[0].subgroups.is_empty());
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            records.len(),
        );
        let text = join(&lines);
        assert!(text.contains(">   - **Clark** · 7"));
        assert!(text.contains("[[eat/place_00|Place 00]] — Ramen\n"));
        assert!(!text.contains("Place 00]] — Ramen · Clark"));
    }

    #[test]
    fn explicit_facets_keep_location_on_grouped_rows() {
        let fields = eat_fields();
        let mut records = Vec::new();
        for index in 0..13 {
            let location = if index < 7 { "Clark" } else { "Cranford" };
            records.push(record(
                &format!("eat/place_{index:02}.md"),
                &format!("Place {index:02}"),
                &[
                    ("status", scalar("to_try")),
                    ("category", scalar("Ramen")),
                    ("location", scalar(location)),
                ],
            ));
        }
        let grouping = group_records(
            &fields,
            Some("location"),
            &["location".to_owned()],
            &records,
        );
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            records.len(),
        );
        assert!(join(&lines).contains("Place 00]] — Clark"));
    }

    #[test]
    fn path_tie_breaks_equal_titles() {
        let fields = vec![text("location", true)];
        let records = vec![
            record("eat/b.md", "Same", &[("location", scalar("Clark"))]),
            record("eat/a.md", "Same", &[("location", scalar("Clark"))]),
        ];
        let grouping = group_records(&fields, None, &[], &records);
        let lines = render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            None,
            2,
        );
        let text = join(&lines);
        let a = text.find("[[eat/a|Same]]").unwrap();
        let b = text.find("[[eat/b|Same]]").unwrap();
        assert!(a < b, "{text}");
    }

    #[test]
    fn first_insert_and_second_render_are_stable() {
        let fields = eat_fields();
        let grouping = group_records(&fields, Some("location"), &[], &[]);
        let text = "---\ntype: [[album]]\n---\n\n# Eat\n\nIntro.\n\n## Notes\n\n- hi\n";
        let plan = plan_index(
            "eat.md",
            Some(text),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        assert_eq!(plan.state, IndexState::Missing);
        let after = plan.edit.as_ref().unwrap().after.clone();
        assert!(
            after.contains("## Records\n\n> [!records] No restaurants yet\n")
        );
        assert!(after.contains("## Notes\n"));
        let again = plan_index(
            "eat.md",
            Some(&after),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        assert_eq!(again.state, IndexState::Current);
        assert!(again.edit.is_none());
    }

    #[test]
    fn moved_callout_stays_put_and_duplicates_heal() {
        let fields = eat_fields();
        let grouping = group_records(&fields, Some("location"), &[], &[]);
        let callout = join(&render_callout(
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
        ));
        let moved = format!(
            "---\n---\n\n# Eat\n\n## Notes\n\n- hi\n\n## Records\n\n{callout}\n"
        );
        let plan = plan_index(
            "eat.md",
            Some(&moved),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        assert_eq!(plan.state, IndexState::Current);
        let dup = format!(
            "---\n---\n\n# Eat\n\n## Records\n\n{callout}\n\n## Notes\n\n## Records\n\n{callout}\n"
        );
        let healed = plan_index(
            "eat.md",
            Some(&dup),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        assert_eq!(healed.state, IndexState::Stale);
        let after = &healed.edit.as_ref().unwrap().after;
        assert_eq!(after.matches("> [!records]").count(), 1);
        assert_eq!(after.matches("## Records").count(), 1);
        assert!(after.contains("## Notes"));
    }

    #[test]
    fn fences_bom_and_crlf_are_preserved() {
        let fields = eat_fields();
        let grouping = group_records(&fields, Some("location"), &[], &[]);
        let fenced = "---\n---\n\n```md\n> [!records] stale\n```\n\n# Eat\n";
        let plan = plan_index(
            "eat.md",
            Some(fenced),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        assert_eq!(plan.state, IndexState::Missing);
        assert!(plan
            .edit
            .as_ref()
            .unwrap()
            .after
            .contains("```md\n> [!records] stale\n```"));
        let crlf = "\u{feff}---\r\n---\r\n\r\n# Eat\r\n";
        let plan = plan_index(
            "eat.md",
            Some(crlf),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        let after = &plan.edit.as_ref().unwrap().after;
        assert!(after.starts_with("\u{feff}---\r\n"));
        assert!(!after.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn incomplete_folder_or_missing_text_is_damaged() {
        let fields = eat_fields();
        let grouping = group_records(&fields, Some("location"), &[], &[]);
        let damaged = plan_index(
            "eat.md",
            Some("# Eat\n"),
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            false,
        );
        assert_eq!(damaged.state, IndexState::Damaged);
        assert!(damaged.edit.is_none());
        assert_eq!(damaged.findings[0].code, "album_index_damaged");
        let missing_text = plan_index(
            "eat.md",
            None,
            "eat",
            &nouns("restaurant", "restaurants"),
            &fields,
            &grouping,
            Some("location"),
            0,
            true,
        );
        assert_eq!(missing_text.state, IndexState::Damaged);
        assert!(missing_text.edit.is_none());
    }

    #[test]
    fn group_is_large_threshold_matches_contract() {
        assert!(!group_is_large(12));
        assert!(group_is_large(13));
    }

    #[test]
    fn hostile_plural_and_choice_labels_stay_on_one_inert_line() {
        let hostile_plural = "restaurants\n## Injected";
        let hostile_singular = "restaurant\n## Injected";
        let hostile_label =
            "To try\n## Injected [x](y) *bold* `code` [[link]] #tag";
        let fields = vec![choices("status", &[("to_try", hostile_label)])];
        let records =
            vec![record("eat/one.md", "One", &[("status", scalar("to_try"))])];
        let grouping = group_records(&fields, None, &[], &records);
        let hostile = nouns(hostile_singular, hostile_plural);
        let lines = render_callout(
            "eat",
            &hostile,
            &fields,
            &grouping,
            None,
            records.len(),
        );
        let head = lines.first().expect("head line");
        assert!(
            !head.contains('\n') && !head.contains('\r'),
            "header stays one physical line: {head:?}"
        );
        assert!(
            !join(&lines).contains("\n## Injected"),
            "no raw newline injection: {}",
            join(&lines)
        );
        assert!(
            head.contains("restaurant"),
            "normal singular text survives: {head}"
        );
        // Second render of the same bytes is a no-op: plan against the
        // installed text yields Current with no edit.
        let text = format!("# Eat\n\n{}\n", join(&lines));
        let regroup = group_records(&fields, None, &[], &records);
        let plan = plan_index(
            "eat.md",
            Some(&text),
            "eat",
            &hostile,
            &fields,
            &regroup,
            None,
            records.len(),
            true,
        );
        assert_eq!(plan.state, IndexState::Current);
        assert!(plan.edit.is_none());
        let empty = render_callout(
            "eat",
            &hostile,
            &fields,
            &group_records(&fields, None, &[], &[]),
            None,
            0,
        );
        let empty_head = empty.first().expect("empty head");
        assert!(
            !empty_head.contains('\n'),
            "empty-album plural is also inert: {empty_head:?}"
        );
    }

    #[test]
    fn tally_text_uses_singular_for_one_and_plural_otherwise() {
        let fields = eat_fields();
        let empty_grouping = group_records(&fields, None, &[], &[]);
        assert_eq!(
            tally_text(&nouns("restaurant", "restaurants"), &empty_grouping, 0),
            "No restaurants yet"
        );
        let one = vec![record("eat/one.md", "One", &[])];
        let one_grouping = group_records(&fields, None, &[], &one);
        assert!(tally_text(
            &nouns("restaurant", "restaurants"),
            &one_grouping,
            1
        )
        .starts_with("1 restaurant"));
        assert!(tally_text(
            &nouns("restaurant", "restaurants"),
            &one_grouping,
            1
        )
        .contains("⚠️ 1 needs a status"));
        let two = vec![
            record("eat/one.md", "One", &[]),
            record("eat/two.md", "Two", &[]),
        ];
        let two_grouping = group_records(&fields, None, &[], &two);
        let two_tally =
            tally_text(&nouns("restaurant", "restaurants"), &two_grouping, 2);
        assert!(two_tally.starts_with("2 restaurants"));
        assert!(two_tally.contains("⚠️ 2 need a status"));
        let acronym_one = vec![record(
            "tv/severance.md",
            "Severance",
            &[("status", scalar("to_watch"))],
        )];
        let tv_fields = vec![choices(
            "status",
            &[("to_watch", "To watch"), ("watched", "Watched")],
        )];
        let acronym_grouping =
            group_records(&tv_fields, None, &[], &acronym_one);
        assert!(tally_text(
            &nouns("TV show", "TV shows"),
            &acronym_grouping,
            1
        )
        .starts_with("1 TV show"));
        let irregular = vec![record("people/ada.md", "Ada", &[])];
        let irregular_grouping = group_records(&fields, None, &[], &irregular);
        assert!(
            tally_text(&nouns("person", "people"), &irregular_grouping, 1)
                .starts_with("1 person")
        );
        assert!(
            tally_text(&nouns("person", "people"), &irregular_grouping, 1)
                .contains("⚠️ 1 needs a status")
        );
    }
}

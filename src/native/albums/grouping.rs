//! Pure record grouping for album reports and the records index.
//!
//! The helper reads already-indexed folder members plus the album schema.
//! It never writes Markdown, dates, or vault files. Report peeks and
//! rendered rows share the same member order.

use chrono::NaiveDate;

use crate::native::{
    capture_block_ids::wikilink_phrase,
    highlights_ref::humanize_stem,
    records::{
        catalog::IndexedRecord,
        schema::{
            validate_literal_values, FieldKind, RecordField, RecordScalar,
            RecordValue,
        },
        IndexedFieldValue,
    },
};

/// Groups larger than this use `index_by` sub-group order for peeks
/// when that field yields at least two distinct values.
pub(crate) const LARGE_GROUP: usize = 12;

/// Display heading for records missing the `index_by` value.
pub(crate) const MISSING_SUBGROUP: &str = "\u{2014}";

const PEEK: usize = 3;
const FACET_CAP: usize = 3;
const NEEDS_STATUS_LABEL: &str = "⚠️ Needs a status";

/// One already-indexed record folder member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupingRecord {
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) title: String,
    pub(crate) created: Option<NaiveDate>,
    pub(crate) fields: Vec<(String, IndexedFieldValue)>,
}

impl GroupingRecord {
    /// Project a catalog member into the shared grouping record.
    pub(crate) fn from_indexed(note: &IndexedRecord) -> Self {
        let title = note
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| humanize_stem(&note.stem));
        Self {
            path: note.relative_path.to_string_lossy().replace('\\', "/"),
            stem: note.stem.clone(),
            title,
            created: created_date(note),
            fields: note
                .fields
                .iter()
                .map(|field| (field.key.clone(), field.value.clone()))
                .collect(),
        }
    }
}

/// Grouping projection shared by `bob albums` reports and album-index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordGrouping {
    pub(crate) group_field: Option<String>,
    pub(crate) groups: Vec<RecordGroup>,
    pub(crate) needs: Option<RecordGroup>,
    pub(crate) needs_status: usize,
    /// `[!records]` headings when `index_group_by` selects a field other
    /// than the auto-picked group field; `None` keeps `groups` and `needs`
    /// as the headings.
    pub(crate) index_headings: Option<IndexHeadings>,
    /// Title-then-path order when the album has no choice groups.
    pub(crate) ungrouped: Vec<GroupingRecord>,
    pub(crate) last_record: Option<String>,
    pub(crate) index_facets: Vec<String>,
    pub(crate) explicit_facets: bool,
}

/// Index headings for an `index_group_by` field that differs from the
/// auto-picked group field. Only the rendered `[!records]` callout reads
/// them; the tally, `groups`, and `needs_status` stay on the group field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexHeadings {
    pub(crate) field: String,
    pub(crate) groups: Vec<RecordGroup>,
    pub(crate) needs: Option<RecordGroup>,
}

/// One nonempty choice group in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordGroup {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) count: usize,
    pub(crate) peek: Vec<String>,
    /// Nonempty only when this group actually renders `index_by` headings.
    pub(crate) subgroups: Vec<RecordSubgroup>,
    /// Members in render order: subgroup order, else title then path.
    pub(crate) members: Vec<GroupingRecord>,
}

/// One `index_by` subgroup inside a large choice group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordSubgroup {
    pub(crate) display: String,
    pub(crate) missing: bool,
    pub(crate) members: Vec<GroupingRecord>,
}

/// Occupied choice bucket in declared order, before title or subgroup sort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChoiceBucket<'a> {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) members: Vec<&'a GroupingRecord>,
}

/// Project counts, groups, peeks, facets, and freshness from schema
/// plus folder members. `index_group_by` is the album's effective
/// `[!records]` heading field; it changes only `index_headings` and the
/// default facet exclusion. `explicit_facets` is the authored
/// `index_facets` list; an empty slice selects the default facet rule.
pub(crate) fn group_records(
    fields: &[RecordField],
    index_by: Option<&str>,
    index_group_by: Option<&str>,
    explicit_facets: &[String],
    records: &[GroupingRecord],
) -> RecordGrouping {
    let last_record = records
        .iter()
        .filter_map(|record| record.created)
        .max()
        .map(|date| date.format("%Y-%m-%d").to_string());
    let explicit = !explicit_facets.is_empty();
    let Some(group_field) = group_field(fields) else {
        let mut ungrouped = records.to_vec();
        ungrouped.sort_by(title_order);
        return RecordGrouping {
            group_field: None,
            groups: Vec::new(),
            needs: None,
            needs_status: 0,
            index_headings: None,
            ungrouped,
            last_record,
            index_facets: facets(fields, None, explicit_facets),
            explicit_facets: explicit,
        };
    };

    let (occupied, needs) = bucket_choice_members(group_field, records);
    let groups = occupied
        .iter()
        .map(|bucket| {
            finish_group(
                bucket.value.clone(),
                bucket.label.clone(),
                &bucket.members,
                index_by,
            )
        })
        .collect();
    let needs_status = needs.len();
    let needs = if needs.is_empty() {
        None
    } else {
        Some(finish_group(
            String::new(),
            NEEDS_STATUS_LABEL.to_owned(),
            &needs,
            index_by,
        ))
    };

    let index_headings =
        index_headings(fields, group_field, index_group_by, index_by, records);
    let heading_key = index_headings
        .as_ref()
        .map_or(group_field.key.as_str(), |headings| headings.field.as_str());
    let index_facets = facets(fields, Some(heading_key), explicit_facets);

    RecordGrouping {
        group_field: Some(group_field.key.clone()),
        groups,
        needs,
        needs_status,
        index_headings,
        ungrouped: Vec::new(),
        last_record,
        index_facets,
        explicit_facets: explicit,
    }
}

/// Headings for an `index_group_by` field, or `None` when the key is unset,
/// is not a single-value choices field, or names the auto-picked group
/// field (that index is already today's bytes).
fn index_headings(
    fields: &[RecordField],
    group_field: &RecordField,
    index_group_by: Option<&str>,
    index_by: Option<&str>,
    records: &[GroupingRecord],
) -> Option<IndexHeadings> {
    let key = index_group_by?;
    let field = fields.iter().find(|field| field.key == key)?;
    if field.key == group_field.key
        || field.list
        || !matches!(field.kind, FieldKind::Choices(_))
    {
        return None;
    }
    let (occupied, needs) = bucket_choice_members(field, records);
    let groups = occupied
        .iter()
        .map(|bucket| {
            finish_group(
                bucket.value.clone(),
                bucket.label.clone(),
                &bucket.members,
                index_by,
            )
        })
        .collect();
    let needs = if needs.is_empty() {
        None
    } else {
        Some(finish_group(
            String::new(),
            needs_group_label(field),
            &needs,
            index_by,
        ))
    };
    Some(IndexHeadings {
        field: field.key.clone(),
        groups,
        needs,
    })
}

pub(crate) fn group_field(fields: &[RecordField]) -> Option<&RecordField> {
    fields
        .iter()
        .find(|field| {
            field.key == "status" && matches!(field.kind, FieldKind::Choices(_))
        })
        .or_else(|| {
            fields
                .iter()
                .find(|field| matches!(field.kind, FieldKind::Choices(_)))
        })
}

/// Needs-group heading for a walk or `index_group_by` heading grouped by
/// `field`. The auto-picked index group keeps the status wording even when
/// that field is not status.
pub(crate) fn needs_group_label(field: &RecordField) -> String {
    if field.key == "status" {
        NEEDS_STATUS_LABEL.to_owned()
    } else {
        format!("⚠️ Needs {}", field.key)
    }
}

/// Bucket records by a choices field in declared choice order. Empty
/// choices are omitted. Missing, blank, malformed, list-shaped, and
/// undeclared values collect in `needs`. Callers choose subgrouping.
pub(crate) fn bucket_choice_members<'a>(
    field: &RecordField,
    records: &'a [GroupingRecord],
) -> (Vec<ChoiceBucket<'a>>, Vec<&'a GroupingRecord>) {
    let FieldKind::Choices(choices) = &field.kind else {
        return (Vec::new(), records.iter().collect());
    };
    let mut buckets: Vec<Vec<&GroupingRecord>> =
        (0..choices.len()).map(|_| Vec::new()).collect();
    let mut needs: Vec<&GroupingRecord> = Vec::new();
    for record in records {
        match group_value(field, record) {
            Some(value) => {
                if let Some(index) =
                    choices.iter().position(|choice| choice.value == value)
                {
                    buckets[index].push(record);
                } else {
                    needs.push(record);
                }
            }
            None => needs.push(record),
        }
    }
    let occupied = choices
        .iter()
        .zip(buckets)
        .filter(|(_, members)| !members.is_empty())
        .map(|(choice, members)| ChoiceBucket {
            value: choice.value.clone(),
            label: choice.label.clone(),
            members,
        })
        .collect();
    (occupied, needs)
}

fn facets(
    fields: &[RecordField],
    group_key: Option<&str>,
    explicit: &[String],
) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let excluded = |key: &str| group_key.is_some_and(|group| group == key);
    let mut selected = Vec::new();
    for field in fields {
        if matches!(field.kind, FieldKind::Choices(_))
            && !excluded(&field.key)
            && !selected.contains(&field.key)
        {
            selected.push(field.key.clone());
        }
        if selected.len() == FACET_CAP {
            return selected;
        }
    }
    for field in fields {
        if field.suggest
            && !excluded(&field.key)
            && !selected.contains(&field.key)
        {
            selected.push(field.key.clone());
        }
        if selected.len() == FACET_CAP {
            break;
        }
    }
    selected
}

pub(crate) fn group_value(
    field: &RecordField,
    record: &GroupingRecord,
) -> Option<String> {
    let value = record
        .fields
        .iter()
        .find(|(key, _)| key == &field.key)
        .map(|(_, value)| value)?;
    let raw = match value {
        IndexedFieldValue::Scalar(text) if !text.trim().is_empty() => {
            vec![text.clone()]
        }
        IndexedFieldValue::List(items) if field.list => {
            let items: Vec<String> = items
                .iter()
                .filter(|item| !item.trim().is_empty())
                .cloned()
                .collect();
            if items.is_empty() {
                return None;
            }
            items
        }
        IndexedFieldValue::Null
        | IndexedFieldValue::Unsupported
        | IndexedFieldValue::List(_)
        | IndexedFieldValue::Scalar(_) => return None,
    };
    let today = NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch");
    // Stored YAML members are already delimited. CLI comma splitting would
    // break a literal member such as "Science, fiction" into two invalid
    // fragments, so validate with literal semantics.
    match validate_literal_values(field, &raw, today).ok()? {
        RecordValue::Scalar(RecordScalar::Choice(choice)) => Some(choice),
        RecordValue::List(values) => match values.as_slice() {
            [RecordScalar::Choice(choice)] => Some(choice.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn finish_group(
    value: String,
    label: String,
    members: &[&GroupingRecord],
    index_by: Option<&str>,
) -> RecordGroup {
    let (subgroups, ordered) = if uses_subgroups(members, index_by) {
        let subgroups = build_subgroups(members, index_by.unwrap());
        let ordered = subgroups
            .iter()
            .flat_map(|subgroup| subgroup.members.iter().cloned())
            .collect::<Vec<_>>();
        (subgroups, ordered)
    } else {
        let mut ordered = members.iter().copied().cloned().collect::<Vec<_>>();
        ordered.sort_by(title_order);
        (Vec::new(), ordered)
    };
    let peek = ordered
        .iter()
        .take(PEEK)
        .map(|record| record.title.clone())
        .collect();
    RecordGroup {
        value,
        label,
        count: ordered.len(),
        peek,
        subgroups,
        members: ordered,
    }
}

fn uses_subgroups(members: &[&GroupingRecord], index_by: Option<&str>) -> bool {
    index_by.is_some_and(|field| {
        members.len() > LARGE_GROUP
            && distinct_index_values(members, field) >= 2
    })
}

fn build_subgroups(
    members: &[&GroupingRecord],
    field: &str,
) -> Vec<RecordSubgroup> {
    let mut present: Vec<(String, Vec<GroupingRecord>)> = Vec::new();
    let mut missing = Vec::new();
    for record in members {
        match field_text(record, field) {
            None => missing.push((*record).clone()),
            Some(raw) => {
                let fold = fold_display(&raw);
                if let Some((_, bucket)) =
                    present.iter_mut().find(|(key, _)| key == &fold)
                {
                    bucket.push((*record).clone());
                } else {
                    present.push((fold, vec![(*record).clone()]));
                }
            }
        }
    }
    let mut subgroups = present
        .into_iter()
        .map(|(_, mut bucket)| {
            let raws = bucket
                .iter()
                .filter_map(|record| field_text(record, field))
                .collect::<Vec<_>>();
            let display = representative_display(&raws);
            bucket.sort_by(title_order);
            RecordSubgroup {
                display,
                missing: false,
                members: bucket,
            }
        })
        .collect::<Vec<_>>();
    subgroups.sort_by(|left, right| {
        left.display
            .to_lowercase()
            .cmp(&right.display.to_lowercase())
            .then_with(|| left.display.cmp(&right.display))
    });
    if !missing.is_empty() {
        missing.sort_by(title_order);
        subgroups.push(RecordSubgroup {
            display: MISSING_SUBGROUP.to_owned(),
            missing: true,
            members: missing,
        });
    }
    subgroups
}

fn representative_display(raws: &[String]) -> String {
    #[derive(Default)]
    struct Stat {
        count: usize,
        plain: bool,
    }
    let mut stats = std::collections::BTreeMap::<String, Stat>::new();
    for raw in raws {
        let display = display_value(raw);
        let entry = stats.entry(display).or_default();
        entry.count += 1;
        if !is_wikilink(raw.trim()) {
            entry.plain = true;
        }
    }
    stats
        .into_iter()
        .max_by(|(left_display, left), (right_display, right)| {
            left.count
                .cmp(&right.count)
                .then_with(|| left.plain.cmp(&right.plain))
                .then_with(|| right_display.cmp(left_display))
        })
        .map(|(display, _)| display)
        .unwrap_or_else(|| MISSING_SUBGROUP.to_owned())
}

fn is_wikilink(value: &str) -> bool {
    value
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .is_some_and(|inner| !inner.contains("[["))
}

pub(crate) fn title_order(
    left: &GroupingRecord,
    right: &GroupingRecord,
) -> std::cmp::Ordering {
    left.title
        .to_lowercase()
        .cmp(&right.title.to_lowercase())
        .then_with(|| left.path.cmp(&right.path))
}

fn distinct_index_values(members: &[&GroupingRecord], field: &str) -> usize {
    let mut folds = members
        .iter()
        .map(|record| field_text(record, field).map(|text| fold_display(&text)))
        .collect::<Vec<_>>();
    folds.sort();
    folds.dedup();
    folds.len()
}

fn field_text(record: &GroupingRecord, key: &str) -> Option<String> {
    let value = record
        .fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)?;
    match value {
        IndexedFieldValue::Scalar(text) if !text.trim().is_empty() => {
            Some(text.clone())
        }
        IndexedFieldValue::List(items) => {
            items.iter().find(|item| !item.trim().is_empty()).cloned()
        }
        _ => None,
    }
}

fn fold_display(value: &str) -> String {
    display_value(value).to_lowercase()
}

pub(crate) fn display_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        && !inner.contains("[[")
    {
        return wikilink_phrase(inner);
    }
    trimmed.to_owned()
}

fn created_date(note: &IndexedRecord) -> Option<NaiveDate> {
    let field = note.fields.iter().find(|field| field.key == "created")?;
    let IndexedFieldValue::Scalar(text) = &field.value else {
        return None;
    };
    let date = text.get(..10)?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::schema::RecordChoice;

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

    fn record(
        path: &str,
        title: &str,
        created: Option<&str>,
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
            created: created.and_then(|raw| {
                NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
            }),
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        }
    }

    fn scalar(value: &str) -> IndexedFieldValue {
        IndexedFieldValue::Scalar(value.to_owned())
    }

    #[test]
    fn status_choices_lead_and_empty_groups_are_omitted() {
        let fields = vec![
            choices(
                "status",
                &[
                    ("to_try", "🔖 To try"),
                    ("liked", "💚 Liked"),
                    ("not_liked", "🚫 Not liked"),
                ],
            ),
            text("location", true),
        ];
        let records = vec![
            record(
                "eat/b.md",
                "Bubble Tea",
                Some("2026-11-08"),
                &[("status", scalar("to_try"))],
            ),
            record(
                "eat/a.md",
                "Ani Ramen House",
                Some("2026-11-01"),
                &[("status", scalar("to_try"))],
            ),
            record(
                "eat/c.md",
                "Cafe",
                Some("2026-10-01"),
                &[("status", scalar("liked"))],
            ),
            record(
                "eat/bad.md",
                "Unknown",
                None,
                &[("status", scalar("nope"))],
            ),
            record(
                "eat/list.md",
                "List",
                None,
                &[(
                    "status",
                    IndexedFieldValue::List(vec!["to_try".to_owned()]),
                )],
            ),
        ];
        let grouping = group_records(&fields, None, None, &[], &records);
        assert_eq!(grouping.group_field.as_deref(), Some("status"));
        assert_eq!(grouping.groups.len(), 2);
        assert_eq!(grouping.groups[0].value, "to_try");
        assert_eq!(grouping.groups[0].count, 2);
        assert_eq!(grouping.groups[0].peek, ["Ani Ramen House", "Bubble Tea"]);
        assert_eq!(grouping.groups[1].value, "liked");
        assert_eq!(grouping.needs_status, 2);
        assert_eq!(grouping.last_record.as_deref(), Some("2026-11-08"));
        assert_eq!(grouping.index_facets, ["location"]);
    }

    #[test]
    fn first_choices_field_wins_without_status() {
        let fields = vec![
            text("title_alias", false),
            choices("verdict", &[("up", "Up"), ("down", "Down")]),
            choices("mood", &[("ok", "Ok")]),
        ];
        let records = vec![record(
            "movies/dune.md",
            "Dune",
            None,
            &[("verdict", scalar("up"))],
        )];
        let grouping = group_records(&fields, None, None, &[], &records);
        assert_eq!(grouping.group_field.as_deref(), Some("verdict"));
        assert_eq!(grouping.groups[0].value, "up");
        assert_eq!(grouping.needs_status, 0);
    }

    #[test]
    fn no_choices_means_no_groups() {
        let fields = vec![text("location", true)];
        let records = vec![record(
            "eat/a.md",
            "A",
            Some("2026-01-01"),
            &[("location", scalar("Clark"))],
        )];
        let grouping = group_records(&fields, None, None, &[], &records);
        assert_eq!(grouping.group_field, None);
        assert!(grouping.groups.is_empty());
        assert_eq!(grouping.needs_status, 0);
        assert_eq!(grouping.last_record.as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn explicit_facets_keep_author_order() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            text("category", true),
            text("location", true),
        ];
        let grouping = group_records(
            &fields,
            Some("location"),
            None,
            &["location".to_owned(), "category".to_owned()],
            &[],
        );
        assert_eq!(grouping.index_facets, ["location", "category"]);
    }

    #[test]
    fn default_facets_skip_group_field_and_keep_index_by() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            choices("kind", &[("bar", "Bar")]),
            text("location", true),
            text("category", true),
            text("quiet", false),
        ];
        let grouping = group_records(&fields, Some("location"), None, &[], &[]);
        assert_eq!(grouping.index_facets, ["kind", "location", "category"]);
    }

    #[test]
    fn missing_unknown_and_invalid_shapes_count_as_needs_status() {
        let mut status =
            choices("status", &[("to_try", "To try"), ("liked", "Liked")]);
        status.list = true;
        let scalar_status = choices("status", &[("to_try", "To try")]);
        let list_records = vec![
            record("eat/ok.md", "Ok", None, &[("status", scalar("to_try"))]),
            record("eat/missing.md", "Missing", None, &[]),
            record(
                "eat/unknown.md",
                "Unknown",
                None,
                &[("status", scalar("nope"))],
            ),
            record(
                "eat/null.md",
                "Null",
                None,
                &[("status", IndexedFieldValue::Null)],
            ),
            record(
                "eat/unsupported.md",
                "Unsupported",
                None,
                &[("status", IndexedFieldValue::Unsupported)],
            ),
        ];
        let grouping = group_records(&[status], None, None, &[], &list_records);
        assert_eq!(grouping.groups[0].count, 1);
        assert_eq!(grouping.needs_status, 4);

        let scalar_records = vec![
            record(
                "eat/list.md",
                "List",
                None,
                &[(
                    "status",
                    IndexedFieldValue::List(vec!["to_try".to_owned()]),
                )],
            ),
            record("eat/empty.md", "Empty", None, &[("status", scalar(""))]),
        ];
        let grouping =
            group_records(&[scalar_status], None, None, &[], &scalar_records);
        assert!(grouping.groups.is_empty());
        assert_eq!(grouping.needs_status, 2);
    }

    #[test]
    fn large_index_by_group_orders_peeks_by_folded_value_then_title() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            text("location", true),
        ];
        let mut records = Vec::new();
        for index in 0..13 {
            let title = format!("Place {index:02}");
            let path = format!("eat/place_{index:02}.md");
            let location = if index < 6 {
                "[[towns#x|Cranford]]"
            } else if index < 10 {
                "Clark"
            } else {
                "clark"
            };
            records.push(record(
                &path,
                &title,
                None,
                &[("status", scalar("to_try")), ("location", scalar(location))],
            ));
        }
        let grouping =
            group_records(&fields, Some("location"), None, &[], &records);
        assert_eq!(grouping.groups[0].count, 13);
        assert_eq!(grouping.groups[0].subgroups.len(), 2);
        assert_eq!(grouping.groups[0].subgroups[0].display, "Clark");
        assert_eq!(grouping.groups[0].subgroups[1].display, "Cranford");
        assert_eq!(
            grouping.groups[0].peek,
            ["Place 06", "Place 07", "Place 08"]
        );
        let first_titles: Vec<&str> = grouping.groups[0]
            .members
            .iter()
            .take(3)
            .map(|record| record.title.as_str())
            .collect();
        assert_eq!(grouping.groups[0].peek, first_titles);
    }

    #[test]
    fn twelve_members_stay_flat_even_with_two_index_values() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            text("location", true),
        ];
        let mut records = Vec::new();
        for index in 0..12 {
            let location = if index < 6 { "Clark" } else { "Cranford" };
            records.push(record(
                &format!("eat/place_{index:02}.md"),
                &format!("Place {index:02}"),
                None,
                &[("status", scalar("to_try")), ("location", scalar(location))],
            ));
        }
        let grouping =
            group_records(&fields, Some("location"), None, &[], &records);
        assert!(grouping.groups[0].subgroups.is_empty());
        assert_eq!(
            grouping.groups[0].peek,
            ["Place 00", "Place 01", "Place 02"]
        );
    }

    #[test]
    fn thirteen_members_with_one_folded_value_stay_flat() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            text("location", true),
        ];
        let mut records = Vec::new();
        for index in 0..13 {
            let location = if index % 2 == 0 {
                "Clark"
            } else {
                "[[x|clark]]"
            };
            records.push(record(
                &format!("eat/place_{index:02}.md"),
                &format!("Place {index:02}"),
                None,
                &[("status", scalar("to_try")), ("location", scalar(location))],
            ));
        }
        let grouping =
            group_records(&fields, Some("location"), None, &[], &records);
        assert!(grouping.groups[0].subgroups.is_empty());
        assert_eq!(
            grouping.groups[0].peek,
            ["Place 00", "Place 01", "Place 02"]
        );
    }

    #[test]
    fn subgroup_heading_prefers_plain_text_on_a_frequency_tie() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            text("location", true),
        ];
        let mut records = Vec::new();
        for index in 0..13 {
            let location = if index < 4 {
                "Cranford"
            } else if index < 8 {
                "[[towns#x|Cranford]]"
            } else {
                "Clark"
            };
            records.push(record(
                &format!("eat/place_{index:02}.md"),
                &format!("Place {index:02}"),
                None,
                &[("status", scalar("to_try")), ("location", scalar(location))],
            ));
        }
        let grouping =
            group_records(&fields, Some("location"), None, &[], &records);
        let cranford = grouping.groups[0]
            .subgroups
            .iter()
            .find(|subgroup| subgroup.display == "Cranford")
            .expect("cranford subgroup");
        assert_eq!(cranford.members.len(), 8);
    }

    #[test]
    fn missing_index_by_values_sort_last_under_an_em_dash() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            text("location", true),
        ];
        let mut records = Vec::new();
        for index in 0..13 {
            let mut values = vec![("status", scalar("to_try"))];
            if index < 7 {
                values.push(("location", scalar("Clark")));
            }
            records.push(record(
                &format!("eat/place_{index:02}.md"),
                &format!("Place {index:02}"),
                None,
                &values,
            ));
        }
        let grouping =
            group_records(&fields, Some("location"), None, &[], &records);
        let subgroups = &grouping.groups[0].subgroups;
        assert_eq!(subgroups.len(), 2);
        assert_eq!(subgroups[0].display, "Clark");
        assert!(subgroups[1].missing);
        assert_eq!(subgroups[1].display, MISSING_SUBGROUP);
        assert_eq!(subgroups[1].members[0].title, "Place 07");
    }

    #[test]
    fn walk_buckets_follow_an_explicit_non_status_field() {
        let section = choices(
            "section",
            &[("intro", "Intro"), ("empty", "Empty"), ("body", "Body")],
        );
        let status = choices("status", &[("open", "Open"), ("done", "Done")]);
        let records = vec![
            record(
                "z/b.md",
                "Beta",
                None,
                &[("section", scalar("body")), ("status", scalar("open"))],
            ),
            record(
                "z/a.md",
                "Alpha",
                None,
                &[("section", scalar("intro")), ("status", scalar("done"))],
            ),
            record(
                "z/c.md",
                "Gamma",
                None,
                &[("section", scalar("body")), ("status", scalar("done"))],
            ),
            record("z/needs.md", "Needs", None, &[("status", scalar("open"))]),
        ];
        let (occupied, needs) = bucket_choice_members(&section, &records);
        assert_eq!(
            occupied
                .iter()
                .map(|bucket| bucket.value.as_str())
                .collect::<Vec<_>>(),
            ["intro", "body"]
        );
        assert_eq!(occupied[0].members[0].title, "Alpha");
        assert_eq!(
            occupied[1]
                .members
                .iter()
                .map(|record| record.title.as_str())
                .collect::<Vec<_>>(),
            ["Beta", "Gamma"]
        );
        assert_eq!(needs.len(), 1);
        assert_eq!(needs[0].title, "Needs");
        assert_eq!(needs_group_label(&section), "⚠️ Needs section");
        assert_eq!(needs_group_label(&status), NEEDS_STATUS_LABEL);

        let index = group_records(
            &[status.clone(), section],
            None,
            None,
            &[],
            &records,
        );
        assert_eq!(index.group_field.as_deref(), Some("status"));
        assert_eq!(index.groups[0].value, "open");
    }

    #[test]
    fn literal_comma_member_groups_without_splitting() {
        let mut category = choices(
            "category",
            &[("Science, fiction", "Fiction"), ("other", "Other")],
        );
        category.list = true;
        let members =
            IndexedFieldValue::List(vec!["Science, fiction".to_owned()]);
        let records = vec![record(
            "items/one.md",
            "One",
            None,
            &[("category", members)],
        )];
        let grouping = group_records(&[category], None, None, &[], &records);
        assert_eq!(grouping.group_field.as_deref(), Some("category"));
        assert_eq!(grouping.groups.len(), 1);
        assert_eq!(grouping.groups[0].value, "Science, fiction");
        assert_eq!(grouping.groups[0].count, 1);
        assert_eq!(grouping.needs_status, 0);
    }

    fn outline_fields() -> Vec<RecordField> {
        vec![
            choices("status", &[("seed", "🌱 Seed"), ("done", "✅ Done")]),
            choices(
                "section",
                &[
                    ("intro", "🪝 Intro"),
                    ("empty", "🕳️ Empty"),
                    ("what", "🧱 What is SASE?"),
                    ("future", "🔮 Future Work"),
                ],
            ),
            choices("claim", &[("a", "A"), ("b", "B")]),
        ]
    }

    fn outline_records() -> Vec<GroupingRecord> {
        vec![
            record(
                "z/future_b.md",
                "Zeta Future",
                None,
                &[("status", scalar("seed")), ("section", scalar("future"))],
            ),
            record(
                "z/what_b.md",
                "why a tui",
                None,
                &[("status", scalar("seed")), ("section", scalar("what"))],
            ),
            record(
                "z/intro.md",
                "What Came Before",
                None,
                &[("status", scalar("done")), ("section", scalar("intro"))],
            ),
            record(
                "z/what_a.md",
                "Abstractions",
                None,
                &[("status", scalar("seed")), ("section", scalar("what"))],
            ),
            record(
                "z/missing.md",
                "No Section",
                None,
                &[("status", scalar("seed"))],
            ),
            record(
                "z/unknown.md",
                "Unknown Section",
                None,
                &[("status", scalar("seed")), ("section", scalar("nope"))],
            ),
            record(
                "z/list.md",
                "List Section",
                None,
                &[
                    ("status", scalar("seed")),
                    (
                        "section",
                        IndexedFieldValue::List(vec!["intro".to_owned()]),
                    ),
                ],
            ),
        ]
    }

    #[test]
    fn index_group_by_heads_the_index_in_declared_order() {
        let fields = outline_fields();
        let grouping = group_records(
            &fields,
            None,
            Some("section"),
            &[],
            &outline_records(),
        );
        let headings = grouping.index_headings.as_ref().expect("headings");
        assert_eq!(headings.field, "section");
        // Declared choice order, empty choices omitted.
        assert_eq!(
            headings
                .groups
                .iter()
                .map(|group| group.value.as_str())
                .collect::<Vec<_>>(),
            ["intro", "what", "future"]
        );
        // Members keep title order inside a heading, ignoring case.
        assert_eq!(
            headings.groups[1]
                .members
                .iter()
                .map(|record| record.title.as_str())
                .collect::<Vec<_>>(),
            ["Abstractions", "why a tui"]
        );
        assert_eq!(headings.groups[1].count, 2);
        // Missing, unknown, and list-shaped values go last under the
        // section-named needs heading.
        let needs = headings.needs.as_ref().expect("needs heading");
        assert_eq!(needs.label, "⚠️ Needs section");
        assert_eq!(
            needs
                .members
                .iter()
                .map(|record| record.title.as_str())
                .collect::<Vec<_>>(),
            ["List Section", "No Section", "Unknown Section"]
        );
    }

    #[test]
    fn index_group_by_leaves_status_groups_and_needs_status_alone() {
        let fields = outline_fields();
        let records = outline_records();
        let plain = group_records(&fields, None, None, &[], &records);
        let headed =
            group_records(&fields, None, Some("section"), &[], &records);
        assert_eq!(plain.index_headings, None);
        assert_eq!(headed.group_field.as_deref(), Some("status"));
        assert_eq!(headed.groups, plain.groups);
        assert_eq!(headed.needs, plain.needs);
        assert_eq!(headed.needs_status, plain.needs_status);
        assert_eq!(headed.last_record, plain.last_record);
        assert_eq!(headed.groups[0].value, "seed");
        assert_eq!(headed.groups[0].count, 6);
    }

    #[test]
    fn index_group_by_default_facets_swap_the_excluded_field() {
        let fields = outline_fields();
        let records = outline_records();
        let plain = group_records(&fields, None, None, &[], &records);
        assert_eq!(plain.index_facets, ["section", "claim"]);
        let headed =
            group_records(&fields, None, Some("section"), &[], &records);
        assert_eq!(headed.index_facets, ["status", "claim"]);
        assert!(!headed.explicit_facets);
    }

    #[test]
    fn index_group_by_honors_explicit_facets_exactly() {
        let fields = outline_fields();
        let explicit = ["section".to_owned(), "claim".to_owned()];
        let headed = group_records(
            &fields,
            None,
            Some("section"),
            &explicit,
            &outline_records(),
        );
        assert!(headed.index_headings.is_some());
        assert_eq!(headed.index_facets, ["section", "claim"]);
        assert!(headed.explicit_facets);
    }

    #[test]
    fn index_group_by_still_subgroups_a_large_heading_group() {
        let mut fields = outline_fields();
        fields.push(text("location", true));
        let records: Vec<GroupingRecord> = (0..=LARGE_GROUP)
            .map(|index| {
                let place = if index % 2 == 0 { "Boston" } else { "Austin" };
                record(
                    &format!("z/n{index:02}.md"),
                    &format!("Note {index:02}"),
                    None,
                    &[
                        ("status", scalar("seed")),
                        ("section", scalar("what")),
                        ("location", scalar(place)),
                    ],
                )
            })
            .collect();
        let grouping = group_records(
            &fields,
            Some("location"),
            Some("section"),
            &[],
            &records,
        );
        let headings = grouping.index_headings.as_ref().expect("headings");
        assert_eq!(headings.groups.len(), 1);
        let group = &headings.groups[0];
        assert_eq!(group.count, LARGE_GROUP + 1);
        assert_eq!(
            group
                .subgroups
                .iter()
                .map(|subgroup| subgroup.display.as_str())
                .collect::<Vec<_>>(),
            ["Austin", "Boston"]
        );
    }

    #[test]
    fn index_group_by_equal_to_the_auto_field_has_no_headings() {
        let fields = outline_fields();
        let records = outline_records();
        let named_status =
            group_records(&fields, None, Some("status"), &[], &records);
        assert_eq!(named_status.index_headings, None);
        let plain = group_records(&fields, None, None, &[], &records);
        assert_eq!(named_status, plain);

        // A non-status auto field is the same no-op.
        let no_status = vec![fields[1].clone(), fields[2].clone()];
        let auto =
            group_records(&no_status, None, Some("section"), &[], &records);
        assert_eq!(auto.group_field.as_deref(), Some("section"));
        assert_eq!(auto.index_headings, None);
    }

    #[test]
    fn index_group_by_ignores_unusable_fields() {
        let mut fields = outline_fields();
        fields.push(text("location", true));
        let mut tags = choices("tags", &[("x", "X")]);
        tags.list = true;
        fields.push(tags);
        let records = outline_records();
        for key in ["location", "tags", "missing"] {
            let grouping =
                group_records(&fields, None, Some(key), &[], &records);
            assert_eq!(grouping.index_headings, None, "key {key:?}");
        }
        // Without any choices field there is nothing to group by.
        let ungrouped = group_records(
            &[text("location", true)],
            None,
            Some("section"),
            &[],
            &records,
        );
        assert_eq!(ungrouped.index_headings, None);
    }
}

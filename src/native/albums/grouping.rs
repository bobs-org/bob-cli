//! Pure record grouping for album reports and the later album index.
//!
//! The helper reads already-indexed folder members plus the album schema.
//! It never writes Markdown, dates, or vault files.

use chrono::NaiveDate;

use crate::native::{
    capture_block_ids::wikilink_phrase,
    records::{
        schema::{
            validate_field_values, FieldKind, RecordField, RecordScalar,
            RecordValue,
        },
        IndexedFieldValue,
    },
};

/// Groups larger than this use `index_by` sub-group order for peeks
/// when that field yields at least two distinct values.
pub(crate) const LARGE_GROUP: usize = 12;

const PEEK: usize = 3;
const FACET_CAP: usize = 3;

/// One already-indexed record folder member.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GroupingRecord {
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) created: Option<NaiveDate>,
    pub(crate) fields: Vec<(String, IndexedFieldValue)>,
}

/// Grouping projection shared by `bob albums` reports and album-index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordGrouping {
    pub(crate) group_field: Option<String>,
    pub(crate) groups: Vec<RecordGroup>,
    pub(crate) needs_status: usize,
    pub(crate) last_record: Option<String>,
    pub(crate) index_facets: Vec<String>,
}

/// One nonempty choice group in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordGroup {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) count: usize,
    pub(crate) peek: Vec<String>,
}

/// Project counts, groups, peeks, facets, and freshness from schema
/// plus folder members. `explicit_facets` is the authored `index_facets`
/// list; an empty slice selects the default facet rule.
pub(crate) fn group_records(
    fields: &[RecordField],
    index_by: Option<&str>,
    explicit_facets: &[String],
    records: &[GroupingRecord],
) -> RecordGrouping {
    let last_record = records
        .iter()
        .filter_map(|record| record.created)
        .max()
        .map(|date| date.format("%Y-%m-%d").to_string());
    let Some(group_field) = group_field(fields) else {
        return RecordGrouping {
            group_field: None,
            groups: Vec::new(),
            needs_status: 0,
            last_record,
            index_facets: facets(fields, None, index_by, explicit_facets),
        };
    };

    let FieldKind::Choices(choices) = &group_field.kind else {
        unreachable!("group_field only returns choices fields");
    };
    let mut buckets: Vec<Vec<&GroupingRecord>> =
        (0..choices.len()).map(|_| Vec::new()).collect();
    let mut needs: Vec<&GroupingRecord> = Vec::new();
    for record in records {
        match group_value(group_field, record) {
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

    let mut groups = Vec::new();
    for (choice, members) in choices.iter().zip(buckets) {
        if members.is_empty() {
            continue;
        }
        let peek = peek_titles(&members, index_by);
        groups.push(RecordGroup {
            value: choice.value.clone(),
            label: choice.label.clone(),
            count: members.len(),
            peek,
        });
    }

    RecordGrouping {
        group_field: Some(group_field.key.clone()),
        groups,
        needs_status: needs.len(),
        last_record,
        index_facets: facets(
            fields,
            Some(group_field.key.as_str()),
            index_by,
            explicit_facets,
        ),
    }
}

fn group_field(fields: &[RecordField]) -> Option<&RecordField> {
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

fn facets(
    fields: &[RecordField],
    group_key: Option<&str>,
    index_by: Option<&str>,
    explicit: &[String],
) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let excluded = |key: &str| {
        group_key.is_some_and(|group| group == key)
            || index_by.is_some_and(|by| by == key)
    };
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

fn group_value(field: &RecordField, record: &GroupingRecord) -> Option<String> {
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
    match validate_field_values(field, &raw, today).ok()? {
        RecordValue::Scalar(RecordScalar::Choice(choice)) => Some(choice),
        RecordValue::List(values) => match values.as_slice() {
            [RecordScalar::Choice(choice)] => Some(choice.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn peek_titles(
    members: &[&GroupingRecord],
    index_by: Option<&str>,
) -> Vec<String> {
    let mut ordered = members.to_vec();
    if let Some(field) = index_by
        && members.len() > LARGE_GROUP
        && distinct_index_values(members, field) >= 2
    {
        ordered.sort_by(|left, right| {
            match (index_sort_key(left, field), index_sort_key(right, field)) {
                (None, None) => title_order(left, right),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(left_key), Some(right_key)) => left_key
                    .cmp(&right_key)
                    .then_with(|| title_order(left, right)),
            }
        });
    } else {
        ordered.sort_by(|left, right| title_order(left, right));
    }
    ordered
        .into_iter()
        .take(PEEK)
        .map(|record| record.title.clone())
        .collect()
}

fn title_order(
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

fn index_sort_key(record: &GroupingRecord, field: &str) -> Option<String> {
    Some(display_value(&field_text(record, field)?).to_lowercase())
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

fn display_value(value: &str) -> String {
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
        GroupingRecord {
            path: path.to_owned(),
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
        let grouping = group_records(&fields, None, &[], &records);
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
        let grouping = group_records(&fields, None, &[], &records);
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
        let grouping = group_records(&fields, None, &[], &records);
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
            &["location".to_owned(), "category".to_owned()],
            &[],
        );
        assert_eq!(grouping.index_facets, ["location", "category"]);
    }

    #[test]
    fn default_facets_skip_group_and_index_by() {
        let fields = vec![
            choices("status", &[("to_try", "To try")]),
            choices("kind", &[("bar", "Bar")]),
            text("location", true),
            text("category", true),
            text("quiet", false),
        ];
        let grouping = group_records(&fields, Some("location"), &[], &[]);
        assert_eq!(grouping.index_facets, ["kind", "category"]);
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
        let grouping = group_records(&fields, Some("location"), &[], &records);
        assert_eq!(grouping.groups[0].count, 13);
        assert_eq!(
            grouping.groups[0].peek,
            ["Place 06", "Place 07", "Place 08"]
        );
    }
}

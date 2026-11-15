//! Invocation-local subscription album snapshots and typed record parsing.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_yaml::{Mapping, Value};

use crate::native::{
    albums::registry::RecordTypeSource,
    projects::parse_frontmatter,
    records::{
        catalog::RecordCatalog,
        context::{RecordContext, RecordKindResolution},
        schema::{FieldKind, RecordScalar, RecordType, RecordValue},
    },
};

const NOISE_NEEDLES: &[&str] = &[
    "PURCHASE",
    "RECURRING",
    "PAYMENT",
    "RECURRING PAYMENT",
    "AUTHORIZED",
    "AUTHORIZED ON",
    "CARD",
    "POS",
    "ACH",
    "DEBIT",
    "GOOGLE",
    "APPLE",
    "PAYPAL",
    "AMAZON",
    "SQ",
    "TST",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionAvailability {
    Missing,
    Available,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionAlbum {
    pub(crate) stem: String,
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) directory: String,
    pub(crate) emoji: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionDiagnostic {
    pub(crate) code: &'static str,
    pub(crate) path: String,
    pub(crate) field: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionRegistry {
    pub(crate) availability: SubscriptionAvailability,
    pub(crate) album: Option<SubscriptionAlbum>,
    pub(crate) records: Vec<SubscriptionRecord>,
    pub(crate) diagnostics: Vec<SubscriptionDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionStatus {
    ToCancel,
    ToDecide,
    Active,
    Cancelled,
}

impl SubscriptionStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ToCancel => "to_cancel",
            Self::ToDecide => "to_decide",
            Self::Active => "active",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_cancelled(self) -> bool {
        self == Self::Cancelled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionCadence {
    Monthly,
    FourWeekly,
    Quarterly,
    Yearly,
}

impl SubscriptionCadence {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Monthly => "monthly",
            Self::FourWeekly => "four_weekly",
            Self::Quarterly => "quarterly",
            Self::Yearly => "yearly",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionPricing {
    Fixed,
    Variable,
}

impl SubscriptionPricing {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Variable => "variable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NeedleInvalidReason {
    TooShort,
    DigitRun,
    NoisePhrase,
}

impl NeedleInvalidReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TooShort => "fewer than three characters",
            Self::DigitRun => "contains a run of four or more ASCII digits",
            Self::NoisePhrase => "is a reserved statement-noise phrase",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidNeedle {
    pub(crate) text: String,
    pub(crate) reason: NeedleInvalidReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionRecord {
    pub(crate) stem: String,
    /// Vault-relative Markdown path, always including `.md`.
    pub(crate) path: String,
    /// Canonical vault link target, without `.md`.
    pub(crate) link_target: String,
    pub(crate) title: String,
    pub(crate) status: SubscriptionStatus,
    pub(crate) status_label: String,
    pub(crate) amount_cents: i64,
    pub(crate) cadence: SubscriptionCadence,
    pub(crate) pricing: SubscriptionPricing,
    pub(crate) needles: Vec<String>,
    pub(crate) invalid_needles: Vec<InvalidNeedle>,
    pub(crate) category: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) renews: Option<NaiveDate>,
    pub(crate) ended: Option<NaiveDate>,
}

impl SubscriptionRegistry {
    pub(crate) fn missing() -> Self {
        Self {
            availability: SubscriptionAvailability::Missing,
            album: None,
            records: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn invalid(
        album: Option<SubscriptionAlbum>,
        diagnostics: Vec<SubscriptionDiagnostic>,
    ) -> Self {
        Self {
            availability: SubscriptionAvailability::Invalid,
            album,
            records: Vec::new(),
            diagnostics,
        }
    }

    /// Invalid snapshot without album details for a guarded import that
    /// cannot resolve the subscription declaration. Imports proceed without
    /// subscription facts with one bounded warning.
    pub(crate) fn missing_invalid() -> Self {
        Self::invalid(
            None,
            vec![SubscriptionDiagnostic {
                code: "bank_subscription_album_invalid",
                path: String::new(),
                field: None,
                message:
                    "subscription album declaration is invalid or ambiguous"
                        .to_owned(),
            }],
        )
    }

    /// Read the noun-bound subscription album once for this invocation.
    /// Semantic record parsing is shared with `parse_subscription_snapshot`,
    /// which lets the import phase reuse the exact same parser on guarded bytes.
    pub(crate) fn load(vault_root: &Path) -> Self {
        let context = RecordContext::discover(vault_root);
        let resolution = context.lookup_noun("subscription");
        let kind = match resolution {
            RecordKindResolution::Unclaimed { .. } => {
                return Self::missing();
            }
            RecordKindResolution::Valid { kind, .. }
                if kind.record_type().id == "subscription" =>
            {
                kind
            }
            // `lookup_noun` also accepts a folder stem. Do not mistake an
            // unrelated record kind named `subscription` for this album.
            RecordKindResolution::Valid { .. } => {
                return Self::missing();
            }
            RecordKindResolution::Invalid { paths, .. } => {
                let diagnostics = paths
                    .into_iter()
                    .map(|path| SubscriptionDiagnostic {
                        code: "bank_subscription_album_invalid",
                        path,
                        field: None,
                        message: "subscription album declaration is invalid or ambiguous".to_owned(),
                    })
                    .collect();
                return Self::invalid(None, diagnostics);
            }
            RecordKindResolution::DiscoveryIncomplete { errors } => {
                let diagnostics = errors
                    .into_iter()
                    .map(|error| SubscriptionDiagnostic {
                        code: "bank_subscription_album_invalid",
                        path: "".to_owned(),
                        field: None,
                        message: format!(
                            "subscription registry discovery failed: {error}"
                        ),
                    })
                    .collect();
                return Self::invalid(None, diagnostics);
            }
        };

        if let crate::native::albums::registry::DiscoveryHealth::Partial(
            errors,
        ) = &context.registry().health
        {
            return Self::invalid(
                None,
                errors
                    .iter()
                    .map(|error| SubscriptionDiagnostic {
                        code: "bank_subscription_album_invalid",
                        path: "".to_owned(),
                        field: None,
                        message: format!(
                            "subscription registry discovery failed: {error}"
                        ),
                    })
                    .collect(),
            );
        }

        let record_type = kind.record_type();
        let RecordTypeSource::Album { path } = &kind.sourced.source;
        let album = SubscriptionAlbum {
            stem: kind
                .album
                .as_ref()
                .map(|identity| identity.stem.clone())
                .unwrap_or_else(|| {
                    Path::new(path)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or_default()
                        .to_owned()
                }),
            path: path.clone(),
            title: kind
                .album
                .as_ref()
                .map(|identity| identity.title.clone())
                .unwrap_or_else(|| record_type.label.clone()),
            directory: record_type.dir.clone(),
            emoji: record_type.emoji.clone(),
        };

        let schema_diagnostics =
            validate_subscription_schema(&album, record_type);
        if !schema_diagnostics.is_empty() {
            return Self::invalid(Some(album), schema_diagnostics);
        }

        let mut catalog = RecordCatalog::new(vault_root);
        let folder = catalog.folder(&album.directory);
        if let Some(message) = folder.create_refusal() {
            return Self::invalid(
                Some(album.clone()),
                vec![SubscriptionDiagnostic {
                    code: "bank_subscription_album_invalid",
                    path: album.directory.clone(),
                    field: None,
                    message: format!(
                        "subscription records folder is unreadable: {message}"
                    ),
                }],
            );
        }
        if folder.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == crate::native::records::schema::RecordDiagnosticCode::NoteRead
        }) {
            return Self::invalid(
                Some(album.clone()),
                vec![SubscriptionDiagnostic {
                    code: "bank_subscription_album_invalid",
                    path: album.directory.clone(),
                    field: None,
                    message: "subscription records folder could not be read completely".to_owned(),
                }],
            );
        }

        let mut record_bytes = Vec::new();
        for indexed in folder.notes() {
            if indexed.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == crate::native::records::schema::RecordDiagnosticCode::NoteRead
            }) {
                return Self::invalid(
                    Some(album.clone()),
                    vec![SubscriptionDiagnostic {
                        code: "bank_subscription_album_invalid",
                        path: display_path(&indexed.relative_path),
                        field: None,
                        message: "a subscription record could not be read".to_owned(),
                    }],
                );
            }
            let bytes = match fs::read(vault_root.join(&indexed.relative_path))
            {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Self::invalid(
                        Some(album.clone()),
                        vec![SubscriptionDiagnostic {
                            code: "bank_subscription_album_invalid",
                            path: display_path(&indexed.relative_path),
                            field: None,
                            message: "a subscription record could not be read"
                                .to_owned(),
                        }],
                    );
                }
            };
            if std::str::from_utf8(&bytes).is_err() {
                return Self::invalid(
                    Some(album.clone()),
                    vec![SubscriptionDiagnostic {
                        code: "bank_subscription_album_invalid",
                        path: display_path(&indexed.relative_path),
                        field: None,
                        message: "a subscription record is not readable UTF-8"
                            .to_owned(),
                    }],
                );
            }
            record_bytes.push((
                indexed.relative_path.clone(),
                indexed.stem.clone(),
                bytes,
            ));
        }

        parse_subscription_snapshot(album, record_type, &record_bytes)
    }
}

/// Purely parse a prepared set of record files using the resolved album schema.
/// Invalid records are isolated; invalid album schemas and unreadable bytes
/// disable the whole snapshot.
pub(crate) fn parse_subscription_snapshot(
    album: SubscriptionAlbum,
    schema: &RecordType,
    records: &[(PathBuf, String, Vec<u8>)],
) -> SubscriptionRegistry {
    let schema_diagnostics = validate_subscription_schema(&album, schema);
    if !schema_diagnostics.is_empty() {
        return SubscriptionRegistry::invalid(Some(album), schema_diagnostics);
    }

    let mut parsed = Vec::new();
    let mut diagnostics = Vec::new();
    let mut stem_counts = HashMap::<String, usize>::new();
    for (_, stem, _) in records {
        *stem_counts.entry(stem.to_lowercase()).or_default() += 1;
    }

    for (path, stem, bytes) in records {
        if std::str::from_utf8(bytes).is_err() {
            return SubscriptionRegistry::invalid(
                Some(album.clone()),
                vec![SubscriptionDiagnostic {
                    code: "bank_subscription_album_invalid",
                    path: display_path(path),
                    field: None,
                    message: "a subscription record is not readable UTF-8"
                        .to_owned(),
                }],
            );
        }
        let (record, mut record_diagnostics) =
            parse_subscription_record(path, stem, bytes, schema);
        diagnostics.append(&mut record_diagnostics);
        if stem_counts
            .get(&stem.to_lowercase())
            .is_some_and(|count| *count > 1)
        {
            diagnostics.push(SubscriptionDiagnostic {
                code: "bank_subscription_record_invalid",
                path: display_path(path),
                field: None,
                message:
                    "record stem is ambiguous within the subscription album"
                        .to_owned(),
            });
            continue;
        }
        if let Some(record) = record {
            parsed.push(record);
        }
    }
    parsed.sort_by(|left, right| left.path.cmp(&right.path));
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.field.cmp(&right.field))
            .then_with(|| left.code.cmp(right.code))
    });

    SubscriptionRegistry {
        availability: SubscriptionAvailability::Available,
        album: Some(album),
        records: parsed,
        diagnostics,
    }
}

fn parse_subscription_record(
    relative_path: &Path,
    stem: &str,
    bytes: &[u8],
    schema: &RecordType,
) -> (Option<SubscriptionRecord>, Vec<SubscriptionDiagnostic>) {
    let path = display_path(relative_path);
    let mut diagnostics = Vec::new();
    let fail = |field: Option<&str>, message: &str| SubscriptionDiagnostic {
        code: "bank_subscription_record_invalid",
        path: path.clone(),
        field: field.map(str::to_owned),
        message: message.to_owned(),
    };
    let contents = match std::str::from_utf8(bytes) {
        Ok(contents) => contents,
        Err(_) => {
            diagnostics.push(SubscriptionDiagnostic {
                code: "bank_subscription_album_invalid",
                path,
                field: None,
                message: "a subscription record is not readable UTF-8"
                    .to_owned(),
            });
            return (None, diagnostics);
        }
    };
    let Some(frontmatter) = parse_frontmatter(contents) else {
        diagnostics.push(fail(None, "record has no complete YAML frontmatter"));
        return (None, diagnostics);
    };
    let yaml = frontmatter.lines.join("\n");
    let value = match serde_yaml::from_str::<Value>(&yaml) {
        Ok(value) => value,
        Err(_) => {
            diagnostics.push(fail(None, "record frontmatter is invalid YAML"));
            return (None, diagnostics);
        }
    };
    let Some(mapping) = value.as_mapping() else {
        diagnostics.push(fail(None, "record frontmatter must be a mapping"));
        return (None, diagnostics);
    };

    let title = match mapping_get(mapping, "title").and_then(Value::as_str) {
        Some(title) if !title.trim().is_empty() => title.to_owned(),
        _ => {
            diagnostics.push(fail(
                Some("title"),
                "record title is missing or invalid",
            ));
            return (None, diagnostics);
        }
    };

    let status_value = match required_string(mapping, "status") {
        Some(value) => value,
        None => {
            diagnostics.push(fail(
                Some("status"),
                "required status is missing or invalid",
            ));
            return (None, diagnostics);
        }
    };
    let status = match status_value.as_str() {
        "to_cancel" => SubscriptionStatus::ToCancel,
        "to_decide" => SubscriptionStatus::ToDecide,
        "active" => SubscriptionStatus::Active,
        "cancelled" => SubscriptionStatus::Cancelled,
        _ => {
            diagnostics.push(fail(
                Some("status"),
                "status is not a supported subscription status",
            ));
            return (None, diagnostics);
        }
    };
    let status_label = match schema.field("status").map(|field| &field.kind) {
        Some(FieldKind::Choices(choices)) => choices
            .iter()
            .find(|choice| choice.value == status.as_str())
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| status.as_str().to_owned()),
        _ => status.as_str().to_owned(),
    };

    let amount_value = match mapping_get(mapping, "amount") {
        Some(Value::Number(_)) => exact_amount_cents(
            top_level_scalar_lexeme(&yaml, "amount").as_deref(),
        ),
        Some(_) => Err(MoneyParseError::InvalidDecimal),
        None => Err(MoneyParseError::InvalidDecimal),
    };
    let amount_cents = match amount_value {
        Ok(cents) => cents,
        Err(error) => {
            diagnostics.push(fail(Some("amount"), error.diagnostic()));
            return (None, diagnostics);
        }
    };

    let cadence = match required_string(mapping, "cadence")
        .as_deref()
        .and_then(parse_cadence)
    {
        Some(cadence) => cadence,
        None => {
            diagnostics.push(fail(
                Some("cadence"),
                "required cadence is missing or invalid",
            ));
            return (None, diagnostics);
        }
    };
    let pricing = match mapping_get(mapping, "pricing") {
        None => SubscriptionPricing::Fixed,
        Some(Value::String(value)) if value == "fixed" => {
            SubscriptionPricing::Fixed
        }
        Some(Value::String(value)) if value == "variable" => {
            SubscriptionPricing::Variable
        }
        Some(_) => {
            diagnostics.push(fail(
                Some("pricing"),
                "pricing must be fixed or variable when present",
            ));
            return (None, diagnostics);
        }
    };

    let mut needles = Vec::new();
    let mut invalid_needles = Vec::new();
    if let Some(value) = mapping_get(mapping, "match") {
        match value {
            Value::Sequence(items) => {
                let mut seen = std::collections::HashSet::new();
                for item in items {
                    let Some(text) = item.as_str() else {
                        diagnostics.push(SubscriptionDiagnostic {
                            code: "bank_subscription_match_invalid",
                            path: path.clone(),
                            field: Some("match".to_owned()),
                            message: "match list item is not text and was ignored".to_owned(),
                        });
                        continue;
                    };
                    let trimmed = text.trim();
                    if let Some(reason) = validate_needle(trimmed) {
                        let invalid = InvalidNeedle {
                            text: trimmed.to_owned(),
                            reason,
                        };
                        diagnostics.push(SubscriptionDiagnostic {
                            code: "bank_subscription_match_invalid",
                            path: path.clone(),
                            field: Some("match".to_owned()),
                            message: format!("match needle {} and was ignored", reason.as_str()),
                        });
                        invalid_needles.push(invalid);
                        continue;
                    }
                    if seen.insert(trimmed.to_lowercase()) {
                        needles.push(trimmed.to_owned());
                    }
                }
            }
            Value::Null => diagnostics.push(fail(
                Some("match"),
                "match must be a list when present; use an empty list for a matchless record",
            )),
            _ => diagnostics.push(fail(
                Some("match"),
                "match must be a list when present; use an empty list for a matchless record",
            )),
        }
    }

    let category = optional_text(mapping, "category", &path, &mut diagnostics);
    let url = optional_url(mapping, &path, &mut diagnostics);
    let renews = optional_date(mapping, "renews", &path, &mut diagnostics);
    let ended = optional_date(mapping, "ended", &path, &mut diagnostics);

    let mut link_path = relative_path.to_path_buf();
    link_path.set_extension("");
    let link_target = display_path(&link_path);
    (
        Some(SubscriptionRecord {
            stem: stem.to_owned(),
            path,
            link_target,
            title,
            status,
            status_label,
            amount_cents,
            cadence,
            pricing,
            needles,
            invalid_needles,
            category,
            url,
            renews,
            ended,
        }),
        diagnostics,
    )
}

fn validate_subscription_schema(
    album: &SubscriptionAlbum,
    schema: &RecordType,
) -> Vec<SubscriptionDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut invalid = |field: &str, message: String| {
        diagnostics.push(SubscriptionDiagnostic {
            code: "bank_subscription_album_invalid",
            path: album.path.clone(),
            field: Some(field.to_owned()),
            message,
        });
    };
    if schema.id != "subscription" {
        invalid("record.noun", "album noun is not subscription".to_owned());
    }
    require_schema_field(
        schema,
        "status",
        true,
        false,
        |kind| matches!(kind, FieldKind::Choices(choices) if ["to_cancel", "to_decide", "active", "cancelled"].iter().all(|value| choices.iter().any(|choice| choice.value == *value))),
        &mut invalid,
    );
    require_schema_field(
        schema,
        "amount",
        true,
        false,
        |kind| matches!(kind, FieldKind::Number),
        &mut invalid,
    );
    require_schema_field(
        schema,
        "cadence",
        true,
        false,
        |kind| matches!(kind, FieldKind::Choices(choices) if ["monthly", "four_weekly", "quarterly", "yearly"].iter().all(|value| choices.iter().any(|choice| choice.value == *value))),
        &mut invalid,
    );
    require_schema_field(
        schema,
        "pricing",
        false,
        false,
        |kind| matches!(kind, FieldKind::Choices(choices) if ["fixed", "variable"].iter().all(|value| choices.iter().any(|choice| choice.value == *value))),
        &mut invalid,
    );
    if let Some(field) = schema.field("pricing")
        && field.default.as_ref().is_some_and(|default| {
            !matches!(default, RecordValue::Scalar(RecordScalar::Choice(value)) if value == "fixed")
        })
    {
        invalid("pricing", "pricing schema default must be fixed".to_owned());
    }
    require_schema_field(
        schema,
        "match",
        false,
        true,
        |kind| matches!(kind, FieldKind::Text),
        &mut invalid,
    );
    for (field_name, expected) in [
        ("category", "text"),
        ("url", "url"),
        ("renews", "date"),
        ("ended", "date"),
    ] {
        if let Some(field) = schema.field(field_name)
            && field.kind.kind_name() != expected
        {
            invalid(
                field_name,
                format!("{field_name} field must use {expected} values"),
            );
        }
    }
    diagnostics
}

fn require_schema_field(
    schema: &RecordType,
    name: &str,
    required: bool,
    list: bool,
    kind_matches: impl FnOnce(&FieldKind) -> bool,
    invalid: &mut impl FnMut(&str, String),
) {
    let Some(field) = schema.field(name) else {
        invalid(name, format!("subscription schema is missing {name}"));
        return;
    };
    if field.required != required
        || field.list != list
        || !kind_matches(&field.kind)
    {
        invalid(name, format!("subscription schema field {name} has an incompatible definition"));
    }
}

fn optional_text(
    mapping: &Mapping,
    name: &str,
    path: &str,
    diagnostics: &mut Vec<SubscriptionDiagnostic>,
) -> Option<String> {
    let value = mapping_get(mapping, name)?;
    match value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => Some(value.to_owned()),
        None => {
            diagnostics.push(SubscriptionDiagnostic {
                code: "bank_subscription_record_invalid",
                path: path.to_owned(),
                field: Some(name.to_owned()),
                message: format!(
                    "optional {name} value is invalid and was omitted"
                ),
            });
            None
        }
    }
}

fn optional_url(
    mapping: &Mapping,
    path: &str,
    diagnostics: &mut Vec<SubscriptionDiagnostic>,
) -> Option<String> {
    let raw = optional_text(mapping, "url", path, diagnostics)?;
    if url::Url::parse(&raw).is_ok() {
        Some(raw)
    } else {
        diagnostics.push(SubscriptionDiagnostic {
            code: "bank_subscription_record_invalid",
            path: path.to_owned(),
            field: Some("url".to_owned()),
            message: "optional url is invalid and was omitted".to_owned(),
        });
        None
    }
}

fn optional_date(
    mapping: &Mapping,
    name: &str,
    path: &str,
    diagnostics: &mut Vec<SubscriptionDiagnostic>,
) -> Option<NaiveDate> {
    let value = mapping_get(mapping, name)?;
    let parsed = value.as_str().and_then(|value| {
        NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
    });
    match parsed {
        Some(date) => Some(date),
        None => {
            diagnostics.push(SubscriptionDiagnostic {
                code: "bank_subscription_record_invalid",
                path: path.to_owned(),
                field: Some(name.to_owned()),
                message: format!(
                    "optional {name} date is invalid and was omitted"
                ),
            });
            None
        }
    }
}

fn required_string(mapping: &Mapping, key: &str) -> Option<String> {
    mapping_get(mapping, key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn parse_cadence(value: &str) -> Option<SubscriptionCadence> {
    match value {
        "monthly" => Some(SubscriptionCadence::Monthly),
        "four_weekly" => Some(SubscriptionCadence::FourWeekly),
        "quarterly" => Some(SubscriptionCadence::Quarterly),
        "yearly" => Some(SubscriptionCadence::Yearly),
        _ => None,
    }
}

pub(crate) fn validate_needle(value: &str) -> Option<NeedleInvalidReason> {
    if value.chars().count() < 3 {
        return Some(NeedleInvalidReason::TooShort);
    }
    let mut digit_run = 0;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            digit_run += 1;
            if digit_run >= 4 {
                return Some(NeedleInvalidReason::DigitRun);
            }
        } else {
            digit_run = 0;
        }
    }
    let folded = value.to_lowercase();
    if NOISE_NEEDLES
        .iter()
        .any(|noise| folded == noise.to_lowercase())
    {
        return Some(NeedleInvalidReason::NoisePhrase);
    }
    None
}

fn top_level_scalar_lexeme(yaml: &str, wanted_key: &str) -> Option<String> {
    for line in yaml.lines() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['\'', '"']);
        if key != wanted_key {
            continue;
        }
        let raw = strip_yaml_comment(raw_value).trim();
        if raw.is_empty() {
            return None;
        }
        return Some(raw.to_owned());
    }
    None
}

fn strip_yaml_comment(value: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let chars = value.char_indices().collect::<Vec<_>>();
    for (index, ch) in chars.iter().copied() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('"'), '\\') => escaped = true,
            (Some(current), ch) if current == ch => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, '#')
                if index == 0
                    || value
                        .get(..index)
                        .and_then(|prefix| prefix.chars().next_back())
                        .is_some_and(char::is_whitespace) =>
            {
                return value.get(..index).unwrap_or(value);
            }
            _ => {}
        }
    }
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoneyParseError {
    InvalidDecimal,
    Negative,
    FractionalCent,
    Overflow,
}

impl MoneyParseError {
    fn diagnostic(self) -> &'static str {
        match self {
            Self::InvalidDecimal => {
                "amount is missing or is not a decimal number"
            }
            Self::Negative => "amount cannot be negative",
            Self::FractionalCent => {
                "amount has a fractional cent and was rejected"
            }
            Self::Overflow => {
                "amount is outside the supported integer-cent range"
            }
        }
    }
}

/// Convert a decimal spelling to cents using integer arithmetic only.
fn exact_amount_cents(value: Option<&str>) -> Result<i64, MoneyParseError> {
    let value = value.ok_or(MoneyParseError::InvalidDecimal)?.trim();
    let (negative, unsigned) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    if negative {
        return Err(MoneyParseError::Negative);
    }
    let unsigned = unsigned.replace('_', "");
    let mut exponent_parts = unsigned.split(['e', 'E']);
    let mantissa = exponent_parts.next().unwrap_or_default();
    let exponent = match exponent_parts.next() {
        Some(value) if exponent_parts.next().is_none() => value
            .parse::<i32>()
            .map_err(|_| MoneyParseError::Overflow)?,
        Some(_) => return Err(MoneyParseError::InvalidDecimal),
        None => 0,
    };
    let mut decimal_parts = mantissa.split('.');
    let whole = decimal_parts.next().unwrap_or_default();
    let fraction = decimal_parts.next().unwrap_or_default();
    if decimal_parts.next().is_some()
        || (whole.is_empty() && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(MoneyParseError::InvalidDecimal);
    }
    let digits =
        format!("{}{}", if whole.is_empty() { "0" } else { whole }, fraction);
    let coefficient = digits
        .parse::<u128>()
        .map_err(|_| MoneyParseError::Overflow)?;
    let scale = i64::try_from(fraction.len())
        .map_err(|_| MoneyParseError::Overflow)?
        .checked_sub(i64::from(exponent))
        .ok_or(MoneyParseError::Overflow)?;
    let shift = 2i64.checked_sub(scale).ok_or(MoneyParseError::Overflow)?;
    let cents = if shift >= 0 {
        if shift > 38 {
            return if coefficient == 0 {
                Ok(0)
            } else {
                Err(MoneyParseError::Overflow)
            };
        }
        let mut result = coefficient;
        for _ in 0..shift {
            result = result.checked_mul(10).ok_or(MoneyParseError::Overflow)?;
        }
        result
    } else {
        let divisor_shift =
            usize::try_from(-shift).map_err(|_| MoneyParseError::Overflow)?;
        if divisor_shift > 38 {
            if coefficient == 0 {
                0
            } else {
                return Err(MoneyParseError::FractionalCent);
            }
        } else {
            let divisor = 10u128.pow(divisor_shift as u32);
            if coefficient % divisor != 0 {
                return Err(MoneyParseError::FractionalCent);
            }
            coefficient / divisor
        }
    };
    i64::try_from(cents).map_err(|_| MoneyParseError::Overflow)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;
    use crate::native::records::schema::{RecordChoice, RecordField};

    fn choice_field(
        key: &str,
        required: bool,
        values: &[(&str, &str)],
    ) -> RecordField {
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
            required,
            list: false,
            default: None,
            suggest: false,
            min: None,
            max: None,
        }
    }

    fn field(
        key: &str,
        kind: FieldKind,
        required: bool,
        list: bool,
    ) -> RecordField {
        RecordField {
            key: key.to_owned(),
            label: key.to_owned(),
            kind,
            required,
            list,
            default: None,
            suggest: false,
            min: None,
            max: None,
        }
    }

    fn schema() -> RecordType {
        let mut pricing = choice_field(
            "pricing",
            false,
            &[("fixed", "📌 Fixed"), ("variable", "📈 Usage")],
        );
        pricing.default = Some(RecordValue::Scalar(RecordScalar::Choice(
            "fixed".to_owned(),
        )));
        RecordType {
            id: "subscription".to_owned(),
            dir: "billing".to_owned(),
            label: "Subscription".to_owned(),
            emoji: Some("🔁".to_owned()),
            parent: None,
            dated_notes: true,
            fields: vec![
                choice_field(
                    "status",
                    true,
                    &[
                        ("to_cancel", "✂️ To cancel"),
                        ("to_decide", "🤔 To decide"),
                        ("active", "💚 Active"),
                        ("cancelled", "🚫 Cancelled"),
                    ],
                ),
                field("amount", FieldKind::Number, true, false),
                choice_field(
                    "cadence",
                    true,
                    &[
                        ("monthly", "Monthly"),
                        ("four_weekly", "Every four weeks"),
                        ("quarterly", "Quarterly"),
                        ("yearly", "Yearly"),
                    ],
                ),
                pricing,
                field("match", FieldKind::Text, false, true),
                field("category", FieldKind::Text, false, false),
                field("url", FieldKind::Url, false, false),
                field("renews", FieldKind::Date, false, false),
                field("ended", FieldKind::Date, false, false),
            ],
            legacy: Vec::new(),
        }
    }

    fn album() -> SubscriptionAlbum {
        SubscriptionAlbum {
            stem: "billing".to_owned(),
            path: "billing.md".to_owned(),
            title: "Subscriptions".to_owned(),
            directory: "billing".to_owned(),
            emoji: Some("🔁".to_owned()),
        }
    }

    fn markdown(fields: &str) -> Vec<u8> {
        format!("---\ntitle: Tunely\n{fields}---\n# Tunely\n").into_bytes()
    }

    fn parse(fields: &str) -> SubscriptionRegistry {
        let records = vec![(
            PathBuf::from("billing/tunely.md"),
            "tunely".to_owned(),
            markdown(fields),
        )];
        parse_subscription_snapshot(album(), &schema(), &records)
    }

    #[test]
    fn parses_exact_cents_defaults_and_optional_values() {
        let registry = parse(
            "status: active\namount: 12.30\ncadence: four_weekly\ncategory: media\nurl: https://example.invalid/manage\nrenews: 2026-12-01\nended: 2026-01-01\nmatch: [ Tunely, ' Tunely ' ]\n",
        );
        assert_eq!(registry.availability, SubscriptionAvailability::Available);
        let record = &registry.records[0];
        assert_eq!(record.amount_cents, 1230);
        assert_eq!(record.pricing, SubscriptionPricing::Fixed);
        assert_eq!(record.status_label, "💚 Active");
        assert_eq!(record.cadence, SubscriptionCadence::FourWeekly);
        assert_eq!(record.needles, ["Tunely"]);
        assert_eq!(record.category.as_deref(), Some("media"));
        assert_eq!(
            record.url.as_deref(),
            Some("https://example.invalid/manage")
        );
        assert_eq!(record.renews, NaiveDate::from_ymd_opt(2026, 12, 1));
        assert_eq!(record.ended, NaiveDate::from_ymd_opt(2026, 1, 1));
        assert_eq!(record.link_target, "billing/tunely");
    }

    #[test]
    fn rejects_negative_fractional_cent_and_overflowing_amounts() {
        for (amount, expected) in [
            ("-1.00", "cannot be negative"),
            ("1.001", "fractional cent"),
            ("92233720368547758.08", "integer-cent range"),
        ] {
            let registry = parse(&format!(
                "status: active\namount: {amount}\ncadence: monthly\n"
            ));
            assert!(registry.records.is_empty(), "amount={amount}");
            assert!(registry.diagnostics.iter().any(|diagnostic| {
                diagnostic.field.as_deref() == Some("amount")
                    && diagnostic.message.contains(expected)
            }));
        }
    }

    #[test]
    fn invalid_required_fields_isolate_the_record() {
        let good = markdown(
            "status: active\namount: 9.99\ncadence: monthly\npricing: variable\n",
        );
        let bad = b"---\ntitle: Cloudbox\nstatus: someday\namount: 9.99\ncadence: monthly\n---\n".to_vec();
        let records = vec![
            (PathBuf::from("billing/tunely.md"), "tunely".into(), good),
            (PathBuf::from("billing/cloudbox.md"), "cloudbox".into(), bad),
        ];
        let registry =
            parse_subscription_snapshot(album(), &schema(), &records);
        assert_eq!(registry.availability, SubscriptionAvailability::Available);
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.records[0].pricing, SubscriptionPricing::Variable);
        assert!(registry.diagnostics.iter().any(|diagnostic| {
            diagnostic.path == "billing/cloudbox.md"
                && diagnostic.field.as_deref() == Some("status")
        }));
    }

    #[test]
    fn optional_bad_dates_are_diagnosed_and_omitted() {
        let registry = parse(
            "status: active\namount: 9.99\ncadence: monthly\nrenews: 2026-02-30\nended: ''\n",
        );
        let record = &registry.records[0];
        assert_eq!(record.renews, None);
        assert_eq!(record.ended, None);
        assert_eq!(
            registry
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.field.as_deref()
                    == Some("renews")
                    || diagnostic.field.as_deref() == Some("ended"))
                .count(),
            2
        );
    }

    #[test]
    fn validates_each_needle_class_and_deduplicates_after_trimming() {
        let registry = parse(
            "status: active\namount: 9.99\ncadence: monthly\nmatch:\n  - A\n  - '1234'\n  - PURCHASE\n  - RECURRING\n  - PAYMENT\n  - 'RECURRING PAYMENT'\n  - AUTHORIZED\n  - 'AUTHORIZED ON'\n  - CARD\n  - POS\n  - ACH\n  - DEBIT\n  - GOOGLE\n  - APPLE\n  - PAYPAL\n  - AMAZON\n  - SQ\n  - TST\n  - ' Tunely '\n  - tunely\n  - 17\n",
        );
        let record = &registry.records[0];
        assert_eq!(record.needles, ["Tunely"]);
        assert_eq!(record.invalid_needles.len(), 18);
        assert!(registry.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "bank_subscription_match_invalid"
                && diagnostic.message.contains("four or more ASCII digits")
        }));
        assert!(registry.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "bank_subscription_match_invalid"
                && diagnostic.message.contains("reserved statement-noise")
        }));
    }

    #[test]
    fn exact_decimal_parser_handles_scale_and_rejects_fractional_cents() {
        assert_eq!(exact_amount_cents(Some("1.2300")), Ok(123));
        assert_eq!(exact_amount_cents(Some("12.3e1")), Ok(12300));
        assert_eq!(
            exact_amount_cents(Some("1.2301")),
            Err(MoneyParseError::FractionalCent)
        );
    }

    #[test]
    fn invalid_schema_disables_snapshot() {
        let mut invalid_schema = schema();
        invalid_schema.fields.retain(|field| field.key != "amount");
        let records = vec![];
        let registry =
            parse_subscription_snapshot(album(), &invalid_schema, &records);
        assert_eq!(registry.availability, SubscriptionAvailability::Invalid);
        assert!(registry.records.is_empty());
        assert!(registry.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("amount")
        }));
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn vault_album(stem: &str, noun: &str) -> String {
        format!(
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nemoji: 🔁\nrecord:\n  noun: {noun}\n  label: Subscription\n  fields:\n    status:\n      required: true\n      values:\n        to_cancel: To cancel\n        to_decide: To decide\n        active: Active\n        cancelled: Cancelled\n    amount:\n      required: true\n      values: number\n      min: 0\n    cadence:\n      required: true\n      values:\n        monthly: Monthly\n        four_weekly: Every four weeks\n        quarterly: Quarterly\n        yearly: Yearly\n    pricing:\n      default: fixed\n      values:\n        fixed: Fixed\n        variable: Variable\n    match: {{list: true}}\n    category: {{suggest: true}}\n    url: {{values: url}}\n    renews: {{values: date}}\n    ended: {{values: date}}\n---\n# {stem}\n"
        )
    }

    #[test]
    fn loader_resolves_subscription_noun_in_renamed_album() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join("area.md"),
            "---\ntype: [[area]]\n---\n# Work\n",
        );
        write(
            &temp.path().join("billing.md"),
            &vault_album("billing", "subscription"),
        );
        write(
            &temp.path().join("billing/tunely.md"),
            "---\ntitle: Tunely\nstatus: active\namount: 9.99\ncadence: monthly\nmatch: [TUNELY]\n---\n# Tunely\n",
        );
        let registry = SubscriptionRegistry::load(temp.path());
        assert_eq!(registry.availability, SubscriptionAvailability::Available);
        assert_eq!(registry.album.as_ref().unwrap().directory, "billing");
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.records[0].path, "billing/tunely.md");
    }

    #[test]
    fn loader_distinguishes_missing_invalid_and_unrelated_stem() {
        let missing = tempfile::tempdir().unwrap();
        assert_eq!(
            SubscriptionRegistry::load(missing.path()).availability,
            SubscriptionAvailability::Missing
        );

        let unrelated = tempfile::tempdir().unwrap();
        write(
            &unrelated.path().join("area.md"),
            "---\ntype: [[area]]\n---\n# Work\n",
        );
        write(
            &unrelated.path().join("subscription.md"),
            &vault_album("subscription", "dining"),
        );
        assert_eq!(
            SubscriptionRegistry::load(unrelated.path()).availability,
            SubscriptionAvailability::Missing,
            "a folder stem named subscription must not satisfy the noun lookup"
        );

        let invalid = tempfile::tempdir().unwrap();
        write(
            &invalid.path().join("area.md"),
            "---\ntype: [[area]]\n---\n# Work\n",
        );
        write(
            &invalid.path().join("billing.md"),
            "---\ntype: [[album]]\nparent: \"[[area]]\"\nrecord: {noun: subscription, fields: {amount: {values: [invalid]}}}\n---\n# Billing\n",
        );
        assert_eq!(
            SubscriptionRegistry::load(invalid.path()).availability,
            SubscriptionAvailability::Invalid
        );
    }
}

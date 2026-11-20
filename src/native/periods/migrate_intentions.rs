//! Intentions migration step for `bob periods migrate`.
//!
//! One dry-run-first, idempotent move of the old body shapes into
//! frontmatter: daily Headline/Reflection, week Intentions, quarter
//! Outcomes, year Theme + Outcomes, `## Reflection`, and the Review
//! opener. Placeholders and conflicts are reported, never migrated.
//! Emptied headings are removed, and so are legacy slot headings that
//! were already empty (old-scaffold leftovers); `## Review` and callouts
//! stay.

use chrono::NaiveDate;

use crate::native::intentions::{
    self,
    fields::FrontmatterState,
    legacy::{self, IntentionsKind},
    write::{apply_with_review, OwnedPatch, Patch, ReviewPatch},
    NoteSubject,
};
use crate::native::periods::calendar::PeriodId;
use crate::native::{daily_headline, markdown};

#[derive(Debug, Clone, Default)]
pub(crate) struct IntentionsOutcome {
    pub(crate) after: Option<String>,
    pub(crate) migrated: bool,
    pub(crate) placeholders: Vec<String>,
    pub(crate) conflicts: Vec<String>,
    pub(crate) skipped: Vec<String>,
}

/// Migrate one note's old body shapes into frontmatter.
///
/// Returns `after` when the file should be rewritten. Placeholder,
/// conflict, and nested/prose reports are per-field strings for the
/// caller to attribute to the note path. Idempotent: a second call on
/// the migrated bytes returns `after: None` with no new reports.
pub(crate) fn migrate_note_contents(
    contents: &str,
    subject: NoteSubject,
    today: NaiveDate,
) -> IntentionsOutcome {
    match subject {
        NoteSubject::Day(_) => migrate_day(contents, subject),
        NoteSubject::Period(id) => {
            if matches!(id, PeriodId::AllTime) {
                IntentionsOutcome::default()
            } else {
                migrate_period(contents, subject, id, today)
            }
        }
    }
}

fn is_placeholder(text: &str) -> bool {
    matches!(text.trim(), "NOT SET" | "TBD" | "TODO" | "-" | "")
}

fn intentions_equal(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

fn has_frontmatter_intentions(
    fields: &crate::native::intentions::fields::Fields,
) -> bool {
    fields.intentions.provenance
        == crate::native::intentions::fields::FieldProvenance::Frontmatter
        && fields.intentions.value.as_ref().is_some_and(|v| {
            !v.is_empty() && v.iter().any(|s| !s.trim().is_empty())
        })
}

fn has_frontmatter_string(
    present_value: Option<&String>,
    provenance: crate::native::intentions::fields::FieldProvenance,
) -> bool {
    provenance
        == crate::native::intentions::fields::FieldProvenance::Frontmatter
        && present_value.is_some_and(|v| !v.trim().is_empty())
}

// ---------------------------------------------------------------------------
// Days
// ---------------------------------------------------------------------------

fn migrate_day(contents: &str, subject: NoteSubject) -> IntentionsOutcome {
    let mut out = IntentionsOutcome::default();
    let fields = intentions::read(contents, subject);
    if matches!(
        fields.frontmatter_state,
        crate::native::intentions::fields::FrontmatterState::Malformed
    ) {
        return out;
    }
    let mut patch = OwnedPatch::default();
    let mut body = contents.to_owned();
    let mut changed_body = false;
    let mut changed_frontmatter = false;

    // Daily Headline -> intentions.
    if let Some(headline) = legacy::parse_daily_headline(contents) {
        if is_placeholder(&headline.text) {
            out.placeholders
                .push(format!("placeholder intention {:?}", headline.text));
        } else if has_frontmatter_intentions(&fields) {
            let existing = fields.intentions.value.clone().unwrap_or_default();
            if existing.len() == 1 && existing[0] == headline.text {
                // Same value already stored: drop the legacy paragraph.
                if let Some(removed) = remove_daily_paragraph(&body, "Headline")
                {
                    body = removed;
                    changed_body = true;
                    out.migrated = true;
                }
            } else {
                out.conflicts
                    .push("intentions already set in frontmatter".to_string());
            }
        } else {
            patch.intentions = Patch::Set(vec![headline.text.clone()]);
            changed_frontmatter = true;
            out.migrated = true;
        }
    }
    // Empty Headline slot removal (template leftovers, including future
    // dailies). An empty slot is not a Headline, so it needs its own
    // scan even when no headline parsed above.
    if let Some(removed) = remove_empty_daily_headline(&body) {
        body = removed;
        changed_body = true;
    }

    // Daily Reflection preamble -> reflection.
    if let Some(reflection) = legacy::reflection(IntentionsKind::Day, contents)
    {
        if has_frontmatter_string(
            fields.reflection.value.as_ref(),
            fields.reflection.provenance,
        ) {
            let existing = fields
                .reflection
                .value
                .as_deref()
                .unwrap_or_default()
                .trim();
            if existing == reflection.trim() {
                if let Some(removed) =
                    remove_daily_paragraph(&body, "Reflection")
                {
                    body = removed;
                    changed_body = true;
                    out.migrated = true;
                }
            } else {
                out.conflicts
                    .push("reflection already set in frontmatter".to_string());
            }
        } else {
            patch.reflection = Patch::Set(reflection);
            changed_frontmatter = true;
            out.migrated = true;
        }
    }

    if !changed_body && !changed_frontmatter {
        return out;
    }
    // Body edits first so the frontmatter writer preserves the new body.
    // Then remove the migrated paragraphs from the edited body.
    let mut final_body = body;
    if changed_frontmatter {
        // Remove the migrated paragraphs now that the patch holds them.
        if matches!(patch.intentions, Patch::Set(_))
            && let Some(removed) =
                remove_daily_paragraph(&final_body, "Headline")
        {
            final_body = removed;
            changed_body = true;
        }
        if matches!(patch.reflection, Patch::Set(_))
            && let Some(removed) =
                remove_daily_paragraph(&final_body, "Reflection")
        {
            final_body = removed;
            changed_body = true;
        }
    }
    let after_frontmatter =
        match crate::native::intentions::write::apply(&final_body, &patch) {
            Ok(after) => after,
            Err(_) => {
                out.conflicts.push("frontmatter write failed".to_string());
                out.after = None;
                out.migrated = false;
                return out;
            }
        };
    if after_frontmatter == contents {
        out.after = None;
        out.migrated = false;
        return out;
    }
    // If only an empty slot was removed, that still counts as a rewrite
    // for idempotency, but not as an intentions migration.
    out.after = Some(after_frontmatter);
    let _ = changed_body;
    out
}

// ---------------------------------------------------------------------------
// Periods
// ---------------------------------------------------------------------------

fn migrate_period(
    contents: &str,
    subject: NoteSubject,
    id: PeriodId,
    today: NaiveDate,
) -> IntentionsOutcome {
    let mut out = IntentionsOutcome::default();
    let fields = intentions::read(contents, subject);
    if matches!(fields.frontmatter_state, FrontmatterState::Malformed) {
        out.conflicts.push("malformed frontmatter".to_string());
        return out;
    }
    let sections = legacy::period_sections(contents);
    let mut patch = OwnedPatch::default();
    let mut review_patch = ReviewPatch::default();
    let mut sections_to_remove: Vec<String> = Vec::new();
    let mut new_review_body: Option<String> = None;

    // Intentions from Intentions / Outcomes / Theme + Outcomes.
    match id {
        PeriodId::Week { .. } => {
            if let Some(body) = sections.get("Intentions") {
                match parse_bullet_section(body) {
                    BulletParse::Empty => {}
                    BulletParse::Invalid(reason) => {
                        out.skipped.push(format!("Intentions: {reason}"));
                    }
                    BulletParse::Placeholders => {
                        out.placeholders.push(
                            "Intentions has placeholder items".to_string(),
                        );
                    }
                    BulletParse::Values(values) => {
                        if has_frontmatter_intentions(&fields) {
                            let existing = fields
                                .intentions
                                .value
                                .clone()
                                .unwrap_or_default();
                            if intentions_equal(&existing, &values) {
                                sections_to_remove
                                    .push("Intentions".to_string());
                                out.migrated = true;
                            } else {
                                out.conflicts.push(
                                    "intentions already set in frontmatter"
                                        .to_string(),
                                );
                            }
                        } else {
                            patch.intentions = Patch::Set(values);
                            sections_to_remove.push("Intentions".to_string());
                            out.migrated = true;
                        }
                    }
                }
            }
        }
        PeriodId::Quarter { .. } => {
            if let Some(body) = sections.get("Outcomes") {
                match parse_bullet_section(body) {
                    BulletParse::Empty => {}
                    BulletParse::Invalid(reason) => {
                        out.skipped.push(format!("Outcomes: {reason}"));
                    }
                    BulletParse::Placeholders => {
                        out.placeholders
                            .push("Outcomes has placeholder items".to_string());
                    }
                    BulletParse::Values(values) => {
                        if has_frontmatter_intentions(&fields) {
                            let existing = fields
                                .intentions
                                .value
                                .clone()
                                .unwrap_or_default();
                            if intentions_equal(&existing, &values) {
                                sections_to_remove.push("Outcomes".to_string());
                                out.migrated = true;
                            } else {
                                out.conflicts.push(
                                    "intentions already set in frontmatter"
                                        .to_string(),
                                );
                            }
                        } else {
                            patch.intentions = Patch::Set(values);
                            sections_to_remove.push("Outcomes".to_string());
                            out.migrated = true;
                        }
                    }
                }
            }
        }
        PeriodId::Year(_) => {
            let mut year_values: Vec<String> = Vec::new();
            let mut theme_ok = false;
            let mut outcomes_ok = false;
            if let Some(theme_body) = sections.get("Theme") {
                match parse_theme_paragraph(theme_body) {
                    ThemeParse::Empty => {}
                    ThemeParse::Invalid(reason) => {
                        out.skipped.push(format!("Theme: {reason}"));
                    }
                    ThemeParse::Placeholder => {
                        out.placeholders
                            .push("Theme is a placeholder".to_string());
                    }
                    ThemeParse::Value(value) => {
                        year_values.push(value);
                        theme_ok = true;
                    }
                }
            }
            if let Some(outcomes_body) = sections.get("Outcomes") {
                match parse_bullet_section(outcomes_body) {
                    BulletParse::Empty => {}
                    BulletParse::Invalid(reason) => {
                        out.skipped.push(format!("Outcomes: {reason}"));
                    }
                    BulletParse::Placeholders => {
                        out.placeholders
                            .push("Outcomes has placeholder items".to_string());
                    }
                    BulletParse::Values(mut values) => {
                        year_values.append(&mut values);
                        outcomes_ok = true;
                    }
                }
            }
            if !year_values.is_empty() {
                if has_frontmatter_intentions(&fields) {
                    let existing =
                        fields.intentions.value.clone().unwrap_or_default();
                    if intentions_equal(&existing, &year_values) {
                        if theme_ok {
                            sections_to_remove.push("Theme".to_string());
                        }
                        if outcomes_ok {
                            sections_to_remove.push("Outcomes".to_string());
                        }
                        out.migrated = true;
                    } else {
                        out.conflicts.push(
                            "intentions already set in frontmatter".to_string(),
                        );
                        // Do not remove either heading on conflict.
                        sections_to_remove.clear();
                    }
                } else if is_placeholder_list(&year_values) {
                    out.placeholders
                        .push("intentions are placeholders".to_string());
                } else {
                    patch.intentions = Patch::Set(year_values);
                    if theme_ok {
                        sections_to_remove.push("Theme".to_string());
                    }
                    if outcomes_ok {
                        sections_to_remove.push("Outcomes".to_string());
                    }
                    out.migrated = true;
                }
            } else if theme_ok || outcomes_ok {
                // Only reachable when values were placeholders; already
                // reported above.
            }
            // If Theme was invalid but Outcomes valid, Outcomes still
            // migrates (Theme heading stays). The code above already
            // handles that: year_values holds only the valid part and
            // only its heading is queued for removal.
            let _ = (theme_ok, outcomes_ok);
        }
        PeriodId::AllTime => return out,
    }

    // Reflection body -> reflection (verbatim).
    if let Some(body) = sections.get("Reflection") {
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            if has_frontmatter_string(
                fields.reflection.value.as_ref(),
                fields.reflection.provenance,
            ) {
                let existing = fields
                    .reflection
                    .value
                    .as_deref()
                    .unwrap_or_default()
                    .trim();
                if existing == trimmed {
                    sections_to_remove.push("Reflection".to_string());
                    out.migrated = true;
                } else {
                    out.conflicts.push(
                        "reflection already set in frontmatter".to_string(),
                    );
                }
            } else {
                patch.reflection = Patch::Set(trimmed.to_string());
                sections_to_remove.push("Reflection".to_string());
                out.migrated = true;
            }
        }
    }

    // Review opener -> summary.
    if let Some(review_body) = sections.get("Review")
        && let Some((summary, remaining)) = split_review_opener(review_body)
    {
        if has_frontmatter_string(
            fields.summary.value.as_ref(),
            fields.summary.provenance,
        ) {
            let existing =
                fields.summary.value.as_deref().unwrap_or_default().trim();
            if existing == summary.trim() {
                // Same summary already stored: drop the opener.
                new_review_body = Some(remaining);
                out.migrated = true;
            } else {
                out.conflicts
                    .push("summary already set in frontmatter".to_string());
            }
        } else {
            // Pre-migration state decides the digest writes.
            let has_review = !fields.review_body.trim().is_empty();
            let state =
                crate::native::intentions::state::evaluate(&fields, has_review);
            patch.summary = Patch::Set(summary.clone());
            if state == crate::native::intentions::state::ReviewState::Drafted {
                patch.summary_digest =
                    Patch::Set(crate::native::intentions::digest(&summary));
                let drafted =
                    fields.review_drafted.value.clone().unwrap_or_else(|| {
                        today.format("%Y-%m-%d").to_string()
                    });
                patch.summary_drafted = Patch::Set(drafted);
                if remaining.trim().is_empty() {
                    review_patch.review_digest = Patch::Unset;
                    review_patch.review_drafted = Patch::Unset;
                } else {
                    review_patch.review_digest = Patch::Set(
                        crate::native::intentions::digest(&remaining),
                    );
                }
            }
            new_review_body = Some(remaining);
            out.migrated = true;
        }
    }

    // Old scaffolds emitted empty legacy slot headings. They carry no
    // text, so they go even when nothing else migrates; the problems
    // report counts them as `legacy_period_text`.
    let empty_slots = legacy_slot_names(id);
    let has_empty_slots =
        remove_empty_sections(contents, empty_slots).is_some();
    if !out.migrated
        && sections_to_remove.is_empty()
        && new_review_body.is_none()
        && !has_empty_slots
    {
        return out;
    }
    // Body edits first.
    let mut intermediate = contents.to_owned();
    if let Some(remaining) = new_review_body.as_deref() {
        if let Some(replaced) = replace_review_body(&intermediate, remaining) {
            intermediate = replaced;
        } else {
            out.conflicts
                .push("could not rewrite Review body".to_string());
            out.after = None;
            out.migrated = false;
            return out;
        }
    }
    if !sections_to_remove.is_empty() {
        intermediate = remove_period_sections(
            &intermediate,
            &sections_to_remove
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
    }
    if let Some(removed) = remove_empty_sections(&intermediate, empty_slots) {
        intermediate = removed;
    }
    let after = match apply_with_review(&intermediate, &patch, &review_patch) {
        Ok(after) => after,
        Err(_) => {
            out.conflicts.push("frontmatter write failed".to_string());
            out.after = None;
            out.migrated = false;
            return out;
        }
    };
    if after == contents {
        out.after = None;
        out.migrated = false;
        return out;
    }
    out.after = Some(after);
    out
}

fn is_placeholder_list(values: &[String]) -> bool {
    !values.is_empty() && values.iter().all(|v| is_placeholder(v))
}

enum BulletParse {
    Empty,
    Invalid(String),
    Placeholders,
    Values(Vec<String>),
}

fn parse_bullet_section(body: &str) -> BulletParse {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return BulletParse::Empty;
    }
    let lines: Vec<&str> = body.lines().collect();
    if !markdown::fenced_lines(&lines, 0..lines.len()).is_empty() {
        return BulletParse::Invalid(
            "fenced block is left in place".to_string(),
        );
    }
    let mut values: Vec<String> = Vec::new();
    let mut active: Option<usize> = None;
    for line in &lines {
        let trimmed_start = line.trim_start();
        let indent = line.len() - trimmed_start.len();
        if trimmed_start.is_empty() {
            active = None;
            continue;
        }
        let is_bullet = indent == 0
            && (trimmed_start.starts_with("- ")
                || trimmed_start.starts_with("* ")
                || trimmed_start.starts_with("+ "));
        let is_indented_bullet = indent > 0
            && (trimmed_start.starts_with("- ")
                || trimmed_start.starts_with("* ")
                || trimmed_start.starts_with("+ "));
        if is_indented_bullet {
            return BulletParse::Invalid(
                "nested bullets are left in place".to_string(),
            );
        }
        if is_bullet {
            values.push(trimmed_start[2..].trim().to_string());
            active = Some(values.len() - 1);
        } else if indent > 0
            && let Some(which) = active
            && !trimmed_start.starts_with(['-', '*', '+'])
        {
            // Soft-wrapped continuation of the previous bullet.
            values[which].push(' ');
            values[which].push_str(trimmed_start);
        } else {
            return BulletParse::Invalid("prose is left in place".to_string());
        }
    }
    values.retain(|v| !v.trim().is_empty());
    if values.is_empty() {
        return BulletParse::Empty;
    }
    if values.iter().any(|v| is_placeholder(v)) {
        return BulletParse::Placeholders;
    }
    BulletParse::Values(values)
}

enum ThemeParse {
    Empty,
    Invalid(String),
    Placeholder,
    Value(String),
}

fn parse_theme_paragraph(body: &str) -> ThemeParse {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return ThemeParse::Empty;
    }
    let lines: Vec<&str> = body.lines().collect();
    if !markdown::fenced_lines(&lines, 0..lines.len()).is_empty() {
        return ThemeParse::Invalid(
            "fenced block is left in place".to_string(),
        );
    }
    // A theme is one paragraph: no bullets, no blank-line paragraph break.
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
            return ThemeParse::Invalid("prose is left in place".to_string());
        }
    }
    let paragraphs: Vec<&str> = trimmed.split("\n\n").collect();
    if paragraphs.len() > 1 {
        return ThemeParse::Invalid("prose is left in place".to_string());
    }
    let text = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.trim().is_empty() {
        return ThemeParse::Empty;
    }
    if is_placeholder(&text) {
        return ThemeParse::Placeholder;
    }
    ThemeParse::Value(text)
}

/// Split a Review body into its opening Headline/Summary paragraph and the
/// remaining body. Returns `None` when the Review does not open with one.
fn split_review_opener(body: &str) -> Option<(String, String)> {
    let mut lines = body.lines();
    let first = lines.next()?;
    // The legacy reader is case-insensitive and accepts both colon forms;
    // mirror it with a direct label check.
    let _ = label_value(first, "Headline")
        .or_else(|| label_value(first, "Summary"))?;
    let mut opener_lines: Vec<&str> = vec![first];
    let mut rest: Vec<&str> = Vec::new();
    let mut in_opener = true;
    for line in lines {
        if in_opener && !daily_headline::ends_headline_paragraph(line) {
            opener_lines.push(line);
        } else if in_opener && daily_headline::ends_headline_paragraph(line) {
            in_opener = false;
            rest.push(line);
        } else {
            rest.push(line);
        }
    }
    let opener_joined = opener_lines.join(" ");
    let summary = label_value(&opener_joined, "Headline")
        .or_else(|| label_value(&opener_joined, "Summary"))
        .unwrap_or_default()
        .trim()
        .to_string();
    if summary.is_empty() {
        return None;
    }
    let remaining = rest.join("\n").trim().to_string();
    Some((summary, remaining))
}

fn label_value<'a>(line: &'a str, wanted: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("**")?;
    let close = rest.find("**")?;
    let label = &rest[..close];
    let after = &rest[close + 2..];
    if label.eq_ignore_ascii_case(&format!("{wanted}:")) {
        Some(after)
    } else if label.eq_ignore_ascii_case(wanted) {
        after.strip_prefix(':')
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Body rewrites (line-preserving where cheap, LF-joined like the existing
// daily rewrite).
// ---------------------------------------------------------------------------

fn frontmatter_end(lines: &[String]) -> Option<usize> {
    if lines.first().map(|l| l.trim().to_string()).as_deref() != Some("---") {
        return None;
    }
    lines
        .iter()
        .skip(1)
        .position(|l| l.trim() == "---")
        .map(|i| i + 1)
}

fn remove_daily_paragraph(contents: &str, wanted: &str) -> Option<String> {
    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let fm_end = frontmatter_end(&lines);
    let start_search = fm_end.map(|i| i + 1).unwrap_or(0);
    // Skip fenced blocks when scanning the preamble.
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&refs, start_search..lines.len());
    let mut index = start_search;
    while index < lines.len() {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        let line = &lines[index];
        if markdown::atx_heading(line).is_some_and(|(level, _)| level >= 2) {
            break;
        }
        if label_value(line, wanted).is_some() {
            let mut end = index + 1;
            while end < lines.len()
                && !fenced.contains(&end)
                && !daily_headline::ends_headline_paragraph(&lines[end])
            {
                end += 1;
            }
            // A paragraph between two blank lines leaves one blank line,
            // not two.
            if index > 0
                && lines[index - 1].trim().is_empty()
                && lines.get(end).is_some_and(|l| l.trim().is_empty())
                && !fenced.contains(&end)
            {
                end += 1;
            }
            let mut out: Vec<String> = Vec::new();
            out.extend_from_slice(&lines[..index]);
            out.extend_from_slice(&lines[end..]);
            let mut joined = out.join("\n");
            if contents.ends_with('\n') && !joined.ends_with('\n') {
                joined.push('\n');
            }
            return Some(joined);
        }
        index += 1;
    }
    None
}

fn remove_empty_daily_headline(contents: &str) -> Option<String> {
    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let fm_end = frontmatter_end(&lines);
    let start_search = fm_end.map(|i| i + 1).unwrap_or(0);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&refs, start_search..lines.len());
    let mut index = start_search;
    while index < lines.len() {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        let line = &lines[index];
        if markdown::atx_heading(line).is_some_and(|(level, _)| level >= 2) {
            break;
        }
        if let Some(rest) = label_value(line, "Headline") {
            // Empty when the label line carries no words and the next
            // line ends the paragraph immediately.
            let next_ends = lines
                .get(index + 1)
                .map(|next| {
                    fenced.contains(&(index + 1))
                        || daily_headline::ends_headline_paragraph(next)
                })
                .unwrap_or(true);
            if rest.trim().is_empty() && next_ends {
                let mut out: Vec<String> = Vec::new();
                out.extend_from_slice(&lines[..index]);
                out.extend_from_slice(&lines[index + 1..]);
                let mut joined = out.join("\n");
                if contents.ends_with('\n') && !joined.ends_with('\n') {
                    joined.push('\n');
                }
                // Collapse a resulting double blank line at the removal
                // point to a single blank line.
                joined = collapse_double_blank(&joined);
                return Some(joined);
            }
            // A non-empty Headline is handled by the paragraph removal.
            // Skip its continuation lines so we do not mistake a later
            // empty slot for this one.
            let mut end = index + 1;
            while end < lines.len()
                && !fenced.contains(&end)
                && !daily_headline::ends_headline_paragraph(&lines[end])
            {
                end += 1;
            }
            index = end.max(index + 1);
        } else {
            index += 1;
        }
    }
    None
}

fn collapse_double_blank(text: &str) -> String {
    // Collapse three-or-more newlines to two (one blank line).
    let mut out = text.to_string();
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

/// The legacy slot headings a period kind's old scaffold carried.
fn legacy_slot_names(id: PeriodId) -> &'static [&'static str] {
    match id {
        PeriodId::Week { .. } => &["Intentions", "Reflection"],
        PeriodId::Quarter { .. } => &["Outcomes", "Reflection"],
        PeriodId::Year(_) => &["Theme", "Outcomes", "Reflection"],
        PeriodId::AllTime => &[],
    }
}

/// Index of the first line after the frontmatter (0 when there is none).
fn body_start(lines: &[String]) -> usize {
    frontmatter_end(lines).map_or(0, |end| end + 1)
}

/// Whether the period reader stops at this line: the agent callout or
/// the `## Archive` heading.
fn ends_period_sections(line: &str) -> bool {
    line.starts_with("> [!agent]")
        || line.trim() == "## Archive"
        || line.starts_with("## Archive ")
}

/// Remove each `## <name>` heading in `names` whose section holds only
/// blank lines. Only the reader's view is scanned: fenced lines never
/// count as headings and the scan stops at the agent callout or
/// `## Archive`. A section with any text keeps its heading. Returns
/// `None` when nothing was removed.
fn remove_empty_sections(contents: &str, names: &[&str]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let start = body_start(&lines);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&refs, start..lines.len());
    let mut skip = vec![false; lines.len()];
    let mut removed = false;
    let mut index = start;
    while index < lines.len() {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        let line = &lines[index];
        if ends_period_sections(line) {
            break;
        }
        let Some(name) = line.strip_prefix("## ") else {
            index += 1;
            continue;
        };
        if !names.contains(&name.trim()) {
            index += 1;
            continue;
        }
        let mut next = index + 1;
        while next < lines.len() && lines[next].trim().is_empty() {
            next += 1;
        }
        let empty = next == lines.len()
            || lines[next].starts_with("## ")
            || ends_period_sections(&lines[next]);
        if empty {
            for flag in &mut skip[index..next] {
                *flag = true;
            }
            removed = true;
        }
        index = next;
    }
    if !removed {
        return None;
    }
    let kept: Vec<String> = lines
        .into_iter()
        .enumerate()
        .filter_map(|(i, l)| (!skip[i]).then_some(l))
        .collect();
    let mut joined = kept.join("\n");
    if contents.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    Some(collapse_double_blank(&joined))
}

fn remove_period_sections(contents: &str, names: &[&str]) -> String {
    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let mut skip = vec![false; lines.len()];
    let mut index = body_start(&lines);
    // Match the reader: fenced lines are never headings, and nothing at
    // or below the agent callout or `## Archive` is a period section.
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&refs, index..lines.len());
    while index < lines.len() {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        if ends_period_sections(&lines[index]) {
            break;
        }
        if let Some(name) = lines[index].strip_prefix("## ") {
            let name = name.trim().to_string();
            if names.contains(&name.as_str()) {
                // Find the section limit: next ## heading, agent callout,
                // or Archive heading, else EOF.
                let mut limit = lines.len();
                for j in index + 1..lines.len() {
                    if !fenced.contains(&j)
                        && (lines[j].starts_with("## ")
                            || ends_period_sections(&lines[j]))
                    {
                        limit = j;
                        break;
                    }
                }
                for k in index..limit {
                    skip[k] = true;
                }
                index = limit;
                continue;
            }
        }
        index += 1;
    }
    let kept: Vec<String> = lines
        .into_iter()
        .enumerate()
        .filter_map(|(i, l)| (!skip[i]).then_some(l))
        .collect();
    let mut joined = kept.join("\n");
    if contents.ends_with('\n') {
        joined.push('\n');
    }
    collapse_double_blank(&joined)
}

fn replace_review_body(contents: &str, body: &str) -> Option<String> {
    let lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let heading = lines.iter().position(|l| l.trim_end() == "## Review")?;
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&refs, heading + 1..lines.len());
    let mut limit = lines.len();
    for (i, line) in lines.iter().enumerate().skip(heading + 1) {
        if fenced.contains(&i) {
            continue;
        }
        if line.starts_with("## ") || ends_period_sections(line) {
            limit = i;
            break;
        }
    }
    let mut start = limit;
    for i in heading + 1..limit {
        if !lines[i].trim().is_empty() {
            start = i;
            break;
        }
    }
    let mut end = start;
    for i in (heading + 1..limit).rev() {
        if !lines[i].trim().is_empty() {
            end = i + 1;
            break;
        }
    }
    let newline = "\n";
    let mut out = String::new();
    out.push_str(&lines[..start].join("\n"));
    // Ensure exactly one blank line after the heading before the body.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        // The heading line itself ends with one newline from join; add
        // the blank line.
        if out.trim_end_matches('\n').ends_with("## Review") {
            out.push('\n');
        } else if !out.is_empty() {
            out.push('\n');
        }
    }
    let _ = newline;
    if !body.trim().is_empty() {
        out.push_str(body.trim());
        out.push('\n');
    }
    if end < lines.len() {
        let suffix = lines[end..].join("\n");
        if !suffix.is_empty() {
            // Separate from the Review body with one blank line.
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(&suffix);
            if contents.ends_with('\n') && !out.ends_with('\n') {
                out.push('\n');
            }
        } else if contents.ends_with('\n') && !out.ends_with('\n') {
            out.push('\n');
        }
    } else if contents.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(collapse_double_blank(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str) -> NoteSubject {
        NoteSubject::Day(
            NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("date"),
        )
    }

    #[test]
    fn daily_headline_moves_to_frontmatter() {
        let before = "---\nparent: \"[[2026/2026-W47]]\"\n---\n\n**Headline:** Ship the thing [[bob#^x]]\n\n## Pomodoros\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, day("2026-11-19"), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        assert!(after.contains("intentions:"), "{after}");
        assert!(after.contains("Ship the thing"), "{after}");
        assert!(!after.contains("**Headline:**"), "{after}");
        assert!(!after.contains("\n\n\n"), "one blank line stays: {after:?}");
        // Idempotent.
        let rerun = migrate_note_contents(&after, day("2026-11-19"), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    #[test]
    fn adjacent_headline_and_reflection_leave_one_blank_line() {
        // The live 20261118 shape: both labels on consecutive lines.
        let before = "---\nid: 20261118\n---\n\n# 2026-11-18 Wed\n\n[[2026/20261117|prev]] · [[2026/2026-W47|W47]] · [[2026/20261119|next]]\n\n**Headline:** Add records\n**Reflection**: Tired today.\n\n- [x] #task [[gtd_daily]] ^gtd\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, day("2026-11-18"), today);
        let after = out.after.expect("migrates");
        assert!(after.contains("intentions:"), "{after}");
        assert!(after.contains("reflection: Tired today."), "{after}");
        assert!(
            after.contains("|next]]\n\n- [x] #task [[gtd_daily]] ^gtd\n"),
            "{after:?}"
        );
        let rerun = migrate_note_contents(&after, day("2026-11-18"), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    fn week_subject() -> NoteSubject {
        NoteSubject::Period(PeriodId::Week {
            year: 2026,
            week: 47,
        })
    }

    fn quarter_subject() -> NoteSubject {
        NoteSubject::Period(PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        })
    }

    fn year_subject() -> NoteSubject {
        NoteSubject::Period(PeriodId::Year(2026))
    }

    fn week_note(sections: &str) -> String {
        format!(
            "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W47\n---\n\n# 2026-W47\n\n```bob-stats-strip\n\n```\n\n{sections}\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\nOld review.\n"
        )
    }

    #[test]
    fn week_intentions_with_block_links_migrate() {
        let before = week_note(
            "## Intentions\n\n- Ship [[work#^a|the alias]]\n- Keep [[bob#^b]]\n- Third [[areas/work]]\n",
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        assert!(after.contains("intentions:"), "{after}");
        assert!(after.contains("Ship"), "{after}");
        assert!(!after.contains("## Intentions"), "{after}");
        assert!(after.contains("## Days"), "{after}");
        assert!(after.contains("## Review"), "{after}");
        let rerun = migrate_note_contents(&after, week_subject(), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    #[test]
    fn placeholder_intentions_are_reported_not_migrated() {
        let before = week_note("## Intentions\n\n- NOT SET\n");
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        assert!(out.after.is_none(), "{out:?}");
        assert!(!out.placeholders.is_empty(), "{out:?}");
    }

    #[test]
    fn nested_bullets_are_left_in_place() {
        let before =
            week_note("## Intentions\n\n- Ship it\n  - nested detail\n");
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        assert!(out.after.is_none(), "{out:?}");
        assert!(!out.skipped.is_empty(), "{out:?}");
    }

    #[test]
    fn conflict_with_frontmatter_is_reported() {
        let before = "---\nintentions:\n  - \"existing\"\n---\n\n# 2026-W47\n\n## Intentions\n\n- New value\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, week_subject(), today);
        assert!(out.after.is_none(), "{out:?}");
        assert!(!out.conflicts.is_empty(), "{out:?}");
    }

    #[test]
    fn quarter_outcomes_migrate() {
        let before = "---\nparent: \"[[2026]]\"\n---\n\n# 2026-Q4\n\n## Outcomes\n\n- First [[work#^a]]\n- Second thing\n- Third thing\n\n## Weeks\n\n```bob-period-parts\n\n```\n\n## Review\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, quarter_subject(), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        assert!(after.contains("intentions:"), "{after}");
        assert!(!after.contains("## Outcomes"), "{after}");
    }

    #[test]
    fn year_theme_then_outcomes_migrate_theme_first() {
        let before = "---\nparent: \"[[all_time]]\"\n---\n\n# 2026\n\n## Theme\n\nMake room for deep work\n\n## Outcomes\n\n- Ship the album\n- Keep the notes\n\n## Quarters\n\n```bob-period-parts\n\n```\n\n## Review\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, year_subject(), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        let theme_at = after.find("Make room").expect("theme");
        let ship_at = after.find("Ship the album").expect("outcome");
        assert!(theme_at < ship_at, "{after}");
        assert!(!after.contains("## Theme"), "{after}");
        assert!(!after.contains("## Outcomes"), "{after}");
    }

    #[test]
    fn reflection_body_migrates_verbatim() {
        let before = week_note(
            "## Reflection\n\nFirst [[work#^a]]\n\nSecond paragraph.\n",
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        assert!(after.contains("reflection:"), "{after}");
        assert!(!after.contains("## Reflection"), "{after}");
    }

    #[test]
    fn drafted_review_restamps_both_digests() {
        let summary = "A calm week";
        let review_body = format!("**Headline:** {summary}\n\nDid the work.\n");
        let review_digest = crate::native::intentions::digest(&review_body);
        let before = format!(
            "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W47\nreview_digest: {review_digest}\nreview_drafted: 2026-11-19\n---\n\n# 2026-W47\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\n{review_body}\n"
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        assert!(after.contains("summary:"), "{after}");
        assert!(after.contains("summary_digest:"), "{after}");
        assert!(
            after.contains("summary_drafted:") && after.contains("2026-11-19"),
            "{after}"
        );
        assert!(!after.contains("**Headline:**"), "{after}");
        // The remaining Review body stays drafted under the new digest.
        let fields = crate::native::intentions::read(&after, week_subject());
        assert_eq!(
            crate::native::intentions::state::evaluate(
                &fields,
                !fields.review_body.trim().is_empty()
            ),
            crate::native::intentions::state::ReviewState::Drafted,
            "{after}"
        );
    }

    #[test]
    fn empty_week_slots_from_the_old_scaffold_are_removed() {
        // The live old-scaffold week anatomy (W27-W45, W48).
        let before = "---\nparent: \"[[2026/2026-Q4]]\"\nid: 2026-W47\n---\n\n# 2026-W47\n\n```bob-stats-strip\n\n```\n\n## Intentions\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\nOld review.\n\n## Reflection\n\n> [!agent]- Agent instructions · weekly v2\n> text\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, week_subject(), today);
        let after = out.after.clone().expect("empty slots are removed");
        assert!(!after.contains("## Intentions"), "{after}");
        assert!(!after.contains("## Reflection"), "{after}");
        assert!(after.contains("## Days"), "{after}");
        assert!(after.contains("## Review\n\nOld review."), "{after}");
        assert!(after.contains("> [!agent]- Agent instructions"), "{after}");
        assert!(!after.contains("\n\n\n"), "{after}");
        assert!(!after.contains("intentions:"), "{after}");
        assert!(!after.contains("reflection:"), "{after}");
        assert!(out.placeholders.is_empty(), "{out:?}");
        assert!(out.conflicts.is_empty(), "{out:?}");
        let fields = crate::native::intentions::read(&after, week_subject());
        let legacy = legacy::find_legacy_fields(
            IntentionsKind::Period(PeriodId::Week {
                year: 2026,
                week: 47,
            }),
            &after,
        );
        assert!(legacy.slots.is_empty(), "{:?}\n{after}", legacy.slots);
        assert!(fields.intentions.value.is_none(), "{after}");
        let rerun = migrate_note_contents(&after, week_subject(), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    #[test]
    fn empty_year_and_quarter_slots_are_removed_but_archive_stays() {
        let year = "---\nparent: \"[[all_time]]\"\n---\n\n# 2026\n\n## Theme\n\n## Outcomes\n\n## Quarters\n\n```bob-period-parts\n## Outcomes\n```\n\n## Review\n\n## Reflection\n\n> [!agent]- Agent instructions · yearly v2\n> text\n\n## Archive\n\n## Theme\n\nOld zorg theme.\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(year, year_subject(), today);
        let after = out.after.clone().expect("empty year slots are removed");
        let (live, archive) = after.split_once("## Archive").expect("archive");
        assert!(!live.contains("## Theme"), "{after}");
        assert!(!live.contains("## Reflection"), "{after}");
        assert!(
            live.contains("```bob-period-parts\n## Outcomes\n```"),
            "fenced lines are not headings: {after}"
        );
        assert_eq!(live.matches("## Outcomes").count(), 1, "{after}");
        assert!(live.contains("## Quarters"), "{after}");
        assert!(live.contains("## Review"), "{after}");
        assert!(archive.contains("## Theme\n\nOld zorg theme."), "{after}");
        assert!(!after.contains("intentions:"), "{after}");
        let rerun = migrate_note_contents(&after, year_subject(), today);
        assert!(rerun.after.is_none(), "{rerun:?}");

        let quarter = "---\nparent: \"[[2026]]\"\n---\n\n# 2026-Q4\n\n## Outcomes\n\n## Weeks\n\n```bob-period-parts\n\n```\n\n## Review\n\n## Reflection\n";
        let out = migrate_note_contents(quarter, quarter_subject(), today);
        let after = out.after.expect("empty quarter slots are removed");
        assert!(!after.contains("## Outcomes"), "{after}");
        assert!(!after.contains("## Reflection"), "{after}");
        assert!(after.ends_with("## Review\n"), "{after:?}");
    }

    #[test]
    fn slots_with_text_keep_their_headings() {
        // A placeholder stays (reported) while the empty Reflection goes.
        let placeholder = "---\nid: 2026-W47\n---\n\n# 2026-W47\n\n## Intentions\n\n- NOT SET\n\n## Days\n\n## Review\n\n## Reflection\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(placeholder, week_subject(), today);
        let after = out.after.clone().expect("empty Reflection is removed");
        assert!(after.contains("## Intentions\n\n- NOT SET"), "{after}");
        assert!(!after.contains("## Reflection"), "{after}");
        assert!(!out.placeholders.is_empty(), "{out:?}");

        // A fence-only Reflection is legacy period text: migrate moves it.
        let fenced = week_note("## Reflection\n\n```text\nkeep me\n```\n");
        let out = migrate_note_contents(&fenced, week_subject(), today);
        let after = out.after.expect("fence-only Reflection migrates");
        assert!(!after.contains("## Reflection"), "{after}");
        let fields = crate::native::intentions::read(&after, week_subject());
        assert_eq!(
            fields.reflection.value.as_deref(),
            Some("```text\nkeep me\n```"),
            "{after}"
        );
        let rerun = migrate_note_contents(&after, week_subject(), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    #[test]
    fn reflection_keeps_a_fenced_code_block_verbatim() {
        let body = "Kept prose.\n\n```rust\nfn x() {}\n```";
        let before = week_note(&format!("## Reflection\n\n{body}\n"));
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        let after = out.after.expect("migrates");
        assert!(!after.contains("## Reflection"), "{after}");
        assert!(after.contains("## Days"), "{after}");
        let fields = crate::native::intentions::read(&after, week_subject());
        assert_eq!(fields.reflection.value.as_deref(), Some(body), "{after}");
        let rerun = migrate_note_contents(&after, week_subject(), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    #[test]
    fn review_opener_keeps_a_fenced_heading_once() {
        let before = "---\nparent: \"[[2026/2026-Q4]]\"\nid: 2026-W47\n---\n\n# 2026-W47\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\n**Headline:** A calm week\n\n```md\n## not a heading\nkeep me\n```\n\n## Extra\n\nAfter review.\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, week_subject(), today);
        let after = out.after.expect("migrates");
        assert!(after.contains("summary:"), "{after}");
        assert!(!after.contains("**Headline:**"), "{after}");
        assert_eq!(
            after
                .matches("```md\n## not a heading\nkeep me\n```")
                .count(),
            1,
            "{after}"
        );
        assert!(after.contains("## Extra\n\nAfter review."), "{after}");
        let fields = crate::native::intentions::read(&after, week_subject());
        assert_eq!(fields.summary.value.as_deref(), Some("A calm week"));
        assert!(
            fields
                .review_body
                .contains("```md\n## not a heading\nkeep me\n```"),
            "{after}"
        );
        let rerun = migrate_note_contents(&after, week_subject(), today);
        assert!(rerun.after.is_none(), "{rerun:?}");
    }

    #[test]
    fn intentions_with_a_fenced_block_are_left_in_place() {
        let before = week_note(
            "## Intentions\n\n- Ship it\n\n```text\nnot a bullet\n```\n",
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(&before, week_subject(), today);
        assert!(out.after.is_none(), "{out:?}");
        assert!(
            out.skipped.iter().any(|s| s.contains("fenced block")),
            "{out:?}"
        );
        assert!(before.contains("## Intentions"), "{before}");
    }

    #[test]
    fn year_theme_with_a_fenced_block_is_left_in_place() {
        let before = "---\nparent: \"[[all_time]]\"\n---\n\n# 2026\n\n## Theme\n\nMake room\n\n```text\nnot a theme\n```\n\n## Quarters\n\n```bob-period-parts\n\n```\n\n## Review\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, year_subject(), today);
        assert!(out.after.is_none(), "{out:?}");
        assert!(
            out.skipped
                .iter()
                .any(|s| s.contains("Theme") && s.contains("fenced block")),
            "{out:?}"
        );
        let values = legacy::intentions(
            IntentionsKind::Period(PeriodId::Year(2026)),
            before,
        );
        assert!(values.is_none(), "{values:?}");
    }

    #[test]
    fn fence_only_reflection_is_legacy_period_text() {
        let before = week_note("## Reflection\n\n```text\nkeep me\n```\n");
        let legacy = legacy::find_legacy_fields(
            IntentionsKind::Period(PeriodId::Week {
                year: 2026,
                week: 47,
            }),
            &before,
        );
        assert!(legacy.slots.contains(&"week ## Reflection"), "{legacy:?}");
        assert_eq!(
            legacy.reflection.as_deref(),
            Some("```text\nkeep me\n```"),
            "{legacy:?}"
        );
    }

    #[test]
    fn empty_headline_slot_is_removed() {
        let before = "---\nparent: \"[[2026/2026-W47]]\"\n---\n\n**Headline:**\n\n## Pomodoros\n";
        let today = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let out = migrate_note_contents(before, day("2026-11-19"), today);
        assert!(out.after.is_some(), "{out:?}");
        let after = out.after.unwrap();
        assert!(!after.contains("**Headline:**"), "{after}");
        assert!(
            !out.migrated,
            "empty slot removal is not an intentions migration"
        );
    }
}

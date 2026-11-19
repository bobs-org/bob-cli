//! `bob periods draft`: write a day's summary or split a period draft
//! into a frontmatter summary plus the `## Review` body.
//!
//! The contract lives in `docs/periods.md` §Drafting reviews. A day
//! draft is one paragraph of 1–3 sentences, at most 60 words and two
//! links, stored as the frontmatter `summary`. A period draft opens
//! with a `**Summary:**` line (the one-release `**Headline:**` alias
//! still reads) and the remaining text becomes only the `## Review`
//! body. Both values are stamped independently with the shared digest
//! recipe. The shared eligibility decision (`super::eligibility`) backs
//! both the brief's stop advice and these guards; `-F/--force`
//! overrides ownership and time guards only. Writes wait for the vault
//! lock, re-read before writing, verify the combined document, and
//! never commit. A period draft is never half applied: frontmatter and
//! body land in one verified pass.

use std::{ffi::OsString, fs, io, io::Read, path::Path, time::Duration};

use serde_json::{json, Value};

use super::{
    calendar::{PeriodId, PeriodState},
    cli, eligibility, note,
};
use crate::native::{
    env as bob_env,
    intentions::{
        self,
        write::{apply_with_review, OwnedPatch, Patch, ReviewPatch},
        NoteSubject,
    },
    ob, yaml_spans,
};

const COMMAND_NAME: &str = "bob periods draft";
const SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT: Duration = Duration::from_secs(3);
const SUMMARY_CAP_PERIOD: usize = 25;
const SUMMARY_CAP_DAY: usize = 60;
const MAX_LINKS_DAY: usize = 2;

#[derive(Debug, Clone)]
struct DraftError {
    code: String,
    message: String,
    hint: String,
    exit_code: i32,
    extra: Option<Value>,
}

impl DraftError {
    fn usage(code: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            message,
            hint: "See docs/periods.md for the draft rules and word caps."
                .to_string(),
            exit_code: 2,
            extra: None,
        }
    }

    fn refused(code: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            message,
            hint: "Pass -F/--force when Bryan said redo or asked for so far."
                .to_string(),
            exit_code: 3,
            extra: None,
        }
    }

    fn failed(code: &str, message: String, exit_code: i32) -> Self {
        Self {
            code: code.to_string(),
            message,
            hint: "Check the vault path and file permissions.".to_string(),
            exit_code,
            extra: None,
        }
    }

    /// Malformed or duplicate YAML fails closed, even with `-F/--force`.
    fn invalid_yaml(code: String, message: String) -> Self {
        Self {
            code,
            message,
            hint: "Fix the note's frontmatter YAML and retry.".to_string(),
            exit_code: 1,
            extra: None,
        }
    }

    fn json(&self) -> Value {
        let mut error = json!({
            "code": self.code,
            "message": self.message,
            "hint": self.hint,
        });
        if let Some(extra) = &self.extra
            && let (Some(object), Value::Object(fields)) =
                (error.as_object_mut(), extra)
        {
            for (key, value) in fields {
                object.insert(key.clone(), value.clone());
            }
        }
        json!({
            "ok": false,
            "schema_version": SCHEMA_VERSION,
            "error": error,
        })
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let command = cli::draft_command();
    let mut command = command;
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let human = matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human")
        != "json";
    match execute(&matches) {
        Ok(report) => {
            if human {
                if matches.get_flag("dry-run") {
                    println!("{}", report.planned);
                } else {
                    println!(
                        "Drafted {} {} · {} words · state {}",
                        report.id,
                        report.noun,
                        report.words,
                        report.review_state,
                    );
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&report.json()).unwrap_or_default()
                );
            }
            0
        }
        Err(error) => {
            if human {
                eprintln!("{COMMAND_NAME}: {}", error.message);
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&error.json()).unwrap_or_default()
                );
            }
            error.exit_code
        }
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

struct DraftReport {
    id: String,
    noun: &'static str,
    words: usize,
    planned: String,
    dry_run: bool,
    review_state: String,
    summary_state: String,
}

impl DraftReport {
    fn json(&self) -> Value {
        let mut report = json!({
            "ok": true,
            "schema_version": SCHEMA_VERSION,
            "id": self.id,
            "words": self.words,
            "review_state": self.review_state,
            "summary_state": self.summary_state,
        });
        // Dry runs make the proposed summary and (for periods) Review
        // reviewable without writes or commits.
        if self.dry_run
            && let Some(object) = report.as_object_mut()
        {
            object.insert(
                "planned".to_string(),
                Value::String(self.planned.clone()),
            );
        }
        report
    }
}

/// A parsed draft: the stored summary plus, for periods, the Review body.
/// `words` counts the submitted summary-plus-body with the existing
/// display-text convention (wiki-links reduced to their shown text).
struct ParsedDraft {
    summary: String,
    review: String,
    words: usize,
}

fn execute(matches: &clap::ArgMatches) -> Result<DraftReport, DraftError> {
    // One explicit BOB_NOW-aware clock per invocation.
    let today = bob_env::current_datetime().date();
    let raw_subject = matches
        .get_one::<String>("period")
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    let subject = if raw_subject.is_empty() {
        NoteSubject::Period(PeriodId::week_for_date(today))
    } else {
        NoteSubject::resolve(raw_subject, today)
            .map_err(|message| DraftError::usage("invalid_period", message))?
            .subject
    };
    if matches!(subject, NoteSubject::Period(PeriodId::AllTime)) {
        return Err(DraftError::usage(
            "no_review_section",
            "all_time has no review".to_string(),
        ));
    }
    let raw = read_input(matches)?;
    let draft = match subject {
        NoteSubject::Day(_) => parse_day(&raw)?,
        NoteSubject::Period(id) => parse_period(&raw, id)?,
    };
    let vault = bob_env::bob_dir();
    let (canonical_id, note_relative, noun, state_name) = match subject {
        NoteSubject::Day(date) => (
            date.format("%Y-%m-%d").to_string(),
            NoteSubject::day_path(date),
            "summary",
            if date < today {
                "closed"
            } else if date > today {
                "future"
            } else {
                "open"
            },
        ),
        NoteSubject::Period(id) => (
            id.to_string(),
            id.path(),
            "Review",
            match id.state(today) {
                PeriodState::Closed => "closed",
                PeriodState::Current => "open",
                PeriodState::Future => "future",
            },
        ),
    };
    // Never create notes or headings: a missing daily note names the
    // Obsidian recovery hint, and a missing period note or Review keeps
    // its existing error.
    let before = match subject {
        NoteSubject::Day(_) => {
            let path = vault.join(&note_relative);
            fs::read_to_string(&path).map_err(|_| {
                let mut error = DraftError::usage(
                    "missing_note",
                    format!(
                        "no daily note at {}: open the day in Obsidian so Templater creates it",
                        note_relative.display(),
                    ),
                );
                error.hint = "Open the day in Obsidian before drafting its summary; Templater creates daily notes.".to_string();
                error
            })?
        }
        NoteSubject::Period(_) => {
            let path = vault.join(&note_relative);
            fs::read_to_string(&path).map_err(|_| {
                DraftError::usage(
                    "no_review_section",
                    format!(
                        "no period note at {}: draft never creates or inserts headings",
                        note_relative.display(),
                    ),
                )
            })?
        }
    };
    if matches!(subject, NoteSubject::Period(_)) && !has_review_heading(&before)
    {
        return Err(DraftError::usage(
            "no_review_section",
            format!(
                "the note at {} has no ## Review: draft never inserts headings",
                note_relative.display(),
            ),
        ));
    }
    // The typed legacy-aware reader keeps old Headline Reviews protected.
    // Malformed or duplicate YAML fails closed, even with -F/--force.
    let fields = intentions::read(&before, subject);
    if matches!(
        fields.frontmatter_state,
        intentions::fields::FrontmatterState::Malformed
    ) {
        if let Err((code, message)) = frontmatter_mapping(&before) {
            return Err(DraftError::invalid_yaml(
                code,
                format!("the note at {}: {message}", note_relative.display()),
            ));
        }
        let detail = fields
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(DraftError::invalid_yaml(
            "frontmatter_invalid".to_string(),
            format!(
                "the note at {} has invalid frontmatter: {detail}",
                note_relative.display()
            ),
        ));
    }
    let has_review_body = match subject {
        NoteSubject::Day(_) => false,
        NoteSubject::Period(_) => !fields.review_body.trim().is_empty(),
    };
    let noun_kind = match subject {
        NoteSubject::Day(_) => "day",
        NoteSubject::Period(PeriodId::Week { .. }) => "week",
        NoteSubject::Period(PeriodId::Quarter { .. }) => "quarter",
        NoteSubject::Period(_) => "year",
    };
    // The shared eligibility decision; -F/--force overrides ownership and
    // time guards only, never invalid text, malformed YAML, missing
    // targets, locking, stale-note detection, or verification failures.
    if !matches.get_flag("force") {
        match eligibility::decide(
            &fields,
            has_review_body,
            state_name,
            noun_kind,
        ) {
            eligibility::Eligibility::Allow { .. } => {}
            eligibility::Eligibility::RefusedOpen { advice } => {
                return Err(DraftError::refused("refused_open", advice));
            }
            eligibility::Eligibility::RefusedState { advice } => {
                return Err(DraftError::refused("refused_state", advice));
            }
        }
    }
    // For periods, an edited or reflected existing Review stays protected
    // even when the new summary key is absent: the first-summary
    // exception must not overwrite Bryan's existing Review. A
    // summary-only draft never wipes a nonempty Review without -F.
    if matches!(subject, NoteSubject::Period(_))
        && draft.review.trim().is_empty()
        && has_review_body
        && !matches.get_flag("force")
    {
        return Err(DraftError::refused(
            "refused_state",
            format!(
                "the Review of {canonical_id} already has content: a summary-only draft needs an explicit redo, with -F"
            ),
        ));
    }
    let today_text = today.format("%Y-%m-%d").to_string();
    let summary_digest = intentions::digest(&draft.summary);
    let owned = OwnedPatch {
        summary: Patch::Set(draft.summary.clone()),
        summary_digest: Patch::Set(summary_digest),
        summary_drafted: Patch::Set(today_text.clone()),
        ..OwnedPatch::default()
    };
    let mut review_patch = ReviewPatch::default();
    if matches!(subject, NoteSubject::Period(_)) {
        if draft.review.trim().is_empty() {
            review_patch.review_digest = Patch::Unset;
            review_patch.review_drafted = Patch::Unset;
        } else {
            review_patch.review_digest =
                Patch::Set(intentions::digest(&draft.review));
            review_patch.review_drafted = Patch::Set(today_text.clone());
        }
    }
    let planned = match subject {
        NoteSubject::Day(_) => draft.summary.clone(),
        NoteSubject::Period(_) => {
            let mut planned = format!("**Summary:** {}", draft.summary);
            planned.push_str("\n\n## Review\n");
            if !draft.review.trim().is_empty() {
                planned.push('\n');
                planned.push_str(&draft.review);
            }
            planned.push('\n');
            planned
        }
    };
    if matches.get_flag("dry-run") {
        let (review_state, summary_state) =
            planned_states(&fields, &draft, subject, has_review_body);
        return Ok(DraftReport {
            id: canonical_id,
            noun,
            words: draft.words,
            planned,
            dry_run: true,
            review_state,
            summary_state,
        });
    }
    let _lock = ob::acquire_lock_waiting(LOCK_WAIT, || {}).map_err(
        |error| match error {
            ob::LockWaitError::Timeout { path } => DraftError::failed(
                "lock_busy",
                format!(
                    "another Bob vault maintenance run is already active (waited on {})",
                    path.display()
                ),
                1,
            ),
            ob::LockWaitError::Open { path, error } => {
                DraftError::failed(
                    "io_error",
                    format!(
                        "could not open lock file {}: {error}",
                        path.display()
                    ),
                    1,
                )
            }
            ob::LockWaitError::Acquire { path, error } => {
                DraftError::failed(
                    "io_error",
                    format!("could not lock {}: {error}", path.display()),
                    1,
                )
            }
        },
    )?;
    // Re-read under the lock; refuse when the note changed.
    let path = vault.join(&note_relative);
    let current = fs::read_to_string(&path).map_err(|error| {
        DraftError::failed("io_error", error.to_string(), 1)
    })?;
    if current != before {
        return Err(DraftError::failed(
            "stale_note",
            format!(
                "the note at {} changed while waiting for the lock",
                note_relative.display()
            ),
            1,
        ));
    }
    // Never apply half a period draft: body and stamps land in one
    // verified pass over the locked snapshot.
    let base_current: String;
    let base: &str = match subject {
        NoteSubject::Day(_) => &current,
        NoteSubject::Period(_) => {
            base_current = replace_review_body(&current, &draft.review)
                .ok_or_else(|| {
                    DraftError::failed(
                        "io_error",
                        "could not plan the draft write".to_string(),
                        1,
                    )
                })?;
            &base_current
        }
    };
    let after =
        apply_with_review(base, &owned, &review_patch).map_err(|error| {
            match error.code {
                "duplicate_key" => DraftError::invalid_yaml(
                    error.code.to_string(),
                    error.message,
                ),
                "frontmatter_invalid" | "write_verify_failed" => {
                    DraftError::invalid_yaml(
                        error.code.to_string(),
                        error.message,
                    )
                }
                _ => DraftError::failed("io_error", error.message, 1),
            }
        })?;
    verify_final(&before, &after, subject, &draft)?;
    atomic_write(&path, &after).map_err(|error| {
        DraftError::failed("io_error", error.to_string(), 1)
    })?;
    let staged = intentions::read(&after, subject);
    let review_state = intentions::evaluate(
        &staged,
        matches!(subject, NoteSubject::Period(_))
            && !draft.review.trim().is_empty(),
    )
    .as_str()
    .to_string();
    let summary_state = intentions::summary_state(&staged).as_str().to_string();
    Ok(DraftReport {
        id: canonical_id,
        noun,
        words: draft.words,
        planned,
        dry_run: false,
        review_state,
        summary_state,
    })
}

/// Predicted overall and summary states for a dry run: a successful
/// first draft after reflection remains `reflected`, while its
/// `summary_state` is `drafted`.
fn planned_states(
    fields: &intentions::fields::Fields,
    draft: &ParsedDraft,
    subject: NoteSubject,
    has_review_body: bool,
) -> (String, String) {
    let reflected = fields
        .reflection
        .value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let summary_state = "drafted".to_string();
    let review_state = if reflected {
        "reflected".to_string()
    } else if matches!(subject, NoteSubject::Day(_)) {
        "drafted".to_string()
    } else if draft.review.trim().is_empty() && !has_review_body {
        "drafted".to_string()
    } else if draft.review.trim().is_empty() {
        intentions::evaluate(fields, has_review_body)
            .as_str()
            .to_string()
    } else {
        "drafted".to_string()
    };
    let _ = fields;
    (review_state, summary_state)
}

fn read_input(matches: &clap::ArgMatches) -> Result<String, DraftError> {
    let Some(input) = matches.get_one::<OsString>("input") else {
        return Err(DraftError::usage(
            "invalid_draft",
            "-i/--input is required: a draft file, or - for stdin".to_string(),
        ));
    };
    if input == "-" {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text).map_err(|error| {
            DraftError::failed("io_error", error.to_string(), 1)
        })?;
        return Ok(text);
    }
    let path = Path::new(input);
    fs::read_to_string(path).map_err(|error| {
        DraftError::usage(
            "invalid_draft",
            format!("could not read {}: {error}", path.display()),
        )
    })
}

fn strip_blank_edges(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .collect();
    kept.join("\n").trim_end().to_string()
}

/// Parse a day draft: one paragraph, optional `**Summary:**` label,
/// soft wraps joined into one stored line.
fn parse_day(raw: &str) -> Result<ParsedDraft, DraftError> {
    let text = strip_blank_edges(raw);
    if text.is_empty() {
        return Err(DraftError::usage(
            "invalid_draft",
            "the summary is empty: write 1–3 sentences in one paragraph"
                .to_string(),
        ));
    }
    let text = strip_summary_label(&text);
    if text.lines().any(|line| line.trim().is_empty()) {
        return Err(DraftError::usage(
            "invalid_draft",
            "the summary must be one paragraph: join soft wraps into one stored line"
                .to_string(),
        ));
    }
    check_day_lines(&text)?;
    let links = count_wikilinks(&text) + count_markdown_links(&text);
    if links > MAX_LINKS_DAY {
        return Err(DraftError::usage(
            "invalid_draft",
            format!("the summary has {links} links; at most {MAX_LINKS_DAY}"),
        ));
    }
    // Join soft wraps of the one paragraph into one stored line. The
    // instruction asset specifies 1–3 sentences; no brittle
    // punctuation-based validation treats abbreviations or link targets
    // as sentences.
    let summary = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return Err(DraftError::usage(
            "invalid_draft",
            "the summary is empty: write 1–3 sentences in one paragraph"
                .to_string(),
        ));
    }
    let words = count_words(&summary);
    if words > SUMMARY_CAP_DAY {
        let mut error = DraftError::usage(
            "over_cap",
            format!("the summary is {words} words; at most {SUMMARY_CAP_DAY}"),
        );
        error.extra = Some(json!({ "count": words, "cap": SUMMARY_CAP_DAY }));
        return Err(error);
    }
    Ok(ParsedDraft {
        summary,
        review: String::new(),
        words,
    })
}

/// Parse a period draft: a `**Summary:**` opener (the one-release
/// `**Headline:**` alias still reads) joined onto one line, plus the
/// remaining text as only the `## Review` body.
fn parse_period(raw: &str, id: PeriodId) -> Result<ParsedDraft, DraftError> {
    let stripped = strip_blank_edges(raw);
    // The one-release alias keeps the established headline joining
    // semantics before the split.
    let text = if stripped.trim_start().starts_with("**Headline:**") {
        note::unwrap_headline(&stripped).unwrap_or(stripped)
    } else {
        stripped
    };
    if text.is_empty() {
        return Err(DraftError::usage(
            "missing_summary",
            "the draft must open with a **Summary:** line".to_string(),
        ));
    }
    // The opening paragraph (soft-wrapped lines joined) carries the
    // summary; the rest becomes the Review body.
    let mut lines = text.lines();
    let mut opener: Vec<&str> = Vec::new();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        opener.push(line);
    }
    let rest: Vec<&str> = lines.collect();
    let opener_joined = opener.join(" ");
    let summary = opener_joined
        .trim_start()
        .strip_prefix("**Summary:**")
        .or_else(|| opener_joined.trim_start().strip_prefix("**Headline:**"))
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .ok_or_else(|| {
            DraftError::usage(
                "missing_summary",
                "the first line must be **Summary:** <text>".to_string(),
            )
        })?
        .to_string();
    let summary_words = count_words(&summary);
    if summary_words > SUMMARY_CAP_PERIOD {
        let mut error = DraftError::usage(
            "over_cap",
            format!(
                "the summary is {summary_words} words; at most {SUMMARY_CAP_PERIOD}"
            ),
        );
        error.extra =
            Some(json!({ "count": summary_words, "cap": SUMMARY_CAP_PERIOD }));
        return Err(error);
    }
    let review = strip_blank_edges(&rest.join("\n"));
    if let Some(first) = review.lines().next() {
        let trimmed = first.trim_start();
        if trimmed.starts_with("**Summary:**")
            || trimmed.starts_with("**Headline:**")
        {
            return Err(DraftError::usage(
                "invalid_draft",
                "the Review body must not repeat the **Summary:** opener"
                    .to_string(),
            ));
        }
    }
    check_lines(&review)?;
    let cap = match id {
        PeriodId::Week { .. } => 250,
        PeriodId::Quarter { .. } => 400,
        PeriodId::Year(_) => 600,
        PeriodId::AllTime => unreachable!("all_time refused above"),
    };
    // Total cap over the submitted summary-plus-body with the existing
    // counting convention.
    let words = count_words(&text);
    if words > cap {
        let mut error = DraftError::usage(
            "over_cap",
            format!("the draft is {words} words; at most {cap}"),
        );
        error.extra = Some(json!({ "count": words, "cap": cap }));
        return Err(error);
    }
    Ok(ParsedDraft {
        summary,
        review,
        words,
    })
}

/// Strip one optional leading `**Summary:**` label line.
fn strip_summary_label(text: &str) -> String {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    if let Some(rest) = first.trim_start().strip_prefix("**Summary:**") {
        let rest = rest.trim();
        if rest.is_empty() {
            return lines.collect::<Vec<_>>().join("\n");
        }
        let mut out = String::from(rest);
        for line in lines {
            out.push('\n');
            out.push_str(line);
        }
        return out;
    }
    text.to_string()
}

/// Reject day-draft structure: headings, lists, fences, and callouts.
fn check_day_lines(text: &str) -> Result<(), DraftError> {
    check_lines(text)?;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || is_ordered_list_item(trimmed)
        {
            return Err(DraftError::usage(
                "invalid_draft",
                "the summary must not contain a list: write one paragraph"
                    .to_string(),
            ));
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            return Err(DraftError::usage(
                "invalid_draft",
                "the summary must not contain a fenced code block".to_string(),
            ));
        }
        if trimmed.starts_with('>') {
            return Err(DraftError::usage(
                "invalid_draft",
                "the summary must not contain a callout".to_string(),
            ));
        }
    }
    Ok(())
}

fn is_ordered_list_item(trimmed: &str) -> bool {
    let mut digits = 0;
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
        } else {
            break;
        }
    }
    digits > 0
        && trimmed[digits..].starts_with(['.', ')'])
        && trimmed[digits + 1..].starts_with(' ')
}

/// Count `[[wiki]]` links.
fn count_wikilinks(text: &str) -> usize {
    let mut count = 0;
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        match after.find("]]") {
            Some(end) => {
                count += 1;
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    count
}

/// Count Markdown `[label](target)` links.
fn count_markdown_links(text: &str) -> usize {
    let mut count = 0;
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(close) = after.find(']') else {
            break;
        };
        if after[close + 1..].starts_with('(')
            && let Some(paren) = after[close + 1..].find(')')
        {
            count += 1;
            rest = &after[close + 1 + paren + 1..];
        } else {
            rest = &after[close + 1..];
        }
    }
    count
}

/// Reject headings, agent callouts, and frontmatter fences.
fn check_lines(text: &str) -> Result<(), DraftError> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            return Err(DraftError::usage(
                "invalid_draft",
                "the draft must not contain a Markdown heading line"
                    .to_string(),
            ));
        }
        if trimmed.starts_with("---") {
            return Err(DraftError::usage(
                "invalid_draft",
                "the draft must not contain a --- frontmatter fence"
                    .to_string(),
            ));
        }
        if line.contains("[!agent]") {
            return Err(DraftError::usage(
                "invalid_draft",
                "the draft must not contain an agent callout".to_string(),
            ));
        }
    }
    Ok(())
}

/// Reduce `[[target|alias]]` to its alias and `[[target]]` to its last
/// path segment, then count whitespace-separated words.
pub(crate) fn count_words(text: &str) -> usize {
    reduce_links(text).split_whitespace().count()
}

fn reduce_links(text: &str) -> String {
    let mut reduced = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        reduced.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("]]") {
            Some(end) => {
                let inner = &after[..end];
                let shown = match inner.find('|') {
                    Some(pipe) => &inner[pipe + 1..],
                    None => inner.rsplit('/').next().unwrap_or(inner),
                };
                reduced.push_str(shown);
                rest = &after[end + 2..];
            }
            None => {
                reduced.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    reduced.push_str(rest);
    reduced
}

/// The first `cap` words of `text`, with `…` when longer.
pub(crate) fn truncate_words(text: &str, cap: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= cap {
        return text.trim().to_string();
    }
    format!("{}…", words[..cap].join(" "))
}

fn has_review_heading(contents: &str) -> bool {
    contents.lines().any(|line| line.trim_end() == "## Review")
}

/// Replace the `## Review` body region, preserving every byte outside
/// the owned Review span: BOM, line endings, comments, unrelated
/// frontmatter, reflection/intentions, other body sections, and callouts
/// stay byte-for-byte. The newline style follows the file.
fn replace_review_body(contents: &str, body: &str) -> Option<String> {
    let lines = yaml_spans::split_lines(contents);
    let heading = lines
        .iter()
        .position(|line| line.text.trim_end() == "## Review")?;
    let newline = lines
        .iter()
        .find(|line| !line.ending.is_empty())
        .map(|line| line.ending.clone())
        .unwrap_or_else(|| "\n".to_owned());
    // Next block boundary: a `## ` heading or an agent callout.
    let mut limit = lines.len();
    for (index, line) in lines.iter().enumerate().skip(heading + 1) {
        if line.text.starts_with("## ") || line.text.starts_with("> [!agent]") {
            limit = index;
            break;
        }
    }
    // Owned span: first non-blank line after the heading through the
    // last non-blank line before the next block.
    let mut start = limit;
    for index in heading + 1..limit {
        if !lines[index].text.trim().is_empty() {
            start = index;
            break;
        }
    }
    let mut end = start;
    for index in (heading + 1..limit).rev() {
        if !lines[index].text.trim().is_empty() {
            end = index + 1;
            break;
        }
    }
    let prefix = yaml_spans::join_lines(&lines[..start]);
    let suffix = yaml_spans::join_lines(&lines[end..]);
    let prefix = prefix.trim_end_matches(['\n', '\r']).to_string();
    let suffix = suffix.trim_start_matches(['\n', '\r']).to_string();
    let mut out = prefix;
    out.push_str(&newline);
    out.push_str(&newline);
    if !body.trim().is_empty() {
        for line in body.split('\n') {
            out.push_str(line.trim_end());
            out.push_str(&newline);
        }
    }
    if !suffix.is_empty() {
        out.push_str(&newline);
        out.push_str(&suffix);
    }
    Some(out)
}

/// Verify the final combined document: owned values equal the intended
/// values and all other YAML keys retain their values. Malformed or
/// duplicate YAML fails closed. BOM is preserved.
fn verify_final(
    original: &str,
    final_doc: &str,
    subject: NoteSubject,
    draft: &ParsedDraft,
) -> Result<(), DraftError> {
    let invalid = |message: String| {
        DraftError::invalid_yaml("write_verify_failed".to_string(), message)
    };
    if original.starts_with('\u{feff}') && !final_doc.starts_with('\u{feff}') {
        return Err(invalid("the write dropped the BOM".to_string()));
    }
    let before_map = frontmatter_mapping(original)
        .map_err(|message| DraftError::invalid_yaml(message.0, message.1))?;
    let after_map = frontmatter_mapping(final_doc)
        .map_err(|message| DraftError::invalid_yaml(message.0, message.1))?;
    let mut expected_before = before_map.clone();
    let mut expected_after = after_map.clone();
    for key in [
        "summary",
        "summary_digest",
        "summary_drafted",
        "review_digest",
        "review_drafted",
    ] {
        expected_before.remove(serde_yaml::Value::String(key.to_string()));
        expected_after.remove(serde_yaml::Value::String(key.to_string()));
    }
    if expected_before != expected_after {
        return Err(invalid("unrelated frontmatter keys changed".to_string()));
    }
    let staged = intentions::read(final_doc, subject);
    if staged.summary.value.as_deref() != Some(draft.summary.as_str()) {
        return Err(invalid("the summary did not round-trip".to_string()));
    }
    if staged.summary_digest.value.as_deref()
        != Some(&intentions::digest(&draft.summary))
    {
        return Err(invalid(
            "the summary digest did not round-trip".to_string(),
        ));
    }
    match subject {
        NoteSubject::Day(_) => {}
        NoteSubject::Period(_) => {
            if draft.review.trim().is_empty() {
                if staged.review_digest.value.is_some() {
                    return Err(invalid(
                        "a cleared Review must drop its stamps".to_string(),
                    ));
                }
            } else if staged.review_digest.value.as_deref()
                != Some(&intentions::digest(&draft.review))
            {
                return Err(invalid(
                    "the Review digest did not round-trip".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// The frontmatter mapping, or an empty map when the note has none.
/// `(code, message)` on malformed or duplicate YAML.
fn frontmatter_mapping(
    contents: &str,
) -> Result<serde_yaml::Mapping, (String, String)> {
    let fail = |message: String| (String::from("frontmatter_invalid"), message);
    let lines = yaml_spans::split_lines(contents);
    let Some(first) = lines.first() else {
        return Ok(serde_yaml::Mapping::new());
    };
    let opener = first.text.strip_prefix('\u{feff}').unwrap_or(&first.text);
    if opener.trim() != "---" {
        return Ok(serde_yaml::Mapping::new());
    }
    let Some(close) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            (line.text.trim() == "---").then_some(index)
        })
    else {
        return Err(fail("frontmatter block is not closed".to_string()));
    };
    yaml_spans::collect_key_spans(&lines[1..close], 1)
        .map_err(|message| (String::from("duplicate_key"), message))?;
    let yaml = yaml_spans::yaml_text(&lines[1..close]);
    // Mirror the field writer: only a duplicate flow key fails here;
    // comments ride through to the serde parse below.
    if yaml_spans::validate_flow_map_keys(&yaml)
        == Err(yaml_spans::FlowMapError::DuplicateKey)
    {
        return Err((
            String::from("duplicate_key"),
            String::from("duplicate key in frontmatter flow mapping"),
        ));
    }
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml)
        .map_err(|error| fail(format!("invalid frontmatter YAML: {error}")))?;
    match value {
        serde_yaml::Value::Mapping(mapping) => Ok(mapping),
        _ => Err(fail("frontmatter YAML must be a mapping".to_string())),
    }
}

fn atomic_write(target: &Path, bytes: &str) -> io::Result<()> {
    let file_name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| OsString::from("period.md"));
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = target.with_file_name(temp_name);
    let _ = fs::remove_file(&temp);
    let mut file = fs::File::create(&temp)?;
    let write_result = io::Write::write_all(&mut file, bytes.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = write_result {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);
    match fs::rename(&temp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod atomic_write_tests {
    use super::atomic_write;

    #[test]
    fn atomic_write_installs_the_synced_temp_file() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("period.md");
        atomic_write(&target, "updated period\n").expect("write period");
        assert_eq!(
            std::fs::read_to_string(&target).expect("period note"),
            "updated period\n"
        );
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("directory")
                .count(),
            1
        );
    }
}

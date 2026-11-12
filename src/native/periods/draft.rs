//! `bob periods draft`: replace a period's `## Review` from a file.
//!
//! The contract lives in `docs/periods.md` §10. The draft text must
//! open with a `**Headline:**` line and stay within the kind's word
//! cap. An open or future period, or a Review Bryan already edited or
//! reflected, is refused unless `--force` is passed. `all_time` has
//! no Review. Writes wait for the vault lock, re-read before
//! writing, and never commit.

use std::{ffi::OsString, fs, io, io::Read, path::Path, time::Duration};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    calendar::{PeriodId, PeriodState},
    cli, note,
};
use crate::native::{env as bob_env, ob};

const COMMAND_NAME: &str = "bob periods draft";
const SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT: Duration = Duration::from_secs(3);
const HEADLINE_CAP: usize = 25;

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
                        "Drafted {} Review · {} words · state drafted",
                        report.id, report.words,
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
    id: PeriodId,
    words: usize,
    planned: String,
}

impl DraftReport {
    fn json(&self) -> Value {
        json!({
            "ok": true,
            "schema_version": SCHEMA_VERSION,
            "id": self.id.to_string(),
            "words": self.words,
            "review_state": "drafted",
        })
    }
}

fn execute(matches: &clap::ArgMatches) -> Result<DraftReport, DraftError> {
    let today = bob_env::current_datetime().date();
    let id = matches
        .get_one::<String>("period")
        .map(|text| {
            PeriodId::resolve(text, today)
                .map_err(|message| DraftError::usage("invalid_period", message))
        })
        .transpose()?
        .unwrap_or_else(|| PeriodId::week_for_date(today));
    if matches!(id, PeriodId::AllTime) {
        return Err(DraftError::usage(
            "no_review_section",
            "all_time has no review".to_string(),
        ));
    }
    let raw = read_input(matches)?;
    let text = strip_blank_edges(&raw);
    // A soft-wrapped Headline paragraph is joined onto one physical line
    // before the checks, so the cap counts the whole sentence and parents
    // never show half of it.
    let text = note::unwrap_headline(&text).unwrap_or(text);
    if text.is_empty() {
        return Err(DraftError::usage(
            "missing_headline",
            "the draft must open with a **Headline:** line".to_string(),
        ));
    }
    let cap = match id {
        PeriodId::Week { .. } => 250,
        PeriodId::Quarter { .. } => 400,
        PeriodId::Year(_) => 600,
        PeriodId::AllTime => unreachable!("all_time refused above"),
    };
    let headline = headline_of(&text).ok_or_else(|| {
        DraftError::usage(
            "missing_headline",
            "the first line must be **Headline:** <text>".to_string(),
        )
    })?;
    if count_words(&headline) > HEADLINE_CAP {
        let count = count_words(&headline);
        let mut error = DraftError::usage(
            "over_cap",
            format!("the Headline is {count} words; at most {HEADLINE_CAP}"),
        );
        error.extra = Some(json!({ "count": count, "cap": HEADLINE_CAP }));
        return Err(error);
    }
    check_lines(&text)?;
    let words = count_words(&text);
    if words > cap {
        let mut error = DraftError::usage(
            "over_cap",
            format!("the draft is {words} words; at most {cap}"),
        );
        error.extra = Some(json!({ "count": words, "cap": cap }));
        return Err(error);
    }
    let vault = bob_env::bob_dir();
    let path = vault.join(id.path());
    let before = fs::read_to_string(&path).map_err(|_| {
        DraftError::usage(
            "no_review_section",
            format!(
                "no period note at {}: draft never creates or inserts headings",
                id.path().display(),
            ),
        )
    })?;
    if !has_review_heading(&before) {
        return Err(DraftError::usage(
            "no_review_section",
            format!(
                "the note at {} has no ## Review: draft never inserts headings",
                id.path().display(),
            ),
        ));
    }
    // Guards, overridden only by -F/--force.
    if !matches.get_flag("force") {
        if !matches!(id.state(today), PeriodState::Closed) {
            return Err(DraftError::refused(
                "refused_open",
                format!(
                    "{id} is not closed: draft only when Bryan asked for so far, with -F"
                ),
            ));
        }
        match note::parse(&before).review_state {
            note::ReviewState::Edited | note::ReviewState::Reflected => {
                return Err(DraftError::refused(
                    "refused_state",
                    format!(
                        "the Review of {id} is already Bryan's: draft only when he said redo, with -F"
                    ),
                ));
            }
            note::ReviewState::Drafted | note::ReviewState::Empty => {}
        }
    }
    let planned = planned_section(&before, &text).ok_or_else(|| {
        DraftError::usage(
            "no_review_section",
            format!(
                "the note at {} has no ## Review: draft never inserts headings",
                id.path().display(),
            ),
        )
    })?;
    if matches.get_flag("dry-run") {
        return Ok(DraftReport { id, words, planned });
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
    // Re-read and compare before writing.
    let current = fs::read_to_string(&path).map_err(|error| {
        DraftError::failed("io_error", error.to_string(), 1)
    })?;
    if current != before {
        return Err(DraftError::failed(
            "stale_note",
            format!(
                "the note at {} changed while waiting for the lock",
                id.path().display()
            ),
            1,
        ));
    }
    let after = apply_draft(&current, &text, today).ok_or_else(|| {
        DraftError::failed(
            "io_error",
            "could not plan the draft write".to_string(),
            1,
        )
    })?;
    atomic_write(&path, &after).map_err(|error| {
        DraftError::failed("io_error", error.to_string(), 1)
    })?;
    Ok(DraftReport { id, words, planned })
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

/// The `**Headline:** <text>` opener, else none. `execute` has already
/// joined a soft-wrapped Headline paragraph onto this first line.
fn headline_of(text: &str) -> Option<String> {
    let first = text.lines().next().unwrap_or("").trim();
    first
        .strip_prefix("**Headline:**")
        .map(|headline| headline.trim().to_string())
        .filter(|headline| !headline.is_empty())
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

/// The planned `## Review` section: heading plus the draft body.
fn planned_section(contents: &str, text: &str) -> Option<String> {
    if !has_review_heading(contents) {
        return None;
    }
    let mut planned = String::from("## Review\n\n");
    planned.push_str(text);
    planned.push('\n');
    Some(planned)
}

/// Byte offsets of the Review body: first non-blank line after the
/// `## Review` heading through the last non-blank line before the
/// next block (`## ` heading, agent callout, or EOF).
fn review_span(contents: &str) -> Option<(usize, usize)> {
    let heading = contents
        .lines()
        .position(|line| line.trim_end() == "## Review")?;
    let mut body_start = None;
    let mut body_end = None;
    let mut offset = 0;
    for (index, line) in contents.lines().enumerate() {
        let length = line.len() + 1;
        if index <= heading {
            offset += length;
            continue;
        }
        if line.starts_with("## ") || line.starts_with("> [!agent]") {
            break;
        }
        if !line.trim().is_empty() {
            if body_start.is_none() {
                body_start = Some(offset);
            }
            body_end = Some(offset + line.trim_end().len());
        }
        offset += length;
    }
    match (body_start, body_end) {
        (Some(start), Some(end)) => Some((start, end)),
        _ => {
            // An empty Review: the body starts where the next block
            // would, right after the heading and its blank line.
            let mut offset = 0;
            for (index, line) in contents.lines().enumerate() {
                if index <= heading {
                    offset += line.len() + 1;
                    continue;
                }
                if line.trim().is_empty() {
                    offset += line.len() + 1;
                    continue;
                }
                break;
            }
            Some((offset, offset))
        }
    }
}

/// Replace the Review body and stamp the frontmatter digest keys.
fn apply_draft(contents: &str, text: &str, today: NaiveDate) -> Option<String> {
    let (start, end) = review_span(contents)?;
    let mut out = String::with_capacity(contents.len() + text.len());
    out.push_str(contents[..start].trim_end_matches(['\n', '\r']));
    out.push_str("\n\n");
    out.push_str(text);
    out.push('\n');
    let tail = contents[end..].trim_start_matches(['\n', '\r']);
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    Some(stamp_frontmatter(&out, text, today))
}

fn stamp_frontmatter(contents: &str, text: &str, today: NaiveDate) -> String {
    let digest = note::review_digest(text);
    let drafted = today.format("%Y-%m-%d").to_string();
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---" || line.trim() == "...")
        .map(|(index, _)| index);
    let Some(close) = close
        .filter(|_| lines.first().is_some_and(|first| first.trim() == "---"))
    else {
        // No frontmatter block: prepend one so the digest persists.
        let mut with_frontmatter = format!(
            "---\nreview_digest: {digest}\nreview_drafted: {drafted}\n---\n"
        );
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    };
    let mut has_digest = false;
    let mut has_drafted = false;
    for line in lines.iter_mut().take(close) {
        let key = line.split(':').next().unwrap_or("").trim();
        if key == "review_digest" {
            *line = format!("review_digest: {digest}");
            has_digest = true;
        } else if key == "review_drafted" {
            *line = format!("review_drafted: {drafted}");
            has_drafted = true;
        }
    }
    if !has_digest {
        lines.insert(close, format!("review_digest: {digest}"));
        let close = close + 1;
        if !has_drafted {
            lines.insert(close, format!("review_drafted: {drafted}"));
        }
    } else if !has_drafted {
        lines.insert(close, format!("review_drafted: {drafted}"));
    }
    let mut stamped = lines.join("\n");
    stamped.push('\n');
    stamped
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

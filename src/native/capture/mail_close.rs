//! Last-gate Gmail Task close confirmation for `bob capture`.
//!
//! After every item and post-pass, capture diffs each staged file's
//! original against its current with [`pair_task_lines`] and collects
//! open→closed transitions whose original block is a Gmail Task. Merge
//! retirement tombstones and tasks this capture created are exempt. A
//! real run with guarded rows refuses unless `--mail-close DIGEST`
//! matches; dry-run reports the same `mail_close` payload without
//! writing.

use super::*;
use crate::native::gmail::close_guard::{
    copy::{
        build_copy, close_verb, row_label, task_text, turn_line, CloseTarget,
        CloseVerb,
    },
    digest::{close_digest, DigestRow},
    predicate::{linked_mail_notes, other_open_linker_count, MailNoteFacts},
};
use crate::native::gmail::config::GmailConfig;
use crate::native::gmail::note::index as mail_index;
use crate::native::gmail::sync::scan::{
    scan_watching_tasks, WatchScan, OPEN_STATUSES,
};
use crate::native::vault_links::VaultLinkResolver;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

/// Top-level `mail_close` object on dry-run JSON and confirmation
/// refusals. Absent when nothing is guarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MailCloseReport {
    pub(super) digest: String,
    pub(super) verb: String,
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) tone: String,
    pub(super) consequence: String,
    pub(super) confirm: String,
    pub(super) tasks: Vec<MailCloseTask>,
}

/// One guarded close, uncapped in JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MailCloseTask {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: Option<String>,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) verb: String,
    pub(super) label: String,
    pub(super) turn_line: Option<String>,
    pub(super) mails: Vec<MailCloseMail>,
}

/// Mail-note facts for one guarded task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MailCloseMail {
    pub(super) note: String,
    pub(super) title: String,
    pub(super) who: String,
    pub(super) turn: Option<String>,
    pub(super) turn_since: Option<String>,
    pub(super) gmail_url: String,
}

impl From<&MailNoteFacts> for MailCloseMail {
    fn from(facts: &MailNoteFacts) -> Self {
        Self {
            note: facts.note.clone(),
            title: facts.title.clone(),
            who: facts.who.clone(),
            turn: facts.turn.clone(),
            turn_since: facts.turn_since.clone(),
            gmail_url: facts.gmail_url.clone(),
        }
    }
}

fn is_open(symbol: char) -> bool {
    OPEN_STATUSES.contains(&symbol)
}

fn is_closed(symbol: char) -> bool {
    matches!(symbol, 'x' | 'X' | '-')
}

fn block_has_wikilink(lines: &[&str], task: &note_tasks::NoteTask) -> bool {
    let end = task.block_end.min(lines.len().saturating_sub(1));
    lines
        .get(task.line_index..=end)
        .is_some_and(|block| block.iter().any(|line| line.contains("[[")))
}

/// Merge-retirement tombstones keep a `merged into` LINKS child or a
/// Cancel Log `📥 merged into` entry. Those closes are not a loss: the
/// watch follows the merge target.
fn is_merge_retirement(lines: &[&str], task: &note_tasks::NoteTask) -> bool {
    let end = task.block_end.min(lines.len().saturating_sub(1));
    let Some(block) = lines.get(task.line_index + 1..=end) else {
        return false;
    };
    block.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        (lower.contains("**links:**") && lower.contains("merged into"))
            || line.contains("📥 merged into")
    })
}

fn verb_name(after: char) -> &'static str {
    match close_verb(after) {
        Some(CloseVerb::Cancel) => "cancel",
        Some(CloseVerb::Complete) => "complete",
        None => "close",
    }
}

fn slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn now_stamp() -> String {
    bob_env::current_datetime()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

fn archive_on() -> bool {
    GmailConfig::resolve()
        .map(|config| config.archive_enabled())
        .unwrap_or(false)
}

fn watch_scan(vault: &Path) -> WatchScan {
    let (folder, link_label) = match GmailConfig::resolve() {
        Ok(config) => {
            (config.folder().to_string(), config.link_label().to_string())
        }
        Err(_) => (
            crate::native::gmail::config::DEFAULT_FOLDER.to_string(),
            crate::native::gmail::config::DEFAULT_LINK_LABEL.to_string(),
        ),
    };
    let folder_path = Path::new(&folder);
    let index =
        mail_index::scan(vault, folder_path, &link_label).unwrap_or_default();
    scan_watching_tasks(vault, folder_path, &index).unwrap_or_default()
}

struct Candidate {
    path: String,
    absolute: PathBuf,
    original_index: usize,
    before: char,
    after: char,
    original_line: String,
    block_id: Option<String>,
}

/// Diff staged files for guarded Gmail Task closes and build the
/// `mail_close` payload. Returns `None` when nothing is guarded.
pub(super) fn detect_mail_close(
    planner: &CaptureBatchPlanner,
    request: &CaptureRequest,
    task_settings: &note_tasks::NoteTaskSettings,
) -> Option<MailCloseReport> {
    let mut candidates = Vec::new();
    for file in &planner.files {
        if !file.existed || !file.present || file.original == file.current {
            continue;
        }
        let Ok(relative) = file.path.strip_prefix(&request.bob_dir) else {
            continue;
        };
        let original_scan = note_tasks::scan(&file.original, task_settings);
        let current_scan = note_tasks::scan(&file.current, task_settings);
        if original_scan.tasks().is_empty() {
            continue;
        }
        let pairs = pair_task_lines(
            &file.original,
            &file.current,
            &original_scan,
            &current_scan,
        );
        let original_lines: Vec<&str> = file.original.lines().collect();
        let current_lines: Vec<&str> = file.current.lines().collect();
        let relative_path = slashes(relative);

        for (&old_line, &new_line) in &pairs {
            let Some(old_task) = original_scan.task_at(old_line) else {
                continue;
            };
            let Some(new_task) = current_scan.task_at(new_line) else {
                continue;
            };
            if !is_open(old_task.status_symbol)
                || !is_closed(new_task.status_symbol)
            {
                continue;
            }
            if let Some(block_id) = old_task.block_id.as_deref()
                && planner.task_was_created(&file.path, block_id)
            {
                continue;
            }
            if is_merge_retirement(&current_lines, new_task) {
                continue;
            }
            if !block_has_wikilink(&original_lines, old_task) {
                continue;
            }
            candidates.push(Candidate {
                path: relative_path.clone(),
                absolute: file.path.clone(),
                original_index: old_task.line_index,
                before: old_task.status_symbol,
                after: new_task.status_symbol,
                original_line: original_lines
                    .get(old_task.line_index)
                    .copied()
                    .unwrap_or("")
                    .to_string(),
                block_id: old_task.block_id.clone(),
            });
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let resolver = VaultLinkResolver::new(&request.bob_dir);
    let mut targets = Vec::new();
    for candidate in &candidates {
        let Some(file) = planner
            .files
            .iter()
            .find(|file| file.path == candidate.absolute)
        else {
            continue;
        };
        let original_lines: Vec<&str> = file.original.lines().collect();
        let mails = linked_mail_notes(
            &request.bob_dir,
            Path::new(&candidate.path),
            &original_lines,
            candidate.original_index,
            &resolver,
        );
        if mails.is_empty() {
            continue;
        }
        targets.push(CloseTarget {
            path: candidate.path.clone(),
            line: candidate.original_index + 1,
            before: candidate.before,
            after: candidate.after,
            task_text: task_text(&candidate.original_line),
            mails,
        });
    }
    if targets.is_empty() {
        return None;
    }

    let now = now_stamp();
    let exclude: BTreeSet<(String, usize)> = targets
        .iter()
        .map(|target| (target.path.clone(), target.line))
        .collect();
    let scan = watch_scan(&request.bob_dir);
    let copy = build_copy(
        &targets,
        &now,
        &|note| other_open_linker_count(note, &exclude, &scan),
        archive_on(),
    );
    let digest = close_digest(
        targets
            .iter()
            .map(|target| DigestRow {
                path: &target.path,
                line: target.line,
                before: target.before,
                after: target.after,
                mails: target
                    .mails
                    .iter()
                    .map(|mail| mail.note.as_str())
                    .collect(),
            })
            .collect(),
    );
    let tasks = targets
        .iter()
        .map(|target| {
            let fallback = if target.task_text.trim().is_empty() {
                task_text("(task)")
            } else {
                target.task_text.clone()
            };
            let chosen = target
                .mails
                .iter()
                .find(|mail| mail.turn.as_deref() == Some("yours"))
                .or_else(|| target.mails.first());
            let (label, line) = match chosen {
                Some(mail) => {
                    (row_label(mail, &fallback), turn_line(mail, &now))
                }
                None => (fallback, None),
            };
            MailCloseTask {
                path: target.path.clone(),
                line: target.line,
                block_id: candidates
                    .iter()
                    .find(|candidate| {
                        candidate.path == target.path
                            && candidate.original_index + 1 == target.line
                    })
                    .and_then(|candidate| candidate.block_id.clone()),
                before: target.before.to_string(),
                after: target.after.to_string(),
                verb: verb_name(target.after).to_string(),
                label,
                turn_line: line,
                mails: target.mails.iter().map(MailCloseMail::from).collect(),
            }
        })
        .collect();
    Some(MailCloseReport {
        digest: digest.clone(),
        verb: copy.verb,
        title: copy.title,
        subtitle: copy.subtitle,
        tone: copy.tone,
        consequence: copy.consequence,
        confirm: confirmation_command(request, &digest),
        tasks,
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn confirmation_command(request: &CaptureRequest, digest: &str) -> String {
    let mut parts = vec!["bob capture".to_string()];
    parts.push(format!("--mail-close {}", shell_quote(digest)));
    if let Some(route) = request.forced_route.as_deref() {
        parts.push(format!("--route {}", shell_quote(route)));
    }
    if let Some(section) = request.forced_section.as_deref() {
        parts.push(format!("--section {}", shell_quote(section)));
    }
    if let Some(SubBulletTarget::BlockId(id)) =
        request.forced_sub_bullet_target.as_ref()
    {
        parts.push(format!("--task {}", shell_quote(id)));
    }
    if let Some(drawer) = request.forced_drawer.as_deref() {
        parts.push(format!("--drawer {}", shell_quote(drawer)));
    }
    if let Some(picks) = request.pull_in.as_ref() {
        parts.push(format!("--pull-in {}", shell_quote(&pull_in_value(picks))));
    }
    for pick in &request.inbox_routes {
        parts.push(format!(
            "--inbox-route {}",
            shell_quote(&inbox_route_value(pick))
        ));
    }
    if let Some(picks) = request.move_in.as_ref() {
        parts.push(format!("--move-in {}", shell_quote(&move_in_value(picks))));
    }
    for offer in &request.complete_offers {
        parts.push(format!("--complete-offer {}", shell_quote(offer)));
    }
    for offer in &request.cancel_offers {
        parts.push(format!("--cancel-offer {}", shell_quote(offer)));
    }
    if let Some(jump) = request.jump_offer.as_deref() {
        parts.push(format!("--jump-offer {}", shell_quote(jump)));
    }
    for carry in &request.carry {
        parts.push(format!("--carry {}", shell_quote(carry)));
    }
    if request.forced_clip.is_some() {
        parts.push("--clip".to_string());
    }
    if request.no_clip {
        parts.push("--no-clip".to_string());
    }
    if request.no_ref {
        parts.push("--no-ref".to_string());
    }
    parts.push("--".to_string());
    parts.push(shell_quote(&request.raw_text));
    parts.join(" ")
}

fn pull_in_value(picks: &PullInPicks) -> String {
    match picks {
        PullInPicks::All => "all".to_string(),
        PullInPicks::None => "none".to_string(),
        PullInPicks::Tokens { numbers, keys } => {
            let mut tokens: Vec<String> =
                numbers.iter().map(ToString::to_string).collect();
            tokens.extend(keys.iter().cloned());
            tokens.join(",")
        }
    }
}

fn inbox_route_value(pick: &InboxRoutePick) -> String {
    match pick {
        InboxRoutePick::KeepAll => "keep".to_string(),
        InboxRoutePick::Answer {
            selector,
            destination,
        } => match destination {
            Some(note) => format!("{selector}={note}"),
            None => format!("{selector}="),
        },
    }
}

fn move_in_value(picks: &super::move_choice::MoveInPicks) -> String {
    match picks {
        super::move_choice::MoveInPicks::All => "all".to_string(),
        super::move_choice::MoveInPicks::None => "none".to_string(),
        super::move_choice::MoveInPicks::Tokens(tokens) => tokens
            .iter()
            .map(|pick| {
                if pick.sigil == ':' {
                    format!(":{}", pick.selector)
                } else {
                    pick.selector.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// Refuse a real run that would close a Gmail Task without a matching
/// digest. Exit 1, write nothing.
pub(super) fn refuse_unconfirmed(
    request: &CaptureRequest,
    report: &MailCloseReport,
) -> Result<(), CaptureError> {
    match request.mail_close.as_deref() {
        Some(digest) if digest == report.digest => Ok(()),
        Some(_) => Err(CaptureError::mail_close_confirmation_mismatch(report)),
        None => Err(CaptureError::mail_close_confirmation_required(report)),
    }
}

impl CaptureError {
    fn mail_close_extra(
        report: &MailCloseReport,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "mail_close".to_string(),
            serde_json::to_value(report).expect("serialize mail_close"),
        );
        extra
    }

    pub(super) fn mail_close_confirmation_required(
        report: &MailCloseReport,
    ) -> Self {
        let count = report.tasks.len();
        let task_word = if count == 1 { "task" } else { "tasks" };
        Self {
            kind: CaptureErrorKind::Io,
            message: format!(
                "this capture closes {count} email {task_word}; pass --mail-close with the preview digest"
            ),
            code: Some("mail_close_confirmation_required".to_string()),
            extra: Some(Self::mail_close_extra(report)),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    pub(super) fn mail_close_confirmation_mismatch(
        report: &MailCloseReport,
    ) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: "--mail-close digest does not match this capture's email-task closes"
                .to_string(),
            code: Some("mail_close_confirmation_mismatch".to_string()),
            extra: Some(Self::mail_close_extra(report)),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }
}

/// Colored Mail Close Card block for human dry-run and refusal output.
pub(super) fn print_mail_close_block(
    report: &MailCloseReport,
    nothing_written: bool,
) {
    let styler = Styler::detect();
    let paint = |text: &str| {
        if report.tone == "warning" {
            styler.orange(text)
        } else {
            styler.yellow(text)
        }
    };
    println!();
    println!("{}  {}", paint("✉"), styler.bold(&report.title));
    println!("      {}", styler.dim(&report.subtitle));
    for task in &report.tasks {
        println!();
        println!("      {}", task.label);
        if let Some(turn) = task.turn_line.as_deref() {
            let turn_text = if report.tone == "warning" {
                paint(turn)
            } else {
                styler.dim(turn)
            };
            println!("      {turn_text}");
        }
        if report.tasks.len() > 1 {
            println!(
                "      {}",
                styler.dim(&format!("{}:{}", task.path, task.line))
            );
        }
    }
    println!();
    println!("      {}", styler.dim(&report.consequence));
    if nothing_written {
        println!();
        println!("      {}", styler.dim("Nothing was written."));
    }
    println!();
    println!(
        "      {}",
        styler.dim(&format!("Confirm with: {}", report.confirm))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a"), "'a'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn open_and_closed_match_the_contract() {
        assert!(is_open(' '));
        assert!(is_open('*'));
        assert!(is_open('/'));
        assert!(is_open('?'));
        assert!(!is_open('x'));
        assert!(is_closed('x'));
        assert!(is_closed('X'));
        assert!(is_closed('-'));
        assert!(!is_closed(' '));
    }
}

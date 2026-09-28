//! `bob gkeep list`: the Keep/vault reconciliation view (default subcommand).
//!
//! Two sections side by side: the Keep inbox (classified per note into
//! `new`/`pending`/`revised` skips) and the open tasks in the target
//! note, plus a footer naming the next command. `-s vault` reads no
//! config and makes no adapter call, so it stays fast enough for
//! widgets; a Keep failure still prints the vault section and exits 1.

use std::{collections::BTreeMap, fs, path::PathBuf};

use serde_json::{json, Value};

use super::{
    adapter::{AdapterClient, Credentials},
    cli::ListSource,
    config::{GkeepConfig, DEFAULT_TARGET},
    ledger::{read_target_tasks, Journal, Ledger, VaultTask},
    model::{KeepNote, KeepNoteKind},
    plan::{classify, NoteState, Plan, PlanOptions},
    render::{display_title, note_counts},
    ui::{format_age, report_error, warn},
};
use super::{GkeepError, ListArgs};
use crate::native::{
    env as bob_env, note_tasks,
    style::{self, Styler},
};

/// Run the `list` subcommand (or the default top-level command).
pub(crate) fn run(args: &ListArgs) -> i32 {
    let bob_dir = args.bob_dir();
    let show_keep = args.source != ListSource::Vault;
    let show_vault = args.source != ListSource::Keep;
    let styler = Styler::detect();

    // The target name prefers the configured target, but `-s vault`
    // must work with no gkeep config at all, so a failed resolve falls
    // back to the default instead of exiting.
    let config = GkeepConfig::resolve(None);
    let target_rel = config
        .as_ref()
        .map(|config| config.target().to_string())
        .unwrap_or_else(|_| DEFAULT_TARGET.to_string());
    let config = match (show_keep, config) {
        (true, Ok(config)) => Some(config),
        (true, Err(error)) => {
            return report_error("list", &error, args.error_format());
        }
        (false, _) => None,
    };

    let vault = match read_vault(&bob_dir, &target_rel, args.all) {
        Ok(vault) => vault,
        Err(error) => {
            return report_error("list", &error, args.error_format());
        }
    };

    if !show_keep {
        if args.format.is_json() {
            print_json(args, &vault, None, None);
        } else {
            print_vault_table(&vault, None, args.all, &styler);
        }
        return 0;
    }
    let config = config.expect("config resolves when Keep is shown");

    let (token, _) = match config.read_token() {
        Ok(token) => token,
        Err(error) => {
            return report_error("list", &error, args.error_format());
        }
    };
    let client = match AdapterClient::resolve(&config) {
        Ok(client) => client,
        Err(error) => {
            return report_error("list", &error, args.error_format());
        }
    };
    let spinner_label = if args.format.is_json() {
        None
    } else {
        Some("Syncing Google Keep")
    };
    let credentials = Credentials::from_config(&config, &token);
    let notes = match client.snapshot(&credentials, args.all, spinner_label) {
        Ok(notes) => notes,
        Err(error) => return print_keep_failure(args, &vault, &error),
    };

    // Archived notes are listed only with `--all`, even if the adapter
    // returns them without being asked.
    let visible: Vec<KeepNote> = notes
        .into_iter()
        .filter(|note| args.all || !note.archived)
        .collect();
    let plan = classify(
        &visible,
        &vault.ledger,
        &vault.journal,
        &PlanOptions::default(),
    );
    let fetched_at = super::ui::now_utc()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let keep = KeepView {
        account: config.email().to_string(),
        fetched_at,
        plan,
    };

    if args.format.is_json() {
        print_json(args, &vault, Some(&keep), None);
    } else {
        print_keep_table(&keep, &styler);
        if show_vault {
            println!();
            print_vault_table(&vault, Some(&keep.plan), args.all, &styler);
        }
        println!();
        println!("{}", footer(&keep.plan));
    }
    0
}

/// The fetched Keep side: the account, the fetch time, and the plan.
struct KeepView {
    account: String,
    fetched_at: String,
    plan: Plan,
}

/// The vault side: target tasks plus the ledger and journal behind them.
struct VaultData {
    target_rel: String,
    target_path: PathBuf,
    tasks: Vec<VaultTask>,
    ledger: Ledger,
    journal: Journal,
    missing_target: bool,
}

/// Scan the ledger and journal, warn about integrity issues, and read
/// the target note's tasks (open only, unless `--all`).
fn read_vault(
    bob_dir: &std::path::Path,
    target_rel: &str,
    show_all: bool,
) -> Result<VaultData, GkeepError> {
    let ledger = Ledger::scan(bob_dir).map_err(|error| {
        GkeepError::setup("vault", format!("scan the vault: {error}"))
    })?;
    let journal_path = bob_env::bob_cli_state_dir()
        .join("gkeep")
        .join("journal.jsonl");
    let journal = Journal::read(&journal_path).map_err(|error| {
        GkeepError::setup("vault", format!("read the journal: {error}"))
    })?;
    if journal.skipped > 0 {
        warn(&format!(
            "skipped {} corrupt journal lines in {}",
            journal.skipped,
            journal_path.display()
        ));
    }
    for group in ledger.duplicates() {
        warn(&format!(
            "duplicate gkeep marker {}:{} at {}",
            group.id,
            group.fp,
            group.locations.join(", ")
        ));
    }
    let target_path = bob_dir.join(target_rel);
    let (contents, missing_target) = match fs::read_to_string(&target_path) {
        Ok(contents) => (contents, false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (String::new(), true)
        }
        Err(_) => (String::new(), false),
    };
    let settings = note_tasks::read_settings(bob_dir);
    let mut tasks = read_target_tasks(&contents, &settings);
    if !show_all {
        tasks.retain(|task| is_open_status(task.status_symbol));
    }
    // Vault rows oldest first: by `created`, then line. Rows without
    // `created` go last, in file order.
    tasks.sort_by(|a, b| match (a.created, b.created) {
        (Some(da), Some(db)) => (da, a.line).cmp(&(db, b.line)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.line.cmp(&b.line),
    });
    Ok(VaultData {
        target_rel: target_rel.to_string(),
        target_path,
        tasks,
        ledger,
        journal,
        missing_target,
    })
}

/// Whether a vault task counts as open: anything but done (`x`) or
/// cancelled (`-`).
fn is_open_status(symbol: char) -> bool {
    !matches!(symbol, 'x' | 'X' | '-')
}

/// A Keep failure still prints the vault section (unless `-s keep`),
/// with the red error and hint in the Keep section. Always exits 1.
fn print_keep_failure(
    args: &ListArgs,
    vault: &VaultData,
    error: &GkeepError,
) -> i32 {
    if args.format.is_json() {
        print_json(args, vault, None, Some(error));
        return 1;
    }
    let styler = Styler::detect();
    println!("{}", styler.cyan("Google Keep"));
    println!();
    println!("  {}: {}", styler.red("error"), error.message());
    if let Some(hint) = error.hint() {
        println!("{}", styler.dim(&format!("  hint: {hint}")));
    }
    if args.source != ListSource::Keep {
        println!();
        print_vault_table(vault, None, args.all, &styler);
    }
    1
}

/// The Keep section: one row per planned note, oldest first.
fn print_keep_table(keep: &KeepView, styler: &Styler) {
    let rows: Vec<KeepRow> = keep
        .plan
        .notes
        .iter()
        .map(|planned| KeepRow::new(planned.note.clone(), planned))
        .collect();
    let inbox = rows
        .iter()
        .filter(|row| row.state != NoteState::Archived)
        .count();
    let archived = rows.len() - inbox;
    let mut title = format!("Google Keep · {inbox} in inbox");
    if archived > 0 {
        title.push_str(&format!(" · {archived} archived"));
    }
    title.push_str(&format!(" · {}", keep.account));
    println!("{}", styler.cyan(&title));
    println!();
    if rows.is_empty() {
        println!("{}", styler.dim("  Keep inbox is empty ✓"));
        return;
    }

    let ref_width = 7;
    let age_width = rows
        .iter()
        .map(|row| style::display_width(&row.age))
        .max()
        .unwrap_or(3)
        .max(3);
    let kind_width = rows
        .iter()
        .map(|row| style::display_width(row.kind))
        .max()
        .unwrap_or(4)
        .max(4);
    let state_width = rows
        .iter()
        .map(|row| style::display_width(row.state.as_str()))
        .max()
        .unwrap_or(5)
        .max(5);
    println!(
        "  {}  {}  {}  {}  NOTE",
        style::pad_right("REF", ref_width),
        style::pad_right("AGE", age_width),
        style::pad_right("KIND", kind_width),
        style::pad_right("STATE", state_width),
    );
    let prefix_width =
        2 + ref_width + 2 + age_width + 2 + kind_width + 2 + state_width + 2;
    let available = style::terminal_width().saturating_sub(prefix_width);
    for row in &rows {
        let state_text = style::pad_right(row.state.as_str(), state_width);
        let state = match row.state {
            NoteState::New => styler.green(&state_text),
            NoteState::Pending | NoteState::Revised => {
                styler.yellow(&state_text)
            }
            NoteState::Empty
            | NoteState::Pinned
            | NoteState::Shared
            | NoteState::Archived => styler.dim(&state_text),
        };
        println!(
            "  {}  {}  {}  {state}  {}",
            styler.dim(&style::pad_right(&row.ref_, ref_width)),
            style::pad_right(&row.age, age_width),
            style::pad_right(row.kind, kind_width),
            row.note_cell(available, styler),
        );
    }
}

/// One Keep table row, precomputed for width layout.
struct KeepRow {
    ref_: String,
    age: String,
    kind: &'static str,
    state: NoteState,
    title: String,
    hints: String,
}

impl KeepRow {
    fn new(note: KeepNote, planned: &super::plan::PlannedNote) -> Self {
        let now = super::ui::now_utc().timestamp();
        let age = match note.created_local() {
            Some(created) => format_age(now, created.timestamp()),
            None => "—".to_string(),
        };
        let kind = match note.kind {
            KeepNoteKind::Note => "note",
            KeepNoteKind::List => "list",
        };
        let counts = note_counts(&note);
        let mut hints = Vec::new();
        if counts.extra_lines > 0 {
            hints.push(if counts.extra_lines == 1 {
                "+1 line".to_string()
            } else {
                format!("+{} lines", counts.extra_lines)
            });
        }
        let mut boxes = Vec::new();
        if counts.open_items > 0 {
            boxes.push(format!("☐ {}", counts.open_items));
        }
        if counts.checked_items > 0 {
            boxes.push(format!("☑ {}", counts.checked_items));
        }
        if !boxes.is_empty() {
            hints.push(boxes.join(" "));
        }
        if !note.attachments.is_empty() {
            hints.push(format!("📎 {}", note.attachments.len()));
        }
        Self {
            ref_: planned.ref_.clone(),
            age,
            kind,
            state: planned.state,
            title: display_title(&note),
            hints: if hints.is_empty() {
                String::new()
            } else {
                format!("  {}", hints.join("  "))
            },
        }
    }

    /// The NOTE cell: the title plus dim hints, truncated to fit.
    fn note_cell(&self, available: usize, styler: &Styler) -> String {
        let plain = format!("{}{}", self.title, self.hints);
        if style::display_width(&plain) <= available {
            return format!("{}{}", self.title, styler.dim(&self.hints));
        }
        if self.hints.is_empty() {
            return style::truncate(&self.title, available);
        }
        style::truncate(&plain, available)
    }
}

/// The vault section: one row per target-note task, oldest first.
fn print_vault_table(
    vault: &VaultData,
    plan: Option<&Plan>,
    show_all: bool,
    styler: &Styler,
) {
    let active: BTreeMap<&str, NoteState> = plan
        .map(|plan| {
            plan.notes
                .iter()
                .map(|planned| (planned.note.id.as_str(), planned.state))
                .collect()
        })
        .unwrap_or_default();
    let count = vault.tasks.len();
    // `read_vault` already filters to open tasks unless `--all`, so the
    // title counts what is shown.
    let title = format!(
        "{} · {count} {} · {}",
        vault.target_rel,
        if show_all { "tasks" } else { "open" },
        display_path(&vault.target_path),
    );
    println!("{}", styler.cyan(&title));
    println!();
    if vault.tasks.is_empty() {
        if vault.missing_target {
            println!(
                "{}",
                styler.dim(&format!(
                    "  {} not found · create it or set gkeep.target",
                    vault.target_rel
                ))
            );
        } else if show_all {
            println!("{}", styler.dim("  No tasks"));
        } else {
            println!("{}", styler.dim("  No open tasks"));
        }
        return;
    }
    let now = super::ui::now_utc().timestamp();
    let ages: Vec<String> = vault
        .tasks
        .iter()
        .map(|task| vault_age(task, now))
        .collect();
    let age_width = ages
        .iter()
        .map(|age| style::display_width(age))
        .max()
        .unwrap_or(3)
        .max(3);
    println!("  {}  STATUS  TASK", style::pad_right("AGE", age_width));
    let prefix_width = 2 + age_width + 2 + 6 + 2;
    let available = style::terminal_width().saturating_sub(prefix_width);
    for (task, age) in vault.tasks.iter().zip(&ages) {
        let mut hints = Vec::new();
        let mut boxes = Vec::new();
        if task.open_items > 0 {
            boxes.push(format!("☐ {}", task.open_items));
        }
        if task.checked_items > 0 {
            boxes.push(format!("☑ {}", task.checked_items));
        }
        if !boxes.is_empty() {
            hints.push(boxes.join(" "));
        }
        if active
            .get(task.marker.as_ref().map_or("", |(id, _)| id.as_str()))
            .is_some_and(|state| *state != NoteState::Archived)
        {
            hints.push("↺ still in Keep".to_string());
        }
        let suffix = if hints.is_empty() {
            String::new()
        } else {
            format!("  {}", hints.join("  "))
        };
        let plain = format!("{}{}", task.description, suffix);
        let cell = if style::display_width(&plain) <= available {
            format!("{}{}", task.description, styler.dim(&suffix))
        } else if suffix.is_empty() {
            style::truncate(&task.description, available)
        } else {
            style::truncate(&plain, available)
        };
        // Pad after painting: the header is 6 wide while `[ ]` is 3.
        let status_plain = format!("[{}]", task.status_symbol);
        let status_cell = format!(
            "{}{}",
            super::ui::paint_status_symbol(styler, task.status_symbol),
            " ".repeat(
                6usize.saturating_sub(style::display_width(&status_plain))
            ),
        );
        println!(
            "  {}  {status_cell}  {cell}",
            style::pad_right(age, age_width),
        );
    }
}

/// The vault task's age from its `[created::…]` date, else `—`.
fn vault_age(task: &VaultTask, now: i64) -> String {
    match task.created {
        Some(date) => {
            let then = date
                .and_hms_opt(0, 0, 0)
                .map(|start| super::ui::local_naive_to_utc(&start).timestamp())
                .unwrap_or(now);
            format_age(now, then)
        }
        None => "—".to_string(),
    }
}

/// Collapse a `$HOME` prefix to `~` for section titles.
fn display_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    let home = bob_env::home_dir();
    if let Some(home) = home.to_str()
        && !home.is_empty()
        && let Some(rest) = text.strip_prefix(home)
    {
        if rest.is_empty() {
            return "~".to_string();
        }
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    text.into_owned()
}

/// The footer: non-zero state counts, then the next command when
/// anything is actionable, else the all-clear line.
fn footer(plan: &Plan) -> String {
    let summary = &plan.summary;
    let actionable = summary.new + summary.pending + summary.revised;
    if actionable == 0 {
        let mut line = "✓ Keep inbox is clear".to_string();
        let pinned = plan
            .notes
            .iter()
            .filter(|planned| planned.state == NoteState::Pinned)
            .count();
        if pinned > 0 {
            line.push_str(&format!(" · {pinned} pinned stays in Keep"));
        }
        let shared = plan
            .notes
            .iter()
            .filter(|planned| planned.state == NoteState::Shared)
            .count();
        if shared > 0 {
            line.push_str(&format!(" · {shared} shared stays in Keep"));
        }
        return line;
    }
    let mut parts = Vec::new();
    if summary.new > 0 {
        parts.push(format!("{} new", summary.new));
    }
    if summary.pending > 0 {
        parts.push(format!("{} pending archive", summary.pending));
    }
    if summary.revised > 0 {
        parts.push(format!("{} revised", summary.revised));
    }
    for state in [NoteState::Pinned, NoteState::Shared] {
        let count = plan
            .notes
            .iter()
            .filter(|planned| planned.state == state)
            .count();
        if count > 0 {
            parts.push(format!("{count} {} stays in Keep", state.as_str()));
        }
    }
    let empty = plan
        .notes
        .iter()
        .filter(|planned| planned.state == NoteState::Empty)
        .count();
    if empty > 0 {
        parts.push(format!("{empty} empty skipped"));
    }
    let archived = plan
        .notes
        .iter()
        .filter(|planned| planned.state == NoteState::Archived)
        .count();
    if archived > 0 {
        parts.push(format!("{archived} archived"));
    }
    format!("{}  →  bob gkeep pull", parts.join(" · "))
}

/// The `schema_version: 1` JSON document for `-f json`.
fn print_json(
    args: &ListArgs,
    vault: &VaultData,
    keep: Option<&KeepView>,
    keep_error: Option<&GkeepError>,
) {
    let keep_value = match (keep, keep_error) {
        (Some(keep), _) => {
            let notes: Vec<Value> = keep
                .plan
                .notes
                .iter()
                .map(|planned| {
                    let counts = note_counts(&planned.note);
                    let kind = match planned.note.kind {
                        KeepNoteKind::Note => "note",
                        KeepNoteKind::List => "list",
                    };
                    json!({
                        "id": planned.note.id,
                        "ref": planned.ref_,
                        "kind": kind,
                        "title": display_title(&planned.note),
                        "state": planned.state.as_str(),
                        "pinned": planned.note.pinned,
                        "shared": planned.note.shared,
                        "archived": planned.note.archived,
                        "labels": planned.note.labels,
                        "created": planned.note.created,
                        "edited": planned.note.edited,
                        "url": planned.note.url,
                        "fingerprint": planned.note.content.fingerprint(),
                        "lines": counts.extra_lines,
                        "items_open": counts.open_items,
                        "items_checked": counts.checked_items,
                        "attachments": planned.note.attachments.len(),
                    })
                })
                .collect();
            json!({
                "account": keep.account,
                "fetched_at": keep.fetched_at,
                "notes": notes,
            })
        }
        (None, Some(error)) => json!({
            "error": {
                "kind": error.kind(),
                "message": error.message(),
                "hint": error.hint(),
            },
        }),
        (None, None) => Value::Null,
    };
    let states: BTreeMap<&str, &str> = keep
        .map(|keep| {
            keep.plan
                .notes
                .iter()
                .map(|planned| {
                    (planned.note.id.as_str(), planned.state.as_str())
                })
                .collect()
        })
        .unwrap_or_default();
    let vault_value = if args.source == ListSource::Keep {
        Value::Null
    } else {
        let tasks: Vec<Value> = vault
            .tasks
            .iter()
            .map(|task| {
                let (keep_id, keep_state): (Option<&str>, Option<&str>) =
                    match &task.marker {
                        Some((id, _)) => (
                            Some(id.as_str()),
                            states.get(id.as_str()).copied(),
                        ),
                        None => (None, None),
                    };
                json!({
                    "line": task.line,
                    "status": format!("[{}]", task.status_symbol),
                    "description": task.description,
                    "created": task.created.map(|date| date.format("%Y-%m-%d").to_string()),
                    "keep_id": keep_id,
                    "keep_state": keep_state,
                })
            })
            .collect();
        json!({
            "path": vault.target_rel,
            "tasks": tasks,
        })
    };
    let (new, pending, revised, skipped) = keep
        .map(|keep| {
            (
                keep.plan.summary.new,
                keep.plan.summary.pending,
                keep.plan.summary.revised,
                keep.plan.summary.skipped,
            )
        })
        .unwrap_or_default();
    let document = json!({
        "schema_version": 1,
        "ok": keep_error.is_none(),
        "keep": keep_value,
        "vault": vault_value,
        "summary": {
            "new": new,
            "pending": pending,
            "revised": revised,
            "skipped": skipped,
            "duplicates": vault.ledger.duplicates().len(),
        },
    });
    println!("{document}");
}

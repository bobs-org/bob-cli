//! `bob gcal doctor`: diagnose the Calendar setup as a checklist.

use std::path::Path;

use serde_json::json;

use super::super::{
    config as bob_config, env as bob_env, pomodoro, pomodoro_homes::HomeKind,
    pomodoro_themes, style::Styler, vault_sync,
};
use super::{
    account,
    client::peek_project,
    config::GcalConfig,
    gog::{Gog, CLIENT},
    journal,
    model::{GcalEvent, MyAnswer, ParsedNote},
    root::validate_root_candidate,
    ui, DoctorArgs, GcalError,
};
use crate::native::google::doctor::{Check, CheckStatus};

pub(crate) fn run(args: &DoctorArgs) -> i32 {
    let bob_dir = args.bob_dir();
    let host = vault_sync::hostname_slug();
    let config = match GcalConfig::resolve() {
        Ok(config) => config,
        Err(error) => {
            let mut checks = vec![Check {
                id: "gog",
                status: CheckStatus::Fail,
                summary: error.message().to_string(),
                fix: error.hint().map(str::to_string),
            }];
            for id in [
                "client",
                "account",
                "token",
                "calendars",
                "default_root",
                "today",
                "journal",
                "notes",
                "writer",
            ] {
                checks.push(skipped(id, "no gcal config"));
            }
            return finish(args, &host, checks);
        }
    };

    let mut checks = Vec::new();
    let gog = match Gog::resolve(
        crate::native::google::GCAL,
        config.gog(),
        config.timeout(),
    ) {
        Ok(gog) => match gog.require_version() {
            Ok(version) => {
                checks.push(Check {
                    id: "gog",
                    status: CheckStatus::Ok,
                    summary: format!(
                        "v{} · {} · store {}",
                        version.trim_start_matches('v'),
                        gog.program().display(),
                        crate::native::google::gog::gog_store_display()
                    ),
                    fix: None,
                });
                Some(gog)
            }
            Err(error) => {
                checks.push(fail_from(&error, "gog"));
                None
            }
        },
        Err(error) => {
            checks.push(fail_from(&error, "gog"));
            None
        }
    };

    let mut selected_account = None;
    match &gog {
        Some(gog) => {
            match gog.credentials_list() {
                Ok(clients) => {
                    if let Some(client) =
                        clients.iter().find(|c| c.client == CLIENT)
                    {
                        let project = client.path.as_deref().and_then(|path| {
                            std::fs::read(path)
                                .ok()
                                .and_then(|bytes| peek_project(&bytes))
                        });
                        checks.push(Check {
                            id: "client",
                            status: CheckStatus::Ok,
                            summary: project.map_or_else(
                                || CLIENT.to_string(),
                                |project| {
                                    format!("{CLIENT} · project {project}")
                                },
                            ),
                            fix: None,
                        });
                    } else {
                        checks.push(Check {
                            id: "client",
                            status: CheckStatus::Fail,
                            summary: format!(
                                "no bob-gcal OAuth client is stored in {}",
                                crate::native::google::gog::gog_store_display()
                            ),
                            fix: Some("run `bob gcal login`".to_string()),
                        });
                    }
                }
                Err(error) => checks.push(fail_from(&error, "client")),
            }

            match account::resolve_command(gog, config.account()) {
                Ok(email) => {
                    selected_account = Some(email.clone());
                    match gog.accounts(Some(CLIENT)) {
                        Ok(entries) => {
                            let connected = entries.iter().any(|entry| {
                                entry.email.eq_ignore_ascii_case(&email)
                                    && entry.has_service_scope(
                                        &crate::native::google::GCAL,
                                    )
                            });
                            if connected {
                                checks.push(Check {
                                    id: "account",
                                    status: CheckStatus::Ok,
                                    summary: format!(
                                        "{email} · read-only Calendar access"
                                    ),
                                    fix: None,
                                });
                            } else {
                                checks.push(Check {
                                    id: "account",
                                    status: CheckStatus::Fail,
                                    summary: format!(
                                        "{email} · no bob-gcal Calendar grant"
                                    ),
                                    fix: Some(
                                        "run `bob gcal login`".to_string(),
                                    ),
                                });
                            }
                        }
                        Err(error) => checks.push(fail_from(&error, "account")),
                    }
                }
                Err(error) => {
                    checks.push(fail_from(&error, "account"));
                }
            }
        }
        None => {
            for id in ["client", "account"] {
                checks.push(skipped(id, "no gog"));
            }
        }
    }

    let calendars = match (gog.as_ref(), selected_account.as_deref()) {
        (Some(gog), Some(email)) => match gog.calendars(email) {
            Ok(calendars) => {
                checks.push(Check {
                    id: "token",
                    status: CheckStatus::Ok,
                    summary: "refreshes".to_string(),
                    fix: None,
                });
                Some(calendars)
            }
            Err(error) => {
                let mut check = fail_from(&error, "token");
                if error.kind() == "invalid_grant" {
                    check.fix = Some(
                        "run `bob gcal login` (it signs in again automatically)"
                            .to_string(),
                    );
                }
                checks.push(check);
                checks.push(skipped("calendars", "token did not refresh"));
                None
            }
        },
        _ => {
            checks.push(skipped("token", "no account"));
            checks.push(skipped("calendars", "no account"));
            None
        }
    };

    if let Some(calendars) = calendars {
        let mut missing = Vec::new();
        for configured in config.calendars() {
            let found = calendars
                .iter()
                .any(|calendar| calendar.matches_configured_id(&configured.id));
            if !found {
                missing.push(configured.id.clone());
            }
        }
        if missing.is_empty() {
            let names: Vec<_> = config
                .calendars()
                .iter()
                .map(|calendar| calendar.id.as_str())
                .collect();
            checks.push(Check {
                id: "calendars",
                status: CheckStatus::Ok,
                summary: format!("{} reachable", names.join(", ")),
                fix: None,
            });
        } else {
            checks.push(Check {
                id: "calendars",
                status: CheckStatus::Fail,
                summary: format!("not reachable: {}", missing.join(", ")),
                fix: Some(
                    "run `bob gcal calendars` and set gcal.calendars to ids you own"
                        .to_string(),
                ),
            });
        }
    }

    checks.push(default_root_check(&bob_dir, config.root()));
    checks.push(today_check(&bob_dir));
    checks.push(journal_check(&bob_dir));
    checks.push(journal_malformed_check(&bob_dir));
    checks.push(notes_check(
        &bob_dir,
        selected_account.as_deref(),
        gog.as_ref(),
    ));
    checks.push(writer_check(config.writer(), &host));
    checks.push(last_sync_check());
    finish(args, &host, checks)
}

const NOTES_LOOKUP_CAP: usize = 25;

fn notes_check(
    vault: &Path,
    account: Option<&str>,
    gog: Option<&Gog>,
) -> Check {
    let Some(account) = account else {
        return skipped("notes", "no resolved account");
    };
    let Ok((book, _)) = journal::load_notes(vault) else {
        return Check {
            id: "notes",
            status: CheckStatus::Warn,
            summary: "could not read private event notes".to_string(),
            fix: Some("inspect .bob/gcal/<host>.jsonl".to_string()),
        };
    };
    let live_mine: Vec<_> = book
        .iter()
        .filter(|record| {
            !record.cleared && record.account.eq_ignore_ascii_case(account)
        })
        .collect();
    let foreign: Vec<_> = book
        .iter()
        .filter(|record| {
            !record.cleared && !record.account.eq_ignore_ascii_case(account)
        })
        .collect();
    let daily_rel = pomodoro::day_file_for(vault)
        .strip_prefix(vault)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let unresolved: Vec<_> = live_mine
        .iter()
        .copied()
        .filter(|record| {
            let parsed = ParsedNote::parse(record.scope, &record.text);
            parsed.description.directives.route.as_deref().is_some_and(
                |route| {
                    validate_root_candidate(vault, &daily_rel, route).is_err()
                },
            )
        })
        .collect();
    let mut orphans = Vec::new();
    let mut not_checked = 0usize;
    if let Some(gog) = gog {
        for (index, record) in live_mine.iter().enumerate() {
            if index >= NOTES_LOOKUP_CAP {
                not_checked += live_mine.len() - NOTES_LOOKUP_CAP;
                break;
            }
            match gog.event(&record.calendar_id, &record.key, account) {
                Ok(raw) => {
                    match GcalEvent::from_google(&record.calendar_id, raw) {
                        Ok(event)
                            if event.status == "cancelled"
                                || matches!(event.my_answer, MyAnswer::No) =>
                        {
                            orphans.push(*record);
                        }
                        Ok(_) => {}
                        Err(_) => not_checked += 1,
                    }
                }
                Err(error) if is_notes_not_found(&error) => {
                    orphans.push(*record);
                }
                Err(_) => not_checked += 1,
            }
        }
    } else {
        not_checked += live_mine.len();
    }
    let malformed = book.malformed;
    if orphans.is_empty()
        && unresolved.is_empty()
        && foreign.is_empty()
        && malformed == 0
        && not_checked == 0
    {
        let count = live_mine.len();
        return Check {
            id: "notes",
            status: CheckStatus::Ok,
            summary: format!(
                "{count} private note{}",
                if count == 1 { "" } else { "s" }
            ),
            fix: None,
        };
    }
    let mut issues = Vec::new();
    let mut fixes = Vec::new();
    if !orphans.is_empty() {
        issues.push(format!("{} orphaned", orphans.len()));
        let first = orphans[0];
        fixes.push(match first.scope {
            journal::NoteScope::Series => format!(
                "bob gcal note -c -s series -i {} -C {}",
                first.key, first.calendar_id
            ),
            journal::NoteScope::Event => format!(
                "bob gcal note -c -i {} -C {}",
                first.key, first.calendar_id
            ),
        });
    }
    if !unresolved.is_empty() {
        issues.push(format!(
            "{} unresolved route{}",
            unresolved.len(),
            if unresolved.len() == 1 { "" } else { "s" }
        ));
        let first = unresolved[0];
        // A bare series master has no recurringEventId, so `-e -s series
        // -i <master>` fails `resolve_show_scope`. The series-key clear
        // works with the named identity; an exact occurrence edit needs an
        // occurrence id we must not invent here.
        let command = match first.scope {
            journal::NoteScope::Series => format!(
                "bob gcal note -c -s series -i {} -C {}",
                first.key, first.calendar_id
            ),
            journal::NoteScope::Event => format!(
                "bob gcal note -e -i {} -C {}",
                first.key, first.calendar_id
            ),
        };
        fixes.push(command);
    }
    if !foreign.is_empty() {
        issues.push(format!("{} foreign-account", foreign.len()));
        fixes.push(format!(
            "sign in as {} to apply that account's notes",
            foreign[0].account
        ));
    }
    if malformed > 0 {
        issues.push(format!("{malformed} malformed"));
        fixes.push("inspect .bob/gcal/<host>.jsonl".to_string());
    }
    if not_checked > 0 {
        issues.push(format!("{not_checked} not checked"));
    }
    Check {
        id: "notes",
        status: CheckStatus::Warn,
        summary: issues.join(" · "),
        fix: if fixes.is_empty() {
            None
        } else {
            Some(fixes.join("\n"))
        },
    }
}

fn is_notes_not_found(error: &GcalError) -> bool {
    error.kind() == "not_found"
        || error.message().to_ascii_lowercase().contains("not found")
        || error.message().to_ascii_lowercase().contains("no such")
}

fn fail_from(error: &GcalError, id: &'static str) -> Check {
    Check {
        id,
        status: CheckStatus::Fail,
        summary: error.message().to_string(),
        fix: error.hint().map(str::to_string),
    }
}

fn skipped(id: &'static str, reason: &str) -> Check {
    Check {
        id,
        status: CheckStatus::Skip,
        summary: format!("skipped: {reason}"),
        fix: None,
    }
}

fn default_root_check(bob_dir: &Path, name: &str) -> Check {
    let daily = pomodoro::day_file_for(bob_dir);
    let daily_rel = daily.strip_prefix(bob_dir).unwrap_or(&daily);
    match validate_root_candidate(bob_dir, daily_rel, name) {
        Ok(target) => {
            let styler = Styler::detect();
            let kind = match target.home.kind {
                HomeKind::Area => "area",
                HomeKind::Project => "project",
                HomeKind::Bucket => "bucket",
            };
            let hue =
                ui::hue_dot(&styler, target.home.hue, &target.home.area_label);
            let mut summary =
                format!("{name} → {} · {kind} {hue}", target.path.display());
            if let Some(suggestion) = type_suggestion(bob_dir, &target.path) {
                summary.push_str(" · suggests ");
                summary.push_str(&suggestion);
            }
            Check {
                id: "default_root",
                status: CheckStatus::Ok,
                summary,
                fix: None,
            }
        }
        Err(rejection) => Check {
            id: "default_root",
            status: CheckStatus::Fail,
            summary: rejection.message,
            fix: Some(rejection.fix),
        },
    }
}

fn type_suggestion(vault: &Path, residence: &Path) -> Option<String> {
    let today = bob_env::current_datetime().date();
    let catalog = pomodoro_themes::Catalog::load(vault, today, false).ok()?;
    let plan = bob_config::load_plan_config(&bob_config::config_path())
        .unwrap_or_default();
    let mut resolver = crate::native::pomodoro_homes::HomeResolver::new(
        crate::native::pomodoro_homes::DiskHomeSource::new(vault),
    );
    let result = pomodoro_themes::suggest::suggest_for_new_task(
        &mut resolver,
        &catalog,
        &plan,
        residence,
    );
    result
        .suggestion
        .map(|suggestion| ui::type_swatch(&Styler::detect(), &suggestion.theme))
}

fn today_check(bob_dir: &Path) -> Check {
    let path = pomodoro::day_file_for(bob_dir);
    let display = display_path(&path, bob_dir);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let lines: Vec<&str> = contents.lines().collect();
            if pomodoro::pomodoros_section_range(&lines).is_some() {
                Check {
                    id: "today",
                    status: CheckStatus::Ok,
                    summary: format!("{display} has ## Pomodoros"),
                    fix: None,
                }
            } else {
                Check {
                    id: "today",
                    status: CheckStatus::Fail,
                    summary: format!("{display} has no ## Pomodoros heading"),
                    fix: Some(
                        "open today's daily note so Templater inserts the \
                         Pomodoros section"
                            .to_string(),
                    ),
                }
            }
        }
        Err(_) => Check {
            id: "today",
            status: CheckStatus::Fail,
            summary: format!("{display} is missing"),
            fix: Some("open today's daily note in Obsidian".to_string()),
        },
    }
}

fn journal_check(bob_dir: &Path) -> Check {
    let gitignore = read_optional(&bob_dir.join(".gitignore"));
    let attributes = read_optional(&bob_dir.join(".gitattributes"));
    let tracked = gitignore
        .as_deref()
        .is_some_and(|text| text.contains("!/.bob/gcal/*.jsonl"));
    let union = attributes
        .as_deref()
        .is_some_and(|text| text.contains(".bob/gcal/*.jsonl merge=union"));
    match (tracked, union) {
        (true, true) => Check {
            id: "journal",
            status: CheckStatus::Ok,
            summary: ".bob/gcal/*.jsonl tracked and union-merged".to_string(),
            fix: None,
        },
        _ => {
            let mut missing = Vec::new();
            if !tracked {
                missing.push("gitignore un-ignore !/.bob/gcal/*.jsonl");
            }
            if !union {
                missing.push(".gitattributes merge=union");
            }
            Check {
                id: "journal",
                status: CheckStatus::Fail,
                summary: format!("missing {}", missing.join(" and ")),
                fix: Some(
                    "add `!/.bob/gcal/*.jsonl` to .gitignore and \
                     `.bob/gcal/*.jsonl merge=union` to .gitattributes"
                        .to_string(),
                ),
            }
        }
    }
}

/// Malformed journal lines never fail a run: they are skipped with a
/// bounded warning. Doctor still surfaces the count so a growing pile
/// of unreadable lines gets noticed.
fn journal_malformed_check(bob_dir: &Path) -> Check {
    match journal::malformed_count(bob_dir) {
        0 => Check {
            id: "journal_malformed",
            status: CheckStatus::Ok,
            summary: "no malformed gcal journal lines".to_string(),
            fix: None,
        },
        count => Check {
            id: "journal_malformed",
            status: CheckStatus::Warn,
            summary: format!(
                "{count} malformed gcal journal line{} skipped",
                if count == 1 { "" } else { "s" }
            ),
            fix: Some(
                "malformed lines are skipped safely; re-sync to confirm \
                 the journal still folds, then inspect \
                 .bob/gcal/<host>.jsonl"
                    .to_string(),
            ),
        },
    }
}

fn last_sync_check() -> Check {
    let Some(status) = journal::read_status() else {
        return skipped("last_sync", "never synced");
    };
    if status.outcome == "waiting" {
        return Check {
            id: "last_sync",
            status: CheckStatus::Warn,
            summary: match status.reason.as_deref() {
                Some("note_unrendered") => format!(
                    "last run waited for Templater to render {}",
                    status.day
                ),
                Some("pomodoros_missing") => format!(
                    "last run waited for a ## Pomodoros section in {}",
                    status.day
                ),
                _ => format!("last run waited for {}", status.day),
            },
            fix: Some(
                match status.reason.as_deref() {
                    Some("note_unrendered") => {
                        "let Templater finish rendering the note, then sync"
                    }
                    _ => "open the daily note in Obsidian, then sync",
                }
                .to_string(),
            ),
        };
    }
    if status.outcome == "held" {
        return Check {
            id: "last_sync",
            status: CheckStatus::Warn,
            summary: format!(
                "last sync held {} because its Events section was replaced",
                status.day
            ),
            fix: Some(
                "run `bob gcal sync -F` to restore today's Events".to_string(),
            ),
        };
    }
    if status.outcome == "error" {
        return Check {
            id: "last_sync",
            status: CheckStatus::Fail,
            summary: format!(
                "last sync for {} failed: {}",
                status.day,
                status.error.as_deref().unwrap_or("unknown error")
            ),
            fix: Some("run `bob gcal sync` and read the error".to_string()),
        };
    }
    if let Some(error) =
        status.error.as_deref().filter(|error| !error.is_empty())
    {
        return Check {
            id: "last_sync",
            status: CheckStatus::Warn,
            summary: format!(
                "synced {} on {} · last error: {error}",
                journal::ago(&status.ts),
                status.host,
            ),
            fix: Some("run `bob gcal sync` and read the error".to_string()),
        };
    }
    Check {
        id: "last_sync",
        status: CheckStatus::Ok,
        summary: format!(
            "synced {} on {} · +{} tasks",
            journal::ago(&status.ts),
            status.host,
            status.tasks_added,
        ),
        fix: None,
    }
}

fn writer_check(configured: Option<&str>, host: &str) -> Check {
    match configured {
        None => Check {
            id: "writer",
            status: CheckStatus::Ok,
            summary: format!("{host} · no writer pin"),
            fix: None,
        },
        Some(writer) if writer == host => Check {
            id: "writer",
            status: CheckStatus::Ok,
            summary: "this host".to_string(),
            fix: None,
        },
        Some(writer) => Check {
            id: "writer",
            status: CheckStatus::Fail,
            summary: format!("this host is {host}; writer is {writer}"),
            fix: Some(format!(
                "run sync on {writer}, or set gcal.writer: {host}"
            )),
        },
    }
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn display_path(path: &Path, bob_dir: &Path) -> String {
    path.strip_prefix(bob_dir)
        .map(|rel| rel.display().to_string().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

fn human_label(id: &str) -> &str {
    match id {
        "default_root" => "default root",
        "last_sync" => "last sync",
        other => other,
    }
}

fn finish(args: &DoctorArgs, host: &str, checks: Vec<Check>) -> i32 {
    let failed = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    if args.format.is_json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": failed == 0,
                "host": host,
                "checks": checks
                    .iter()
                    .map(|check| json!({
                        "id": check.id,
                        "status": check.status.as_str(),
                        "summary": check.summary,
                        "fix": check.fix,
                    }))
                    .collect::<Vec<_>>(),
            })
        );
        return if failed == 0 { 0 } else { 1 };
    }

    let styler = Styler::detect();
    println!("{}", styler.cyan(&format!("bob gcal doctor · {host}")));
    let width = checks
        .iter()
        .map(|check| human_label(check.id).len())
        .max()
        .unwrap_or(0);
    for check in &checks {
        println!(
            "{} {:width$} {}",
            ui::glyph(&styler, check.status.label()),
            human_label(check.id),
            check.summary,
            width = width,
        );
        if let Some(fix) = &check.fix
            && matches!(check.status, CheckStatus::Fail | CheckStatus::Warn)
        {
            for line in fix.lines() {
                println!("  {}", styler.dim(&format!("fix: {line}")));
            }
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

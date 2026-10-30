//! `bob gcal doctor`: diagnose the Calendar setup as a checklist.

use std::path::Path;

use serde_json::json;

use super::super::{
    config as bob_config, env as bob_env, pomodoro, pomodoro_homes::HomeKind,
    pomodoro_themes, style::Styler, vault_sync,
};
use super::{
    config::GcalConfig,
    gog::{Gog, CLIENT},
    root::validate_root_candidate,
    ui, DoctorArgs, GcalError,
};

struct Check {
    id: &'static str,
    status: CheckStatus,
    summary: String,
    fix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    #[allow(dead_code)]
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }

    fn label(self) -> &'static str {
        self.as_str()
    }
}

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
                "writer",
            ] {
                checks.push(skipped(id, "no gcal config"));
            }
            return finish(args, &host, checks);
        }
    };

    let mut checks = Vec::new();
    let gog = match Gog::resolve(config.timeout()) {
        Ok(gog) => match gog.require_version() {
            Ok(version) => {
                checks.push(Check {
                    id: "gog",
                    status: CheckStatus::Ok,
                    summary: format!(
                        "v{version} · {}",
                        gog.program().display()
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

    let mut account = config.account().map(str::to_string);
    match &gog {
        Some(gog) => {
            match gog.credentials_list() {
                Ok(clients) if clients.iter().any(|c| c.client == CLIENT) => {
                    checks.push(Check {
                        id: "client",
                        status: CheckStatus::Ok,
                        summary: CLIENT.to_string(),
                        fix: None,
                    });
                }
                Ok(_) => {
                    checks.push(Check {
                        id: "client",
                        status: CheckStatus::Fail,
                        summary: "no bob-gcal OAuth client is stored"
                            .to_string(),
                        fix: Some(
                            "run `bob gcal login -c PATH` with a Desktop-app \
                             client JSON"
                                .to_string(),
                        ),
                    });
                }
                Err(error) => checks.push(fail_from(&error, "client")),
            }

            if account.is_none()
                && let Ok(accounts) = gog.accounts(None)
            {
                let emails: Vec<_> = accounts
                    .iter()
                    .map(|entry| entry.email.clone())
                    .filter(|email| !email.is_empty())
                    .collect();
                if emails.len() == 1 {
                    account = emails.into_iter().next();
                }
            }

            match account.as_deref() {
                Some(email) => match gog.accounts(Some(CLIENT)) {
                    Ok(entries) => {
                        let connected = entries.iter().any(|entry| {
                            entry.email == email
                                && entry.has_calendar_readonly()
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
                                fix: Some("run `bob gcal login`".to_string()),
                            });
                        }
                    }
                    Err(error) => checks.push(fail_from(&error, "account")),
                },
                None => checks.push(Check {
                    id: "account",
                    status: CheckStatus::Fail,
                    summary: "no Google account is configured".to_string(),
                    fix: Some(
                        "set gcal.account or pass `bob gcal login -a EMAIL`"
                            .to_string(),
                    ),
                }),
            }
        }
        None => {
            for id in ["client", "account"] {
                checks.push(skipped(id, "no gog"));
            }
        }
    }

    let calendars = match (gog.as_ref(), account.as_deref()) {
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
                checks.push(fail_from(&error, "token"));
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
    checks.push(writer_check(config.writer(), &host));
    finish(args, &host, checks)
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
            println!("  {}", styler.dim(&format!("fix: {fix}")));
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

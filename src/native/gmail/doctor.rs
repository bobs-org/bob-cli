//! `bob gmail doctor`: diagnose the Gmail setup as a checklist.

use std::path::Path;

use serde_json::json;

use super::super::{config as bob_config, style::Styler};
use super::{
    account,
    client::peek_project,
    config::GmailConfig,
    gog::{Gog, CLIENT},
    journal, list, ui, DoctorArgs, GmailError,
};
use crate::native::{
    google::doctor::{Check, CheckStatus},
    vault_sync,
};

pub(crate) fn run(args: &DoctorArgs) -> i32 {
    let bob_dir = args.bob_dir();
    let host = vault_sync::hostname_slug();
    let config = match GmailConfig::resolve() {
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
                "scopes",
                "config",
                "writer",
                "journal",
                "journal_malformed",
                "gmail_urls",
                "gmail_duplicates",
            ] {
                checks.push(skipped(id, "no gmail config"));
            }
            return finish(args, &host, checks);
        }
    };

    let mut checks = Vec::new();
    let gog = match Gog::resolve(crate::native::google::GMAIL, config.timeout())
    {
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
                            summary: "no bob-gmail OAuth client is stored"
                                .to_string(),
                            fix: Some("run `bob gmail login`".to_string()),
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
                                        &crate::native::google::GMAIL,
                                    )
                            });
                            if connected {
                                checks.push(Check {
                                    id: "account",
                                    status: CheckStatus::Ok,
                                    summary: format!("{email} · Gmail access"),
                                    fix: None,
                                });
                            } else {
                                checks.push(Check {
                                    id: "account",
                                    status: CheckStatus::Fail,
                                    summary: format!(
                                        "{email} · no bob-gmail Gmail grant"
                                    ),
                                    fix: Some(
                                        "run `bob gmail login`".to_string(),
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

    match (gog.as_ref(), selected_account.as_deref()) {
        (Some(gog), Some(email)) => match gog.search(email, "in:inbox", 1) {
            Ok(_) => {
                checks.push(Check {
                    id: "token",
                    status: CheckStatus::Ok,
                    summary: "refreshes".to_string(),
                    fix: None,
                });
            }
            Err(error) => {
                let mut check = fail_from(&error, "token");
                if error.kind() == "invalid_grant" {
                    check.fix = Some(
                        "run `bob gmail login` (it signs in again automatically)"
                            .to_string(),
                    );
                }
                checks.push(check);
            }
        },
        _ => {
            checks.push(skipped("token", "no account"));
        }
    }

    match (gog.as_ref(), selected_account.as_deref()) {
        (Some(gog), Some(email)) => match gog.accounts(Some(CLIENT)) {
            Ok(entries) => {
                let scopes = entries
                    .iter()
                    .find(|entry| entry.email.eq_ignore_ascii_case(email))
                    .map(|entry| entry.scopes.clone())
                    .unwrap_or_default();
                let has_compose = scopes.iter().any(|scope| {
                    scope.contains("gmail.compose")
                        || scope.contains("gmail.send")
                });
                let has_readonly = entries.iter().any(|entry| {
                    entry.email.eq_ignore_ascii_case(email)
                        && entry
                            .has_service_scope(&crate::native::google::GMAIL)
                });
                if has_readonly && has_compose {
                    checks.push(Check {
                        id: "scopes",
                        status: CheckStatus::Ok,
                        summary: "readonly+compose".to_string(),
                        fix: None,
                    });
                } else if has_readonly {
                    checks.push(Check {
                        id: "scopes",
                        status: CheckStatus::Ok,
                        summary: "readonly".to_string(),
                        fix: None,
                    });
                } else {
                    checks.push(Check {
                        id: "scopes",
                        status: CheckStatus::Fail,
                        summary: "no Gmail scopes granted".to_string(),
                        fix: Some("run `bob gmail login`".to_string()),
                    });
                }
            }
            Err(error) => checks.push(fail_from(&error, "scopes")),
        },
        _ => {
            checks.push(skipped("scopes", "no account"));
        }
    }

    checks.push(Check {
        id: "config",
        status: CheckStatus::Ok,
        summary: format!(
            "folder {} · body {} · pull {}",
            config.folder(),
            match config.body() {
                bob_config::GmailBodyMode::Full => "full",
                bob_config::GmailBodyMode::Headers => "headers",
            },
            config.pull_label().unwrap_or("(pull disabled)"),
        ),
        fix: None,
    });

    checks.push(writer_check(config.writer(), &host));
    checks.push(journal_check(&bob_dir));
    checks.push(journal_malformed_check(&bob_dir));
    // Local mail-note checks: they read only vault Markdown, so they run
    // even when gog or sign-in checks fail.
    checks.push(url_health_check(&bob_dir));
    checks.push(mail_duplicate_check(&bob_dir, &config));
    finish(args, &host, checks)
}

fn fail_from(error: &GmailError, id: &'static str) -> Check {
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

fn journal_check(bob_dir: &Path) -> Check {
    let gitignore = read_optional(&bob_dir.join(".gitignore"));
    let attributes = read_optional(&bob_dir.join(".gitattributes"));
    let tracked = gitignore
        .as_deref()
        .is_some_and(|text| text.contains("!/.bob/gmail/*.jsonl"));
    let union = attributes
        .as_deref()
        .is_some_and(|text| text.contains(".bob/gmail/*.jsonl merge=union"));
    match (tracked, union) {
        (true, true) => Check {
            id: "journal",
            status: CheckStatus::Ok,
            summary: ".bob/gmail/*.jsonl tracked and union-merged".to_string(),
            fix: None,
        },
        _ => {
            let mut missing = Vec::new();
            if !tracked {
                missing.push("gitignore un-ignore !/.bob/gmail/*.jsonl");
            }
            if !union {
                missing.push(".gitattributes merge=union");
            }
            Check {
                id: "journal",
                status: CheckStatus::Fail,
                summary: format!("missing {}", missing.join(" and ")),
                fix: Some(
                    "add `!/.bob/gmail/*.jsonl` to .gitignore and \
                     `.bob/gmail/*.jsonl merge=union` to .gitattributes"
                        .to_string(),
                ),
            }
        }
    }
}

/// Decoder health census: every Gmail URL found in vault notes with
/// resolvable/refused/unknown-token counts. Local only: it runs even when
/// gog or sign-in checks fail, and reports counts rather than raw URLs.
fn url_health_check(bob_dir: &Path) -> Check {
    let (total, resolvable, refused, unknown) = list::url_health(bob_dir);
    Check {
        id: "gmail_urls",
        status: CheckStatus::Ok,
        summary: list::url_summary(total, resolvable, refused, unknown),
        fix: None,
    }
}

/// Duplicate `(account, thread)` identities stay visible as diagnostics.
/// Listing skips them rather than selecting one as canonical.
fn mail_duplicate_check(bob_dir: &Path, config: &GmailConfig) -> Check {
    let folder = std::path::Path::new(config.folder());
    let index =
        match super::note::index::scan(bob_dir, folder, config.link_label()) {
            Ok(index) => index,
            Err(_) => return skipped("gmail_duplicates", "no mail index"),
        };
    if index.duplicates.is_empty() {
        return Check {
            id: "gmail_duplicates",
            status: CheckStatus::Ok,
            summary: "no duplicate mail identities".to_string(),
            fix: None,
        };
    }
    let notes: usize = index
        .duplicates
        .iter()
        .map(|(_, _, paths)| paths.len())
        .sum();
    Check {
        id: "gmail_duplicates",
        status: CheckStatus::Warn,
        summary: format!(
            "{} duplicate mail {} in {notes} notes; list skips them",
            index.duplicates.len(),
            if index.duplicates.len() == 1 {
                "identity"
            } else {
                "identities"
            },
        ),
        fix: Some(
            "keep one saved note per account and thread; save refuses ambiguous identities"
                .to_string(),
        ),
    }
}

fn journal_malformed_check(bob_dir: &Path) -> Check {
    match journal::malformed_count(bob_dir) {
        0 => Check {
            id: "journal_malformed",
            status: CheckStatus::Ok,
            summary: "no malformed gmail journal lines".to_string(),
            fix: None,
        },
        count => Check {
            id: "journal_malformed",
            status: CheckStatus::Warn,
            summary: format!(
                "{count} malformed gmail journal line{} skipped",
                if count == 1 { "" } else { "s" }
            ),
            fix: Some(
                "malformed lines are skipped safely; inspect \
                 .bob/gmail/<host>.jsonl"
                    .to_string(),
            ),
        },
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
                "run gmail writes on {writer}, or set gmail.writer: {host}"
            )),
        },
    }
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn human_label(id: &str) -> &str {
    id
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
    println!("{}", styler.cyan(&format!("bob gmail doctor · {host}")));
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

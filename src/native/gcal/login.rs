//! `bob gcal login`: guided client setup and read-only Calendar consent.

use serde_json::json;

use super::{
    account,
    client::ClientOrigin,
    client_setup::{ensure_client, ResolvedClient},
    config::GcalConfig,
    gog::{Gog, GogCalendar, API_CONSOLE_URL, CLIENT},
    login_ui::LoginUi,
    signin, ui, GcalError, LoginArgs,
};
use crate::native::vault_sync;

#[cfg(test)]
use super::client_setup::{parse_confirm_default_yes, Confirm};

struct LoginReport {
    already_connected: bool,
    account: String,
    client: String,
    client_source: &'static str,
    client_project: Option<String>,
    calendars: usize,
    syncing: usize,
    syncing_ids: Vec<String>,
    sign_in: &'static str,
    reconnected: bool,
}

pub(crate) fn run(args: &LoginArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GcalConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("login", &error, format),
    };
    let ui_login =
        LoginUi::new(crate::native::google::GCAL, args.format.is_json());
    match login(args, &config, &ui_login) {
        Ok(report) => {
            print_report(&report, &ui_login);
            0
        }
        Err(error) => ui::report_error("login", &error, format),
    }
}

fn login(
    args: &LoginArgs,
    config: &GcalConfig,
    ui: &LoginUi,
) -> Result<LoginReport, GcalError> {
    let gog = Gog::resolve(crate::native::google::GCAL, config.timeout())?;
    let _version = gog.require_version()?;
    ui.title();

    let resolved = ensure_client(&gog, args.credentials.as_deref(), ui)?;
    render_client_step(ui, &resolved);
    let account = account::resolve_login(
        &gog,
        args.account.as_deref(),
        config.account(),
        ui,
    )?;

    let listed = gog.accounts(Some(CLIENT))?;
    let has_readonly_grant = listed.iter().any(|entry| {
        entry.email.eq_ignore_ascii_case(&account)
            && entry.has_service_scope(&crate::native::google::GCAL)
    });

    let mut already_connected = false;
    let mut reconnected = false;
    let mut testing_tip = false;
    let mut sign_in = "already_connected";
    let mut first_check_error = None;

    if has_readonly_grant {
        match gog.calendars(&account) {
            Ok(calendars) => {
                already_connected = true;
                render_signin_already(ui, &account);
                let report = build_report(
                    &account,
                    &resolved,
                    sign_in,
                    already_connected,
                    reconnected,
                    &calendars,
                    config,
                );
                render_check_step(ui, &report, &calendars, config);
                return finish_login(
                    report,
                    ui,
                    config,
                    &gog,
                    reconnected,
                    testing_tip,
                );
            }
            Err(error) if is_reconnect_error(&error) => {
                testing_tip = error.kind() == "invalid_grant";
                reconnected = true;
                ui.step_warn(
                    2,
                    "Sign in",
                    "the saved sign-in stopped working; signing in again…",
                );
            }
            Err(error) if error.kind() == "api_disabled" => {
                already_connected = true;
                render_signin_already(ui, &account);
                first_check_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    if !already_connected {
        let (flow, reason) = signin::choose_flow(
            args.manual,
            signin::FlowEnv::detect(crate::native::google::GCAL),
        );
        let flow = signin::run(
            &gog,
            ui,
            &account,
            flow,
            reason,
            resolved.project.as_deref(),
        )?;
        sign_in = flow.as_str();
    }

    let calendars = check_calendars(
        &gog,
        ui,
        &account,
        resolved.project.as_deref(),
        first_check_error,
    )?;
    let report = build_report(
        &account,
        &resolved,
        sign_in,
        already_connected,
        reconnected,
        &calendars,
        config,
    );
    render_check_step(ui, &report, &calendars, config);
    finish_login(report, ui, config, &gog, reconnected, testing_tip)
}

fn finish_login(
    report: LoginReport,
    ui: &LoginUi,
    config: &GcalConfig,
    gog: &Gog,
    reconnected: bool,
    testing_tip: bool,
) -> Result<LoginReport, GcalError> {
    ui.summary(&report.account);
    let host = vault_sync::hostname_slug();
    if let Some(writer) = config.writer().filter(|writer| *writer != host) {
        ui.writer_note(writer);
    }
    if account::would_be_ambiguous(gog, config.account()) {
        ui.account_tip(&report.account);
    }
    if reconnected && testing_tip {
        ui.testing_tip();
    }
    Ok(report)
}

fn render_signin_already(ui: &LoginUi, account: &str) {
    ui.step_ok(2, "Sign in", &format!("already connected · {account}"));
}

fn build_report(
    account: &str,
    resolved: &ResolvedClient,
    sign_in: &'static str,
    already_connected: bool,
    reconnected: bool,
    calendars: &[GogCalendar],
    config: &GcalConfig,
) -> LoginReport {
    let syncing_ids: Vec<String> = calendars
        .iter()
        .filter(|calendar| config.syncs(&calendar.id, calendar.primary))
        .map(|calendar| {
            if calendar.primary {
                "primary".to_string()
            } else {
                calendar.id.clone()
            }
        })
        .collect();
    LoginReport {
        already_connected,
        account: account.to_string(),
        client: CLIENT.to_string(),
        client_source: resolved.origin.as_str(),
        client_project: resolved.project.clone(),
        calendars: calendars.len(),
        syncing: syncing_ids.len(),
        syncing_ids,
        sign_in,
        reconnected,
    }
}

fn render_client_step(ui: &LoginUi, resolved: &ResolvedClient) {
    let project = resolved
        .project
        .as_deref()
        .map(|project| format!(" · project {project}"))
        .unwrap_or_default();
    let detail = match resolved.origin {
        ClientOrigin::GogClient => {
            let name = resolved.reused_name.as_deref().unwrap_or("default");
            format!("reusing gog's \"{name}\" client{project}")
        }
        ClientOrigin::File | ClientOrigin::Downloads | ClientOrigin::Pasted => {
            let file =
                resolved.file_display.as_deref().unwrap_or("client.json");
            format!("{file} · Desktop app{project}")
        }
        ClientOrigin::Stored => format!("bob-gcal{project}"),
    };
    ui.step_ok(1, "Google client", &detail);
    if resolved.origin == ClientOrigin::GogClient {
        ui.note("Bob keeps its own read-only sign-in; Gmail stays untouched.");
    }
}

fn render_check_step(
    ui: &LoginUi,
    report: &LoginReport,
    calendars: &[GogCalendar],
    config: &GcalConfig,
) {
    let syncing = if report.syncing_ids.is_empty() {
        "none".to_string()
    } else {
        report.syncing_ids.join(", ")
    };
    ui.step_ok(
        3,
        "Check",
        &format!(
            "{} calendars · syncing {}: {syncing}",
            report.calendars, report.syncing
        ),
    );
    let missing: Vec<_> = config
        .calendars()
        .iter()
        .filter(|configured| {
            !calendars
                .iter()
                .any(|calendar| calendar.matches_configured_id(&configured.id))
        })
        .map(|configured| configured.id.as_str())
        .collect();
    if !missing.is_empty() {
        ui.step_warn(
            3,
            "Check",
            &format!("configured calendars not found: {}", missing.join(", ")),
        );
        ui.note("run `bob gcal calendars` to choose available calendars");
    }
}

fn check_calendars(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    project: Option<&str>,
    mut first_error: Option<GcalError>,
) -> Result<Vec<GogCalendar>, GcalError> {
    loop {
        let result = match first_error.take() {
            Some(error) => Err(error),
            None => gog.calendars(account),
        };
        match result {
            Ok(calendars) => return Ok(calendars),
            Err(error) if error.kind() == "api_disabled" => {
                let url = api_disabled_url(&error, project);
                ui.step_fail(
                    3,
                    "Check",
                    "the Calendar API is turned off for this project",
                );
                ui.note_interactive("Turn it on, then come back:");
                ui.link(&url);
                if !ui.is_tty() {
                    return Err(error.with_hint(&api_kept_hint(&url)));
                }
                match ui
                    .prompt("Press Enter to check again, or q to stop › ")?
                {
                    None => return Err(api_retry_cancelled(&url)),
                    Some(answer)
                        if answer.trim().eq_ignore_ascii_case("q")
                            || answer.trim().eq_ignore_ascii_case("quit") =>
                    {
                        return Err(api_retry_cancelled(&url));
                    }
                    Some(_) => {}
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn api_retry_cancelled(url: &str) -> GcalError {
    GcalError::runtime(
        "api_disabled",
        "the Google Calendar API is not enabled for this Cloud project"
            .to_string(),
    )
    .with_hint(&api_kept_hint(url))
}

fn api_kept_hint(url: &str) -> String {
    format!(
        "your sign-in is kept; enable the Calendar API at {url}, then rerun `bob gcal login`"
    )
}

fn api_disabled_url(error: &GcalError, project: Option<&str>) -> String {
    if let Some(hint) = error.hint()
        && let Some(start) = hint.find("https://")
    {
        let candidate = hint[start..]
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(['.', ',', ')']);
        if let Ok(mut url) = url::Url::parse(candidate) {
            if url
                .query_pairs()
                .any(|(key, value)| key == "project" && !value.is_empty())
            {
                return url.into();
            }
            if let Some(project) = project {
                url.query_pairs_mut().append_pair("project", project);
                return url.into();
            }
            return url.into();
        }
    }
    if let Some(project) = project
        && let Ok(mut url) = url::Url::parse(API_CONSOLE_URL)
    {
        url.query_pairs_mut().append_pair("project", project);
        return url.into();
    }
    API_CONSOLE_URL.to_string()
}

fn is_reconnect_error(error: &GcalError) -> bool {
    matches!(
        error.kind(),
        "invalid_grant" | "insufficient_scope" | "not_logged_in"
    )
}

fn print_report(report: &LoginReport, ui: &LoginUi) {
    if ui.json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "already_connected": report.already_connected,
                "account": report.account,
                "client": report.client,
                "calendars": report.calendars,
                "syncing": report.syncing,
                "syncing_ids": report.syncing_ids,
                "sign_in": report.sign_in,
                "client_source": report.client_source,
                "client_project": report.client_project,
                "reconnected": report.reconnected,
            })
        );
        ui::flush_stdout();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_names_match_json() {
        assert_eq!(signin::Flow::Browser.as_str(), "browser");
        assert_eq!(signin::Flow::Paste.as_str(), "paste");
    }

    #[test]
    fn downloads_confirm_treats_q_as_cancel_and_junk_as_unknown() {
        assert_eq!(parse_confirm_default_yes(""), Confirm::Yes);
        assert_eq!(parse_confirm_default_yes("Y"), Confirm::Yes);
        assert_eq!(parse_confirm_default_yes("yes"), Confirm::Yes);
        assert_eq!(parse_confirm_default_yes("n"), Confirm::No);
        assert_eq!(parse_confirm_default_yes("No"), Confirm::No);
        assert_eq!(parse_confirm_default_yes("q"), Confirm::Quit);
        assert_eq!(parse_confirm_default_yes("quit"), Confirm::Quit);
        assert_eq!(parse_confirm_default_yes("maybe"), Confirm::Unknown);
    }

    #[test]
    fn api_url_prefers_message_then_client_project() {
        let with_google_project = GcalError::runtime("api_disabled", "off".to_string())
            .with_hint("enable it at https://console.developers.google.com/apis/api/calendar-json.googleapis.com/overview?project=123");
        assert!(api_disabled_url(&with_google_project, Some("client-proj"))
            .contains("project=123"));
        let generic = GcalError::runtime("api_disabled", "off".to_string())
            .with_hint(&format!("enable it at {API_CONSOLE_URL}"));
        assert!(api_disabled_url(&generic, Some("client-proj"))
            .contains("project=client-proj"));
    }
}

//! `bob gmail login`: guided client setup and Gmail consent.

use serde_json::json;

use super::{
    account,
    client::ClientOrigin,
    client_setup::{ensure_client, ResolvedClient},
    config::{GmailConfig, COMPOSE_SCOPE},
    gog::{Gog, API_CONSOLE_URL, CLIENT},
    login_ui::LoginUi,
    signin, ui, GmailError, LoginArgs,
};
use crate::native::vault_sync;

struct LoginReport {
    already_connected: bool,
    account: String,
    client: String,
    client_source: &'static str,
    client_project: Option<String>,
    scope: String,
    granted_scopes: Vec<String>,
    sign_in: &'static str,
    reconnected: bool,
}

pub(crate) fn run(args: &LoginArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GmailConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("login", &error, format),
    };
    let ui_login =
        LoginUi::new(crate::native::google::GMAIL, args.format.is_json());
    match login(args, &config, &ui_login) {
        Ok(report) => {
            super::worker::remember_login(&report.account);
            print_report(&report, &ui_login);
            0
        }
        Err(error) => ui::report_error("login", &error, format),
    }
}

fn login(
    args: &LoginArgs,
    config: &GmailConfig,
    ui: &LoginUi,
) -> Result<LoginReport, GmailError> {
    let wants_compose = args.scope == super::cli::LoginScope::Compose;
    let mut gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    if wants_compose {
        gog = gog.with_extra_scopes(vec![COMPOSE_SCOPE.to_string()]);
    }
    let _version = gog.require_version()?;
    ui.title_with_access(if wants_compose {
        "compose"
    } else {
        "read-only"
    });

    let resolved = ensure_client(&gog, args.credentials.as_deref(), ui)?;
    render_client_step(ui, &resolved);
    let account = account::resolve_login(
        &gog,
        args.account.as_deref(),
        config.account(),
        ui,
    )?;

    let listed = gog.accounts(Some(CLIENT))?;
    let has_grant = listed.iter().any(|entry| {
        entry.email.eq_ignore_ascii_case(&account)
            && entry.has_service_scope(&crate::native::google::GMAIL)
    });

    let mut already_connected = false;
    let mut reconnected = false;
    let mut testing_tip = false;
    let mut sign_in = "already_connected";
    let mut first_check_error = None;

    if has_grant {
        match gog.search(&account, "in:inbox", 1) {
            // A read-only grant cannot satisfy `-s compose`: Google adds a
            // scope only through a fresh consent, so sign in again below.
            Ok(_)
                if wants_compose
                    && !has_compose(&granted_scopes(&gog, &account)) =>
            {
                ui.step_warn(
                    2,
                    "Sign in",
                    "the saved sign-in is read-only; signing in again to add compose…",
                );
            }
            Ok(_) => {
                already_connected = true;
                render_signin_already(ui, &account);
                let granted = granted_scopes(&gog, &account);
                let report = build_report(
                    &account,
                    &resolved,
                    args,
                    sign_in,
                    already_connected,
                    reconnected,
                    granted,
                );
                render_check_step(ui, &report);
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
            signin::FlowEnv::detect(crate::native::google::GMAIL),
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

    check_gmail(
        &gog,
        ui,
        &account,
        resolved.project.as_deref(),
        first_check_error,
    )?;
    let granted = granted_scopes(&gog, &account);
    if wants_compose && !has_compose(&granted) {
        ui.step_fail(
            3,
            "Check",
            "Google granted read-only access, not compose",
        );
        return Err(compose_not_granted());
    }
    let report = build_report(
        &account,
        &resolved,
        args,
        sign_in,
        already_connected,
        reconnected,
        granted,
    );
    render_check_step(ui, &report);
    finish_login(report, ui, config, &gog, reconnected, testing_tip)
}

fn has_compose(granted: &[String]) -> bool {
    granted.iter().any(|scope| scope == "compose")
}

/// Google's consent screen lets each scope be unticked; an unticked compose
/// box still yields a working read-only grant, so say so instead of
/// reporting success.
fn compose_not_granted() -> GmailError {
    GmailError::runtime(
        "compose_not_granted",
        "Google granted read-only Gmail access but not the compose scope"
            .to_string(),
    )
    .with_hint(
        "rerun `bob gmail login -s compose` and tick \"Manage drafts and \
         send emails\" on Google's consent screen; the read-only grant is kept",
    )
}

fn granted_scopes(gog: &Gog, account: &str) -> Vec<String> {
    let Ok(entries) = gog.accounts(Some(CLIENT)) else {
        return Vec::new();
    };
    let Some(entry) = entries
        .iter()
        .find(|entry| entry.email.eq_ignore_ascii_case(account))
    else {
        return Vec::new();
    };
    let mut scopes = Vec::new();
    if entry.has_service_scope(&crate::native::google::GMAIL) {
        scopes.push("readonly".to_string());
    }
    if entry
        .scopes
        .iter()
        .any(|scope| scope.contains("gmail.compose"))
        || entry
            .scopes
            .iter()
            .any(|scope| scope.contains("gmail.send"))
    {
        scopes.push("compose".to_string());
    }
    scopes
}

fn finish_login(
    report: LoginReport,
    ui: &LoginUi,
    config: &GmailConfig,
    gog: &Gog,
    reconnected: bool,
    testing_tip: bool,
) -> Result<LoginReport, GmailError> {
    let compose = has_compose(&report.granted_scopes);
    if compose {
        ui.summary_compose(&report.account);
    } else {
        ui.summary(&report.account);
    }
    if compose {
        ui.note(
            "Compose scope granted: gmail.compose also permits sending; \
             every send still goes through a previewed draft.",
        );
    }
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
    args: &LoginArgs,
    sign_in: &'static str,
    already_connected: bool,
    reconnected: bool,
    granted_scopes: Vec<String>,
) -> LoginReport {
    LoginReport {
        already_connected,
        account: account.to_string(),
        client: CLIENT.to_string(),
        client_source: resolved.origin.as_str(),
        client_project: resolved.project.clone(),
        scope: args.scope.as_str().to_string(),
        granted_scopes,
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
        ClientOrigin::Stored => format!("bob-gmail{project}"),
    };
    ui.step_ok(1, "Google client", &detail);
    if resolved.origin == ClientOrigin::GogClient {
        ui.note("Bob keeps its own Gmail sign-in; Calendar stays untouched.");
    }
}

fn render_check_step(ui: &LoginUi, report: &LoginReport) {
    let scopes = if report.granted_scopes.is_empty() {
        "read-only".to_string()
    } else {
        report.granted_scopes.join("+")
    };
    ui.step_ok(3, "Check", &format!("Gmail search ok · scopes: {scopes}"));
}

fn check_gmail(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    project: Option<&str>,
    mut first_error: Option<GmailError>,
) -> Result<(), GmailError> {
    loop {
        let result = match first_error.take() {
            Some(error) => Err(error),
            None => gog.search(account, "in:inbox", 1).map(|_| ()),
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == "api_disabled" => {
                let url = api_disabled_url(&error, project);
                ui.step_fail(
                    3,
                    "Check",
                    "the Gmail API is turned off for this project",
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

fn api_retry_cancelled(url: &str) -> GmailError {
    GmailError::runtime(
        "api_disabled",
        "the Google Gmail API is not enabled for this Cloud project"
            .to_string(),
    )
    .with_hint(&api_kept_hint(url))
}

fn api_kept_hint(url: &str) -> String {
    format!(
        "your sign-in is kept; enable the Gmail API at {url}, then rerun `bob gmail login`"
    )
}

fn api_disabled_url(error: &GmailError, project: Option<&str>) -> String {
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

fn is_reconnect_error(error: &GmailError) -> bool {
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
                "scope": report.scope,
                "granted_scopes": report.granted_scopes,
                "sign_in": report.sign_in,
                "client_source": report.client_source,
                "client_project": report.client_project,
                "reconnected": report.reconnected,
            })
        );
        ui::flush_stdout();
    }
}

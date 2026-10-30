//! Bob-owned Google sign-in: flow choice, paste prompt, loopback
//! browser listener, HTML outcome pages, and the token exchange.

use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use url::Url;

use crate::native::env as bob_env;

use super::{
    config::stdin_is_tty,
    gog::{self, Gog, CLIENT},
    login_ui::{self, LoginUi},
    GcalError,
};

const CONSENT_WINDOW: Duration = Duration::from_secs(600);
const CALLBACK_PATH: &str = "/oauth2/callback";
const HEADER_LIMIT: usize = 8 * 1024;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// How Bob drives Google's consent screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Browser,
    Paste,
}

impl Flow {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Paste => "paste",
        }
    }
}

/// Injected environment for [`choose_flow`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlowEnv {
    pub browser_override: bool,
    pub ssh: bool,
    pub has_display: bool,
    pub is_macos: bool,
}

impl FlowEnv {
    pub(crate) fn detect() -> Self {
        Self {
            browser_override: non_empty("BOB_GCAL_BROWSER"),
            ssh: non_empty("SSH_CONNECTION") || non_empty("SSH_TTY"),
            has_display: non_empty("DISPLAY") || non_empty("WAYLAND_DISPLAY"),
            is_macos: cfg!(target_os = "macos"),
        }
    }
}

fn non_empty(key: &str) -> bool {
    bob_env::var_os(key).is_some_and(|value| !value.is_empty())
}

/// Pick browser vs paste. Pure: the env is injected.
pub(crate) fn choose_flow(manual: bool, env: FlowEnv) -> (Flow, &'static str) {
    if manual {
        return (Flow::Paste, "you asked to paste");
    }
    if env.browser_override {
        return (Flow::Browser, "opening your browser");
    }
    if env.ssh {
        return (Flow::Paste, "you're on SSH");
    }
    if !env.is_macos && !env.has_display {
        return (Flow::Paste, "no desktop session");
    }
    (Flow::Browser, "opening your browser")
}

/// Outcome of parsing a pasted redirect address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RedirectParse {
    Help,
    Quit,
    Empty,
    Denied,
    Url(String),
    Error(String),
}

/// Parse a pasted redirect. `expected_state` is the live step-1 state.
pub(crate) fn parse_redirect(
    input: &str,
    expected_state: &str,
) -> RedirectParse {
    let trimmed = strip_wrapping(input.trim());
    if trimmed.is_empty() {
        return RedirectParse::Empty;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "?" || lower == "help" {
        return RedirectParse::Help;
    }
    if lower == "q" || lower == "quit" {
        return RedirectParse::Quit;
    }

    let candidate = if looks_bare_loopback(trimmed) {
        format!("http://{trimmed}")
    } else {
        trimmed.to_string()
    };
    let Ok(url) = Url::parse(&candidate) else {
        return RedirectParse::Error(not_the_address());
    };
    if !host_is_loopback(&url) {
        return RedirectParse::Error(not_the_address());
    }
    let mut error = None;
    let mut code = None;
    let mut state = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "error" if error.is_none() => error = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            "state" if state.is_none() => state = Some(value.into_owned()),
            _ => {}
        }
    }
    if error.is_some() {
        return RedirectParse::Denied;
    }
    if code.as_deref().is_none_or(str::is_empty) {
        return RedirectParse::Error(not_the_address());
    }
    match state.as_deref() {
        Some(got) if got == expected_state => RedirectParse::Url(candidate),
        _ => RedirectParse::Error(
            "That address belongs to an older sign-in link. Use the link \
             above, click Allow, and copy the new page's address."
                .to_string(),
        ),
    }
}

fn strip_wrapping(input: &str) -> &str {
    let trimmed = input.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"')
        || trimmed.starts_with('\'') && trimmed.ends_with('\'')
        || trimmed.starts_with('<') && trimmed.ends_with('>'))
        && trimmed.len() >= 2
    {
        return trimmed[1..trimmed.len() - 1].trim();
    }
    trimmed
}

fn looks_bare_loopback(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.starts_with("127.0.0.1")
        || lower.starts_with("localhost")
        || lower.starts_with("[::1]")
        || lower.starts_with("::1")
}

fn host_is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some("127.0.0.1") | Some("localhost") | Some("::1") => true,
        Some(host) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn not_the_address() -> String {
    "That isn't the address Bob needs. After you click Allow, the \
     browser goes to a page starting with http://127.0.0.1: — copy that \
     page's address."
        .to_string()
}

/// Append `login_hint=<email>` and assert `state` is unchanged.
pub(crate) fn inject_login_hint(
    auth_url: &str,
    email: &str,
) -> Result<String, GcalError> {
    let mut url = Url::parse(auth_url).map_err(|error| {
        GcalError::runtime(
            "bad_json",
            format!("gog auth_url is not a URL: {error}"),
        )
    })?;
    let before = gog::state_from_auth_url(auth_url);
    url.query_pairs_mut().append_pair("login_hint", email);
    let after = gog::state_from_auth_url(url.as_str());
    if before != after {
        return Err(GcalError::runtime(
            "bad_json",
            "adding login_hint changed the OAuth state".to_string(),
        ));
    }
    Ok(url.into())
}

/// HTML-escape dynamic page text.
pub(crate) fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Outcome page: success (connected) or failure (didn't finish).
pub(crate) fn page(success: bool, email: &str, reason: &str) -> String {
    let (mark, headline, detail) = if success {
        (
            "✓",
            "Bob can read your Google Calendar",
            format!(
                "read-only access for {} · you can close this tab",
                html_escape(email)
            ),
        )
    } else {
        ("✗", "Sign-in didn't finish", html_escape(reason))
    };
    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>Bob</title>\n\
<style>\n\
:root {{ color-scheme: light dark; }}\n\
body {{\n\
  font-family: system-ui, sans-serif;\n\
  margin: 0;\n\
  min-height: 100vh;\n\
  display: flex;\n\
  align-items: center;\n\
  justify-content: center;\n\
  background: #f6f5f2;\n\
  color: #1b1b18;\n\
}}\n\
@media (prefers-color-scheme: dark) {{\n\
  body {{ background: #161615; color: #eceae3; }}\n\
}}\n\
main {{ text-align: center; padding: 2rem; max-width: 32rem; }}\n\
.mark {{ font-size: 4rem; line-height: 1; }}\n\
h1 {{ font-size: 1.35rem; font-weight: 600; }}\n\
p {{ opacity: 0.8; }}\n\
</style>\n\
</head>\n\
<body>\n\
<main>\n\
<div class=\"mark\">{mark}</div>\n\
<h1>{headline}</h1>\n\
<p>{detail}</p>\n\
</main>\n\
</body>\n\
</html>\n"
    )
}

/// Run the chosen sign-in flow and confirm the grant.
pub(crate) fn run(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    flow: Flow,
    reason: &str,
    project: Option<&str>,
) -> Result<Flow, GcalError> {
    match flow {
        Flow::Paste => paste_flow(gog, ui, account, reason, project),
        Flow::Browser => browser_flow(gog, ui, account, project),
    }?;
    Ok(flow)
}

fn paste_flow(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    reason: &str,
    project: Option<&str>,
) -> Result<(), GcalError> {
    if !stdin_is_tty() {
        return Err(needs_tty());
    }
    let mut auto_retries = 0;
    loop {
        let start = gog.auth_begin(account, None)?;
        let auth_url = inject_login_hint(&start.auth_url, account)?;
        ui.step_open(
            2,
            "Sign in",
            &format!("{account} · {reason}, so sign in from any browser:"),
        );
        ui.link(&auth_url);
        print_paste_instructions(ui, account);
        match paste_prompt_loop(gog, ui, account, &start.state, project)? {
            PasteOutcome::SignedIn => {
                ui.note("✓ signed in · read-only Calendar access granted");
                return Ok(());
            }
            PasteOutcome::RetryFresh => continue,
            PasteOutcome::RetryAuto => {
                if auto_retries >= 1 {
                    return Err(GcalError::runtime(
                        "stale_link",
                        "that sign-in link is out of date".to_string(),
                    )
                    .with_hint("rerun `bob gcal login` for a fresh link"));
                }
                auto_retries += 1;
                ui.note(
                    "that sign-in link is out of date; starting a fresh one",
                );
            }
        }
    }
}

enum PasteOutcome {
    SignedIn,
    RetryFresh,
    RetryAuto,
}

fn paste_prompt_loop(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    expected_state: &str,
    project: Option<&str>,
) -> Result<PasteOutcome, GcalError> {
    loop {
        let Some(line) = ui.prompt("Paste the address (? for help) › ")?
        else {
            return Err(cancelled());
        };
        match parse_redirect(&line, expected_state) {
            RedirectParse::Help => print_paste_help(ui, project),
            RedirectParse::Quit => return Err(cancelled()),
            RedirectParse::Empty => {}
            RedirectParse::Denied => {
                ui.note("Google denied access.");
                return retry_or_quit(ui, cancelled_or_denied(true));
            }
            RedirectParse::Error(message) => {
                ui.note(&message);
                if message.contains("older sign-in") {
                    // reprint happens by leaving the current link above
                }
            }
            RedirectParse::Url(redirect) => {
                match finish_and_confirm(gog, account, &redirect) {
                    Ok(()) => return Ok(PasteOutcome::SignedIn),
                    Err(error) if is_auto_retry(&error) => {
                        return Ok(PasteOutcome::RetryAuto);
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

fn retry_or_quit(
    ui: &LoginUi,
    on_quit: GcalError,
) -> Result<PasteOutcome, GcalError> {
    match ui.prompt("Press Enter to try again, or q to stop › ")? {
        None => Err(cancelled()),
        Some(line) => {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("q")
                || trimmed.eq_ignore_ascii_case("quit")
            {
                Err(on_quit)
            } else {
                Ok(PasteOutcome::RetryFresh)
            }
        }
    }
}

fn cancelled_or_denied(_from_denied: bool) -> GcalError {
    // The phase tests pin `access_denied` then `q` as exit 2 `cancelled`.
    cancelled()
}

fn print_paste_instructions(ui: &LoginUi, account: &str) {
    ui.note(&format!("1. Open the link, choose {account}, and allow"));
    ui.note("   \"See and download any calendar you can access\".");
    ui.note("2. \"Google hasn't verified this app\"? It's your own client:");
    ui.note("   Advanced → Go to … (unsafe).");
    ui.note(
        "3. You land on a 127.0.0.1 page that can't load. That's expected:",
    );
    ui.note("   copy that page's whole address.");
}

fn print_paste_help(ui: &LoginUi, project: Option<&str>) {
    let audience = match project {
        Some(project) => format!(
            "https://console.cloud.google.com/auth/audience?project={project}"
        ),
        None => "https://console.cloud.google.com/auth/audience".to_string(),
    };
    ui.note(
        "\"Access blocked: … has not completed the Google verification \
         process\":",
    );
    ui.note("  the consent screen is in Testing. Publish it, or add yourself");
    ui.note(&format!("  as a test user, on {audience}"));
    ui.note(
        "\"Google hasn't verified this app\": expected for a personal client.",
    );
    ui.note("  Use Advanced → Go to … (unsafe).");
    ui.note(
        "\"Error 400: redirect_uri_mismatch\": the client isn't a Desktop app.",
    );
    ui.note("  Rerun with `bob gcal login -c` and a Desktop app JSON.");
    ui.note("\"This site can't be reached\" after Allow: expected. Copy that");
    ui.note("  page's address.");
}

fn browser_flow(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    _project: Option<&str>,
) -> Result<(), GcalError> {
    let mut auto_retries = 0;
    loop {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
            GcalError::runtime(
                "gog",
                format!("bind a loopback listener: {error}"),
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                GcalError::runtime(
                    "gog",
                    format!("read the loopback port: {error}"),
                )
            })?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let start = gog.auth_begin(account, Some(&redirect_uri))?;
        let auth_url = inject_login_hint(&start.auth_url, account)?;
        ui.step_open(
            2,
            "Sign in",
            &format!("{account} · opening your browser…"),
        );
        ui.note("Allow \"See and download any calendar you can access\".");
        ui.note("Browser didn't open? Use this link:");
        ui.link(&auth_url);
        let opener = spawn_opener(&auth_url);
        let wait = ui.wait("waiting for Google · Ctrl-C to cancel");
        let result = wait_for_callback(
            gog,
            ui,
            account,
            &listener,
            &start.state,
            opener,
        );
        drop(wait);
        match result {
            Ok(()) => {
                ui.note("✓ signed in · read-only Calendar access granted");
                return Ok(());
            }
            Err(error) if is_auto_retry(&error) && auto_retries < 1 => {
                auto_retries += 1;
                ui.note(
                    "that sign-in link is out of date; starting a fresh one",
                );
            }
            Err(error) => return Err(error),
        }
    }
}

fn wait_for_callback(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    listener: &TcpListener,
    expected_state: &str,
    mut opener: Option<Child>,
) -> Result<(), GcalError> {
    listener.set_nonblocking(true).map_err(|error| {
        GcalError::runtime(
            "gog",
            format!("set the listener non-blocking: {error}"),
        )
    })?;
    let deadline = Instant::now() + consent_window();
    let mut opener_failed_noted = false;
    loop {
        if Instant::now() >= deadline {
            return Err(GcalError::runtime(
                "browser_timeout",
                "no callback within the 10-minute consent window".to_string(),
            )
            .with_hint(
                "run `bob gcal login -m` to paste the address instead",
            ));
        }
        if let Some(child) = opener.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            opener = None;
            if !status.success() && !opener_failed_noted {
                ui.dim_note("couldn't open a browser; use the link above");
                opener_failed_noted = true;
            }
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                match handle_connection(
                    gog,
                    ui,
                    account,
                    &mut stream,
                    expected_state,
                )? {
                    ConnResult::Continue => {}
                    ConnResult::SignedIn => return Ok(()),
                    ConnResult::Denied => {
                        return browser_denied(ui);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                return Err(GcalError::runtime(
                    "gog",
                    format!("accept the OAuth callback: {error}"),
                ));
            }
        }
    }
}

enum ConnResult {
    Continue,
    SignedIn,
    Denied,
}

fn handle_connection(
    gog: &Gog,
    _ui: &LoginUi,
    account: &str,
    stream: &mut TcpStream,
    expected_state: &str,
) -> Result<ConnResult, GcalError> {
    let headers = match read_headers(stream) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(ConnResult::Continue),
    };
    let Some((method, target)) = parse_request_line(&headers) else {
        let _ = write_http(
            stream,
            400,
            "Bad Request",
            &page(false, account, "this sign-in link is out of date"),
        );
        return Ok(ConnResult::Continue);
    };
    if method != "GET" {
        let _ = write_http(
            stream,
            404,
            "Not Found",
            &page(false, account, "this is not the sign-in page"),
        );
        return Ok(ConnResult::Continue);
    }
    let parsed = Url::parse(&format!("http://127.0.0.1{target}")).ok();
    let Some(url) = parsed else {
        let _ = write_http(
            stream,
            404,
            "Not Found",
            &page(false, account, "this is not the sign-in page"),
        );
        return Ok(ConnResult::Continue);
    };
    if url.path() != CALLBACK_PATH {
        let _ = write_http(
            stream,
            404,
            "Not Found",
            &page(false, account, "this is not the sign-in page"),
        );
        return Ok(ConnResult::Continue);
    }
    let mut error = None;
    let mut code = None;
    let mut state = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "error" if error.is_none() => error = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            "state" if state.is_none() => state = Some(value.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        let _ = write_http(
            stream,
            400,
            "Bad Request",
            &page(false, account, "this sign-in link is out of date"),
        );
        return Ok(ConnResult::Continue);
    }
    if let Some(kind) = error {
        let reason = if kind == "access_denied" {
            "Google denied access"
        } else {
            kind.as_str()
        };
        let _ = write_http(stream, 200, "OK", &page(false, account, reason));
        return Ok(ConnResult::Denied);
    }
    let Some(code) = code.filter(|c| !c.is_empty()) else {
        let _ = write_http(
            stream,
            400,
            "Bad Request",
            &page(false, account, "this sign-in link is out of date"),
        );
        return Ok(ConnResult::Continue);
    };
    let redirect = format!(
        "http://127.0.0.1{CALLBACK_PATH}?state={}&code={}",
        expected_state, code
    );
    match finish_and_confirm(gog, account, &redirect) {
        Ok(()) => {
            let _ = write_http(stream, 200, "OK", &page(true, account, ""));
            Ok(ConnResult::SignedIn)
        }
        Err(error) if is_auto_retry(&error) => {
            let _ = write_http(
                stream,
                200,
                "OK",
                &page(false, account, error.message()),
            );
            Err(error)
        }
        Err(error) => {
            let _ = write_http(
                stream,
                200,
                "OK",
                &page(false, account, error.message()),
            );
            Err(error)
        }
    }
}

fn browser_denied(ui: &LoginUi) -> Result<(), GcalError> {
    if !login_ui::is_tty() {
        return Err(consent_denied());
    }
    ui.note("Google denied access.");
    match ui.prompt("Press Enter to try again, or q to stop › ")? {
        None => Err(cancelled()),
        Some(line) => {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("q")
                || trimmed.eq_ignore_ascii_case("quit")
            {
                Err(consent_denied())
            } else {
                Err(GcalError::runtime(
                    "stale_link",
                    "retrying sign-in".to_string(),
                ))
            }
        }
    }
}

fn spawn_opener(url: &str) -> Option<Child> {
    let program: PathBuf = if let Some(override_cmd) =
        bob_env::var_os("BOB_GCAL_BROWSER").filter(|value| !value.is_empty())
    {
        PathBuf::from(override_cmd)
    } else if cfg!(target_os = "macos") {
        PathBuf::from("open")
    } else {
        PathBuf::from("xdg-open")
    };
    Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn consent_window() -> Duration {
    if let Ok(raw) = bob_env::var("BOB_GCAL_CONSENT_SECS")
        && let Ok(secs) = raw.parse::<u64>()
        && secs > 0
    {
        return Duration::from_secs(secs);
    }
    CONSENT_WINDOW
}

/// First request line: method and target.
pub(crate) fn parse_request_line(headers: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(headers).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    Some((method.to_string(), target.to_string()))
}

fn read_headers(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let _ = stream.set_nonblocking(false);
    stream.set_read_timeout(Some(HEADER_READ_TIMEOUT))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 256];
    loop {
        if buf.len() >= HEADER_LIMIT {
            break;
        }
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                let room = HEADER_LIMIT - buf.len();
                buf.extend_from_slice(&tmp[..n.min(room)]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut
                    || error.kind() == io::ErrorKind::Interrupted =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(buf)
}

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> io::Result<()> {
    let _ = stream.set_nonblocking(false);
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn finish_and_confirm(
    gog: &Gog,
    account: &str,
    redirect: &str,
) -> Result<(), GcalError> {
    gog.auth_finish(account, redirect)?;
    confirm_account(gog, account)
}

fn confirm_account(gog: &Gog, account: &str) -> Result<(), GcalError> {
    let accounts = gog.accounts(Some(CLIENT))?;
    if accounts
        .iter()
        .any(|entry| entry.email == account && entry.has_calendar_readonly())
    {
        return Ok(());
    }
    if let Some(other) = accounts.iter().find(|entry| entry.email != account) {
        return Err(GcalError::runtime(
            "wrong_account",
            format!("signed in as {} instead of {account}", other.email),
        )
        .with_hint("choose the right account on Google's picker, or pass -a"));
    }
    Err(GcalError::runtime(
        "gog",
        "sign-in finished but Calendar access is missing".to_string(),
    )
    .with_hint("run `bob gcal login` again"))
}

fn is_auto_retry(error: &GcalError) -> bool {
    matches!(error.kind(), "stale_link" | "invalid_grant")
}

fn needs_tty() -> GcalError {
    GcalError::setup(
        "needs_tty",
        "the paste sign-in flow needs a terminal".to_string(),
    )
    .with_hint("run `bob gcal login` in a terminal")
}

fn cancelled() -> GcalError {
    GcalError::setup("cancelled", "sign-in cancelled".to_string())
        .with_hint("a stored client stays stored; rerun `bob gcal login`")
}

fn consent_denied() -> GcalError {
    GcalError::runtime(
        "consent_denied",
        "Google denied Calendar access".to_string(),
    )
    .with_hint("rerun `bob gcal login` and allow access")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(browser: bool, ssh: bool, display: bool, macos: bool) -> FlowEnv {
        FlowEnv {
            browser_override: browser,
            ssh,
            has_display: display,
            is_macos: macos,
        }
    }

    #[test]
    fn choose_flow_matrix() {
        let paste = choose_flow(true, env(true, true, true, true));
        assert_eq!(paste, (Flow::Paste, "you asked to paste"));

        let browser = choose_flow(false, env(true, true, false, false));
        assert_eq!(browser.0, Flow::Browser);

        let ssh = choose_flow(false, env(false, true, true, true));
        assert_eq!(ssh, (Flow::Paste, "you're on SSH"));

        let headless = choose_flow(false, env(false, false, false, false));
        assert_eq!(headless, (Flow::Paste, "no desktop session"));

        let linux_display = choose_flow(false, env(false, false, true, false));
        assert_eq!(linux_display.0, Flow::Browser);

        let mac = choose_flow(false, env(false, false, false, true));
        assert_eq!(mac.0, Flow::Browser);
    }

    #[test]
    fn parse_redirect_cases() {
        let state = "fakestate";
        let good =
            "http://127.0.0.1:36015/oauth2/callback?state=fakestate&code=abc";
        assert!(matches!(parse_redirect(good, state), RedirectParse::Url(_)));
        assert_eq!(parse_redirect("?", state), RedirectParse::Help);
        assert_eq!(parse_redirect("help", state), RedirectParse::Help);
        assert_eq!(parse_redirect("q", state), RedirectParse::Quit);
        assert_eq!(parse_redirect("  ", state), RedirectParse::Empty);
        assert!(matches!(
            parse_redirect(&format!("\"{good}\""), state),
            RedirectParse::Url(_)
        ));
        assert!(matches!(
            parse_redirect(&format!("<{good}>"), state),
            RedirectParse::Url(_)
        ));
        let bare = "127.0.0.1:36015/oauth2/callback?state=fakestate&code=abc";
        assert!(matches!(
            parse_redirect(bare, state),
            RedirectParse::Url(url) if url.starts_with("http://")
        ));
        assert!(matches!(
            parse_redirect(
                "http://example.com/oauth2/callback?state=fakestate&code=abc",
                state,
            ),
            RedirectParse::Error(_)
        ));
        assert_eq!(
            parse_redirect(
                "http://127.0.0.1:36015/oauth2/callback?state=fakestate&error=access_denied",
                state,
            ),
            RedirectParse::Denied
        );
        assert!(matches!(
            parse_redirect(
                "http://127.0.0.1:36015/oauth2/callback?state=fakestate",
                state,
            ),
            RedirectParse::Error(_)
        ));
        let stale = parse_redirect(
            "http://127.0.0.1:36015/oauth2/callback?state=other&code=abc",
            state,
        );
        assert!(
            matches!(stale, RedirectParse::Error(msg) if msg.contains("older sign-in"))
        );
    }

    #[test]
    fn login_hint_preserves_state() {
        let url =
            "https://accounts.google.com/o/oauth2/auth?state=abc&client_id=x";
        let out = inject_login_hint(url, "you@example.com").expect("hint");
        let parsed = Url::parse(&out).expect("url");
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs
            .iter()
            .any(|(k, v)| k == "login_hint" && v == "you@example.com"));
        assert_eq!(
            pairs
                .iter()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.as_str()),
            Some("abc")
        );
    }

    #[test]
    fn request_line_parsing() {
        let headers = b"GET /oauth2/callback?state=s&code=c HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let (method, target) = parse_request_line(headers).expect("line");
        assert_eq!(method, "GET");
        assert!(target.starts_with("/oauth2/callback"));
        assert!(parse_request_line(b"not a request").is_none());
    }

    #[test]
    fn html_escape_and_page() {
        let escaped = html_escape(r#"<script>"&'"#);
        assert_eq!(escaped, "&lt;script&gt;&quot;&amp;&#39;");
        let success = page(true, "you@example.com", "");
        assert!(success.contains("Bob can read your Google Calendar"));
        assert!(success.contains("you@example.com"));
        assert!(success.contains("prefers-color-scheme"));
        assert!(!success.contains("<script"));
        let fail = page(false, "you@example.com", "Google denied access");
        assert!(fail.contains("Sign-in didn't finish"));
        assert!(fail.contains("Google denied access"));
        let injected = page(false, "x", "<script>alert(1)</script>");
        assert!(!injected.contains("<script>alert"));
        assert!(injected.contains("&lt;script&gt;"));
    }
}

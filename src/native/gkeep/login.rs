//! `bob gkeep login`: exchange a sign-in cookie for a stored token.
//!
//! The cookie is read from a hidden TTY prompt, or from stdin when stdin
//! is not a TTY (`pbpaste | bob gkeep login`). It is exchanged through
//! the adapter, piped to `token_store_command` on stdin, read back with
//! `token_command`, and checked with a `snapshot`. The token travels on
//! stdin only and is never printed.

use std::{
    fs,
    io::{IsTerminal, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

use super::super::{env as bob_env, style::Styler};
use super::{
    adapter::{AdapterClient, Credentials},
    config::GkeepConfig,
    ui, GkeepError, LoginArgs,
};

/// Read the cookie on a TTY with echo disabled and restored by trap.
const TTY_COOKIE_HELPER: &str = r#"trap 'stty echo </dev/tty 2>/dev/null' EXIT INT TERM
stty -echo </dev/tty 2>/dev/null
printf '%s' 'Paste oauth_token (input hidden): ' >/dev/tty
IFS= read -r token </dev/tty
status=$?
stty echo </dev/tty 2>/dev/null
trap - EXIT INT TERM
printf '\n' >/dev/tty
printf '%s' "$token"
exit "$status""#;

pub(crate) fn run(args: &LoginArgs) -> i32 {
    let config = match GkeepConfig::resolve(args.email.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            return ui::report_error("login", &error, "human");
        }
    };
    // Check the store before touching the single-use cookie.
    if let Err(error) = check_store_command(&config) {
        return ui::report_error("login", &error, "human");
    }
    if std::io::stdin().is_terminal() {
        print_login_steps(config.email());
    }
    let cookie = match read_cookie() {
        Ok(cookie) => cookie,
        Err(error) => {
            return ui::report_error("login", &error, "human");
        }
    };
    if !cookie.starts_with("oauth2_4/") {
        ui::warn(
            "the value does not start with `oauth2_4/`; continuing \
             anyway",
        );
    }
    let client = match AdapterClient::resolve(&config) {
        Ok(client) => client,
        Err(error) => {
            return ui::report_error("login", &error, "human");
        }
    };
    let master_token = match client.exchange(
        config.email(),
        &cookie,
        config.device_id(),
        None,
    ) {
        Ok(token) => token,
        Err(error) => {
            return ui::report_error("login", &error, "human");
        }
    };
    if let Err(detail) = store_token(&config, &master_token) {
        return store_failure(&master_token, detail);
    }
    match config.read_token() {
        Ok((stored, _)) if stored == master_token => {}
        Ok(_) => {
            return store_failure(
                &master_token,
                "the stored token did not match the exchanged token"
                    .to_string(),
            );
        }
        Err(error) => {
            return store_failure(&master_token, error.message().to_string());
        }
    }
    let credentials = Credentials::from_config(&config, &master_token);
    let notes = match client.snapshot(&credentials, false, None) {
        Ok(notes) => notes,
        Err(error) => {
            return ui::report_error("login", &error, "human");
        }
    };

    let styler = Styler::detect();
    println!("{} exchanged for a master token", styler.green("✓"));
    println!(
        "{} stored and readable via {}",
        styler.green("✓"),
        config.token_command()
    );
    println!(
        "{} Google Keep reachable · {}",
        styler.green("✓"),
        notes_in_inbox(notes.len())
    );
    0
}

/// The numbered setup steps, shown on stderr before a TTY prompt.
fn print_login_steps(email: &str) {
    let styler = Styler::detect();
    eprintln!("{}", styler.cyan(&format!("Google Keep login · {email}")));
    eprintln!();
    eprintln!(
        "  1. Open https://accounts.google.com/EmbeddedSetup and sign in."
    );
    eprintln!(
        "  2. Click \"I agree\" (the page may then spin forever; that's \
         expected)."
    );
    eprintln!(
        "  3. In DevTools → Application → Cookies → accounts.google.com, \
         copy the"
    );
    eprintln!(
        "     value of the `oauth_token` cookie (it starts with \
         oauth2_4/)."
    );
}

/// Read the cookie from the hidden TTY prompt or piped stdin.
fn read_cookie() -> Result<String, GkeepError> {
    if std::io::stdin().is_terminal() {
        read_cookie_tty()
    } else {
        read_cookie_stdin()
    }
}

/// The first non-empty stdin line, trimmed.
fn read_cookie_stdin() -> Result<String, GkeepError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| {
            GkeepError::setup(
                "login",
                format!("read the oauth_token from stdin: {error}"),
            )
        })?;
    first_cookie_line(&input).ok_or_else(|| {
        GkeepError::setup("login", "no oauth_token on stdin".to_string())
            .with_hint(
                "pipe the cookie, for example `pbpaste | bob gkeep login`",
            )
    })
}

/// The first non-empty trimmed line, if any.
fn first_cookie_line(input: &str) -> Option<String> {
    input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Run the hidden-prompt helper and return what it printed.
fn read_cookie_tty() -> Result<String, GkeepError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(TTY_COOKIE_HELPER)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| {
            GkeepError::setup(
                "login",
                format!("prompt for the oauth_token: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(GkeepError::setup(
            "login",
            "no oauth_token entered".to_string(),
        ));
    }
    let cookie = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if cookie.is_empty() {
        return Err(GkeepError::setup(
            "login",
            "no oauth_token entered".to_string(),
        ));
    }
    Ok(cookie)
}

/// The first word of `token_store_command` must resolve before the
/// single-use cookie is consumed.
fn check_store_command(config: &GkeepConfig) -> Result<(), GkeepError> {
    let first = config
        .token_store_command()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    if first.is_empty() {
        return Err(GkeepError::setup(
            "login",
            "gkeep.token_store_command is empty".to_string(),
        )
        .with_hint(
            "set gkeep.token_store_command in ~/.config/bob/config.yml",
        ));
    }
    let found = Command::new("sh")
        .arg("-c")
        .arg("command -v \"$1\" >/dev/null 2>&1")
        .arg("bob gkeep login")
        .arg(first)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if found {
        Ok(())
    } else {
        Err(GkeepError::setup(
            "login",
            format!("token store command not found: {first}"),
        )
        .with_hint("set gkeep.token_store_command in ~/.config/bob/config.yml"))
    }
}

/// Pipe the fresh token to `token_store_command` on stdin.
fn store_token(config: &GkeepConfig, master_token: &str) -> Result<(), String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(config.token_store_command())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("start the token store command: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(master_token.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|error| {
                format!("write to the token store command: {error}")
            })?;
    }
    let status = child.wait().map_err(|error| {
        format!("wait for the token store command: {error}")
    })?;
    if status.success() {
        Ok(())
    } else if let Some(code) = status.code() {
        Err(format!("the token store command exited {code}"))
    } else {
        Err("the token store command was terminated".to_string())
    }
}

/// The exchanged token could not be stored or verified: save a 0600
/// recovery copy (never printed) and report exit 1.
fn store_failure(master_token: &str, detail: String) -> i32 {
    let error = GkeepError::runtime(
        "store",
        format!("store the master token: {detail}"),
    );
    let code = ui::report_error("login", &error, "human");
    match save_recovery_token(master_token) {
        Ok(path) => {
            eprintln!(
                "saved the master token to {} (0600); move it into your \
                 store and delete the file",
                path.display()
            );
        }
        Err(save_error) => {
            eprintln!(
                "could not save a recovery copy ({save_error}); run `bob \
                 gkeep login` again"
            );
        }
    }
    code
}

/// The recovery copy path under the state dir.
fn recovery_path() -> PathBuf {
    bob_env::bob_cli_state_dir()
        .join("gkeep")
        .join("master_token.recovered")
}

/// Write the token to the recovery path with 0600 permissions at open.
fn save_recovery_token(master_token: &str) -> Result<PathBuf, String> {
    let path = recovery_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|error| {
                    format!("chmod {}: {error}", parent.display())
                })?;
        }
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    {
        use std::io::Write as _;
        file.write_all(format!("{master_token}\n").as_bytes())
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        file.sync_all()
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// `1 note in inbox` or `N notes in inbox`.
fn notes_in_inbox(count: usize) -> String {
    if count == 1 {
        "1 note in inbox".to_string()
    } else {
        format!("{count} notes in inbox")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_line_picks_the_first_non_empty_line() {
        assert_eq!(
            first_cookie_line("  \noauth2_4/abc\nsecond\n"),
            Some("oauth2_4/abc".to_string())
        );
        assert_eq!(first_cookie_line("   \n\t\n"), None);
        assert_eq!(first_cookie_line(""), None);
    }

    #[test]
    fn inbox_count_handles_singular_and_plural() {
        assert_eq!(notes_in_inbox(0), "0 notes in inbox");
        assert_eq!(notes_in_inbox(1), "1 note in inbox");
        assert_eq!(notes_in_inbox(4), "4 notes in inbox");
    }

    #[test]
    fn recovery_path_lives_under_the_state_dir() {
        assert_eq!(
            recovery_path(),
            bob_env::bob_cli_state_dir()
                .join("gkeep")
                .join("master_token.recovered")
        );
    }
}

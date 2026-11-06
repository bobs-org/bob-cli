//! `bob gmail logout`: remove Bob's Gmail grant only.

use std::io::{self, BufRead, IsTerminal, Write};

use serde_json::json;

use super::super::style::Styler;
use super::{
    account,
    config::{assume_tty, GmailConfig},
    gog::{Gog, CLIENT, REVOKE_URL},
    ui, GmailError, LogoutArgs,
};

pub(crate) fn run(args: &LogoutArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GmailConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("logout", &error, format),
    };
    match logout(args, &config) {
        Ok(account) => {
            print_report(&account, args.format.is_json());
            0
        }
        Err(error) => ui::report_error("logout", &error, format),
    }
}

fn logout(
    args: &LogoutArgs,
    config: &GmailConfig,
) -> Result<String, GmailError> {
    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    let _ = gog.require_version()?;
    let account = account::resolve_logout(
        &gog,
        args.account.as_deref(),
        config.account(),
    )?;
    if !args.force {
        confirm_logout(&account)?;
    }
    gog.auth_remove(&account)?;
    super::worker::forget_login(&account);
    Ok(account)
}

fn confirm_logout(account: &str) -> Result<(), GmailError> {
    if !(assume_tty() || io::stdin().is_terminal()) {
        return Err(GmailError::setup(
            "not_logged_in",
            "logout needs --force when stdin is not a TTY".to_string(),
        )
        .with_hint("rerun `bob gmail logout -y`"));
    }
    eprint!("Remove bob-gmail Gmail grant for {account}? [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).map_err(|error| {
        GmailError::setup(
            "not_logged_in",
            format!("read the logout prompt: {error}"),
        )
    })?;
    let answer = line.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        Err(GmailError::setup(
            "not_logged_in",
            "logout cancelled".to_string(),
        ))
    }
}

fn print_report(account: &str, json: bool) {
    if json {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "account": account,
                "client": CLIENT,
                "revoke_url": REVOKE_URL,
            })
        );
        ui::flush_stdout();
        return;
    }
    let styler = Styler::detect();
    println!(
        "{} removed bob-gmail Gmail grant for {account}",
        styler.green("✓")
    );
    println!("Revoke Bob's Gmail access at {REVOKE_URL}");
    ui::flush_stdout();
}

//! `bob gcal calendars`: list calendars with mapped roots.

use super::super::style::{self, Styler};
use super::{
    config::GcalConfig,
    gog::{Gog, GogCalendar},
    ui, CalendarsArgs, GcalError,
};

pub(crate) fn run(args: &CalendarsArgs) -> i32 {
    let format = args.error_format();
    match list(args) {
        Ok(()) => 0,
        Err(error) => ui::report_error("calendars", &error, format),
    }
}

fn list(args: &CalendarsArgs) -> Result<(), GcalError> {
    let config = GcalConfig::resolve()?;
    let gog = Gog::resolve(config.timeout())?;
    let _ = gog.require_version()?;
    let account = resolve_account(&gog, &config)?;
    let calendars = gog.calendars(&account)?;
    if args.format.is_json() {
        print_json(&calendars, &config);
    } else {
        print_table(&calendars, &config);
    }
    Ok(())
}

pub(crate) fn resolve_account(
    gog: &Gog,
    config: &GcalConfig,
) -> Result<String, GcalError> {
    if let Some(email) = config.account() {
        return Ok(email.to_string());
    }
    let accounts = gog.accounts(None)?;
    match accounts.as_slice() {
        [account] => Ok(account.email.clone()),
        [] => Err(GcalError::setup(
            "not_logged_in",
            "no Google account is configured".to_string(),
        )
        .with_hint("run `bob gcal login` or set gcal.account")),
        _ => Err(GcalError::setup(
            "not_logged_in",
            "gog holds more than one account; set gcal.account".to_string(),
        )),
    }
}

fn print_json(calendars: &[GogCalendar], config: &GcalConfig) {
    let rows: Vec<serde_json::Value> = calendars
        .iter()
        .map(|calendar| {
            serde_json::json!({
                "id": calendar.id,
                "name": calendar.name(),
                "access_role": calendar.access_role,
                "color": calendar.colour(),
                "primary": calendar.primary,
                "synced": config.syncs(&calendar.id, calendar.primary),
                "root": config.root_for(if calendar.primary {
                    "primary"
                } else {
                    &calendar.id
                }),
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "schema_version": 1,
            "ok": true,
            "calendars": rows,
        })
    );
    ui::flush_stdout();
}

fn print_table(calendars: &[GogCalendar], config: &GcalConfig) {
    let styler = Styler::detect();
    let rows: Vec<[String; 6]> = calendars
        .iter()
        .map(|calendar| {
            let synced = if config.syncs(&calendar.id, calendar.primary) {
                "yes"
            } else {
                "no"
            };
            let root_id = if calendar.primary {
                "primary"
            } else {
                calendar.id.as_str()
            };
            [
                calendar.id.clone(),
                calendar.name().to_string(),
                calendar.access_role.clone().unwrap_or_default(),
                calendar.colour().unwrap_or("-").to_string(),
                synced.to_string(),
                config.root_for(root_id).to_string(),
            ]
        })
        .collect();
    let headers = ["ID", "NAME", "ROLE", "COLOUR", "SYNC", "ROOT"];
    let widths: Vec<usize> = (0..6)
        .map(|col| {
            rows.iter()
                .map(|row| style::display_width(&row[col]))
                .chain(std::iter::once(headers[col].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    println!("{}", styler.cyan("Google Calendar · calendars"));
    println!(
        "{}",
        styler.dim(&format_row(&headers.map(str::to_string), &widths))
    );
    for row in &rows {
        println!("{}", format_row(row, &widths));
    }
    ui::flush_stdout();
}

fn format_row(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| style::pad_right(cell, *width))
        .collect::<Vec<_>>()
        .join("  ")
}

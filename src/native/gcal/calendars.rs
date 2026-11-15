//! `bob gcal calendars`: list calendars with mapped roots.

use super::super::style::{self, Styler};
use super::{
    account,
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
    let gog = Gog::resolve(
        crate::native::google::GCAL,
        config.gog(),
        config.timeout(),
    )?;
    let _ = gog.require_version()?;
    let account = account::resolve_command(&gog, config.account())?;
    let calendars = gog.calendars(&account)?;
    if args.format.is_json() {
        print_json(&calendars, &config);
    } else {
        print_table(&calendars, &config, &account);
    }
    Ok(())
}

fn print_json(calendars: &[GogCalendar], config: &GcalConfig) {
    let rows: Vec<serde_json::Value> = calendars
        .iter()
        .map(|calendar| {
            // Match the table: `primary` (or the account email) configures
            // the primary calendar, so look the mark up by configured id.
            let mark = config
                .calendars()
                .iter()
                .find(|configured| {
                    calendar.matches_configured_id(&configured.id)
                })
                .and_then(|configured| configured.mark.as_deref());
            serde_json::json!({
                "id": calendar.id,
                "name": calendar.name(),
                "access_role": calendar.access_role,
                "color": calendar.colour(),
                "primary": calendar.primary,
                "synced": config.syncs(&calendar.id, calendar.primary),
                "mark": mark,
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

fn print_table(calendars: &[GogCalendar], config: &GcalConfig, account: &str) {
    let styler = Styler::detect();
    let mut ordered: Vec<(
        &GogCalendar,
        Option<&crate::native::config::GcalCalendarSettings>,
    )> = Vec::new();
    for configured in config.calendars() {
        if let Some(calendar) = calendars
            .iter()
            .find(|calendar| calendar.matches_configured_id(&configured.id))
            && !ordered.iter().any(|(seen, _)| seen.id == calendar.id)
        {
            ordered.push((calendar, Some(configured)));
        }
    }
    for calendar in calendars {
        if !ordered.iter().any(|(seen, _)| seen.id == calendar.id) {
            ordered.push((calendar, None));
        }
    }
    let rows: Vec<([String; 7], bool)> = ordered
        .iter()
        .map(|(calendar, configured)| {
            let synced = configured.is_some();
            let mark = configured
                .and_then(|configured| configured.mark.as_deref())
                .unwrap_or("—");
            let root = configured.map_or_else(
                || "—".to_string(),
                |configured| config.root_for(&configured.id).to_string(),
            );
            (
                [
                    calendar.name().to_string(),
                    if synced { "yes" } else { "no" }.to_string(),
                    mark.to_string(),
                    root,
                    calendar.access_role.clone().unwrap_or_default(),
                    calendar.colour().unwrap_or("-").to_string(),
                    calendar.id.clone(),
                ],
                synced,
            )
        })
        .collect();
    let headers = ["NAME", "SYNC", "MARK", "ROOT", "ROLE", "COLOUR", "ID"];
    let widths: Vec<usize> = (0..7)
        .map(|col| {
            rows.iter()
                .map(|(row, _)| style::display_width(&row[col]))
                .chain(std::iter::once(headers[col].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    println!(
        "{}",
        styler.cyan(&format!("Google Calendar · calendars · {account}"))
    );
    println!(
        "{}",
        styler.dim(&format_row(&headers.map(str::to_string), &widths))
    );
    for (row, synced) in &rows {
        let line = format_row(row, &widths);
        println!("{}", if *synced { line } else { styler.dim(&line) });
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

//! Default-root validation shared by doctor now and list/sync later.

use std::path::{Path, PathBuf};

use crate::native::{
    parent_notes,
    pomodoro_homes::{path_string, DiskHomeSource, Home, HomeResolver, Hue},
    task_root::is_invalid_root_value,
    vault_links::LinkResolution,
};

use super::{config::GcalConfig, model::GcalEvent};

/// A resolved, valid root candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootTarget {
    pub name: String,
    pub path: PathBuf,
    pub home: Home,
}

/// Why a root candidate was rejected, with R11 reason and fix text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootRejection {
    pub reason: &'static str,
    pub message: String,
    pub fix: String,
}

/// How the chosen root was selected. Distinguishes series memory from
/// Google-owned sources without string-matching the `"learned from"` label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RootSourceKind {
    #[default]
    None,
    /// Source 1: series memory of an earlier `[root::]` edit.
    SeriesMemory,
    /// Title-ending or description-only `@route` token.
    Token,
    /// Event colour mapping (`gcal.colors`).
    Colour,
    /// Calendar-level `root`.
    Calendar,
    /// Config default (`gcal.root`).
    Default,
}

/// Chosen root and user-facing provenance for one Event Task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventRoot {
    pub value: Option<String>,
    pub source: Option<String>,
    pub kind: RootSourceKind,
    pub hue: Option<Hue>,
    pub warnings: Vec<String>,
    pub title: String,
}

/// Choose the first valid root in the fixed precedence order.
pub(crate) fn choose_event_root(
    vault: &Path,
    daily_rel: &Path,
    event: &GcalEvent,
    calendar_name: &str,
    config: &GcalConfig,
    learned: Option<&super::journal::LearnedRoot>,
) -> EventRoot {
    let mut warnings = Vec::new();
    let mut title = event.title.clone();

    // Source 1: your earlier `[root::]` edit on another occurrence of the
    // same recurring event taught the series.
    if let Some(learned) = learned
        && !learned.root.trim().is_empty()
    {
        let label = if learned.day.is_empty() {
            "learned from your edit".to_string()
        } else {
            format!("learned from your {} edit", learned.day)
        };
        match validate_root_candidate(vault, daily_rel, &learned.root) {
            Ok(target) => {
                return chosen(
                    target,
                    label,
                    RootSourceKind::SeriesMemory,
                    warnings,
                    title,
                );
            }
            Err(rejection) => warnings.push(rejection_message(
                &format!("series memory ({})", learned.root),
                &rejection,
            )),
        }
    }

    if let Some(route) = event
        .title_route_hint
        .as_deref()
        .or(event.route_hint.as_deref())
    {
        match parent_notes::resolve_parent(vault, route) {
            Ok(parent) => {
                match validate_root_candidate(vault, daily_rel, &parent.route) {
                    Ok(target) => {
                        if event.title_route_hint.as_deref() == Some(route) {
                            title = strip_title_route(&event.title, route);
                        }
                        return chosen(
                            target,
                            format!("@{route}"),
                            RootSourceKind::Token,
                            warnings,
                            title,
                        );
                    }
                    Err(rejection) => warnings.push(rejection_message(
                        &format!("@{route}"),
                        &rejection,
                    )),
                }
            }
            Err(error) => {
                let mut warning = format!(
                    "route_unknown (@{route}): {}",
                    error.message().replace('\n', "; ")
                );
                if let Err(rejection) =
                    validate_root_candidate(vault, daily_rel, route)
                {
                    warning.push_str("; ");
                    warning.push_str(&rejection_message(
                        &format!("@{route}"),
                        &rejection,
                    ));
                }
                warnings.push(warning);
            }
        }
    }

    if let Some(color_id) = event.color_id.as_deref()
        && let Some(root) = config.color_root(color_id)
    {
        let color = crate::native::config::color_name_for_id(color_id)
            .unwrap_or(color_id);
        match validate_root_candidate(vault, daily_rel, root) {
            Ok(target) => {
                return chosen(
                    target,
                    format!("colour {color}"),
                    RootSourceKind::Colour,
                    warnings,
                    title,
                );
            }
            Err(rejection) => warnings.push(rejection_message(
                &format!("colour {color}"),
                &rejection,
            )),
        }
    }

    if let Some(root) = config.calendar_root(&event.key.calendar_id) {
        match validate_root_candidate(vault, daily_rel, root) {
            Ok(target) => {
                return chosen(
                    target,
                    format!("calendar {calendar_name}"),
                    RootSourceKind::Calendar,
                    warnings,
                    title,
                );
            }
            Err(rejection) => warnings.push(rejection_message(
                &format!("calendar {calendar_name}"),
                &rejection,
            )),
        }
    }

    match validate_root_candidate(vault, daily_rel, config.root()) {
        Ok(target) => chosen(
            target,
            "default".to_string(),
            RootSourceKind::Default,
            warnings,
            title,
        ),
        Err(rejection) => {
            warnings.push(rejection_message("default", &rejection));
            EventRoot {
                value: None,
                source: None,
                kind: RootSourceKind::None,
                hue: None,
                warnings,
                title,
            }
        }
    }
}

fn chosen(
    target: RootTarget,
    source: String,
    kind: RootSourceKind,
    warnings: Vec<String>,
    title: String,
) -> EventRoot {
    EventRoot {
        value: Some(target.name),
        source: Some(source),
        kind,
        hue: target.home.hue,
        warnings,
        title,
    }
}

fn rejection_message(source: &str, rejection: &RootRejection) -> String {
    format!(
        "{source} rejected ({}): {}; fix: {}",
        rejection.reason, rejection.message, rejection.fix
    )
}

fn strip_title_route(title: &str, route: &str) -> String {
    let token = format!("@{route}");
    let Some((prefix, suffix)) = title.rsplit_once(char::is_whitespace) else {
        return "(untitled event)".to_string();
    };
    if suffix == token {
        let prefix = prefix.trim_end();
        if prefix.is_empty() {
            "(untitled event)".to_string()
        } else {
            prefix.to_string()
        }
    } else {
        title.to_string()
    }
}

/// Validate `name` as a `[root::]` value relative to today's daily note.
pub(crate) fn validate_root_candidate(
    vault: &Path,
    daily_rel: &Path,
    name: &str,
) -> Result<RootTarget, RootRejection> {
    let trimmed = name.trim();
    if trimmed.is_empty() || is_invalid_root_value(trimmed) {
        return Err(RootRejection {
            reason: "invalid",
            message: format!(
                "{trimmed:?} is not a valid root name (blank, or contains \
                 [ ] | # ^)"
            ),
            fix: "write the bare note name, for example meet".to_string(),
        });
    }

    let source = DiskHomeSource::new(vault);
    let mut resolver = HomeResolver::new(source);
    match resolver.resolve_target(daily_rel, trimmed) {
        LinkResolution::Missing => Err(RootRejection {
            reason: "missing",
            message: format!("no note matches {trimmed:?}"),
            fix: format!("create {trimmed}.md or pick an existing area"),
        }),
        LinkResolution::Ambiguous => Err(RootRejection {
            reason: "ambiguous",
            message: format!("two or more notes match {trimmed:?}"),
            fix: format!("use a vault-relative path such as {trimmed}.md"),
        }),
        LinkResolution::Found(path) => {
            let home = resolver.resolve_note(&path);
            if home.key == "@unfiled" {
                return Err(RootRejection {
                    reason: "no_home",
                    message: format!(
                        "{} is not an area or project",
                        path.display()
                    ),
                    fix: format!(
                        "set type: area or type: project on {}",
                        path.display()
                    ),
                });
            }
            let target_key = path_string(&path);
            if home.root_parent != target_key {
                return Err(RootRejection {
                    reason: "root_field_not_root",
                    message: format!(
                        "{trimmed} groups under {}; write [root:: {}] or \
                         set root: true on {trimmed}",
                        home.root_parent, home.root_parent
                    ),
                    fix: format!(
                        "{} groups under {}; write [root:: {}] or set \
                         root: true on {trimmed}",
                        trimmed, home.root_parent, home.root_parent
                    ),
                });
            }
            Ok(RootTarget {
                name: trimmed.to_string(),
                path,
                home,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::{GcalCalendarSettings, GcalSettings};
    use crate::native::gcal::model::GcalEvent;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs;

    fn write_note(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, body).expect("write note");
    }

    #[test]
    fn rejects_invalid_and_missing() {
        let dir = tempfile::tempdir().expect("temp");
        write_note(dir.path(), "2026/20261030.md", "# day\n");
        let daily = Path::new("2026/20261030.md");
        let invalid = validate_root_candidate(dir.path(), daily, "[[meet]]")
            .expect_err("wikilink");
        assert_eq!(invalid.reason, "invalid");
        let missing = validate_root_candidate(dir.path(), daily, "meet")
            .expect_err("missing");
        assert_eq!(missing.reason, "missing");
    }

    #[test]
    fn accepts_an_area_that_is_its_own_root() {
        let dir = tempfile::tempdir().expect("temp");
        write_note(
            dir.path(),
            "meet.md",
            "---\ntype: area\ncolor: violet\n---\n# Meet\n",
        );
        write_note(dir.path(), "2026/20261030.md", "# day\n");
        let target = validate_root_candidate(
            dir.path(),
            Path::new("2026/20261030.md"),
            "meet",
        )
        .expect("area");
        assert_eq!(target.name, "meet");
        assert_eq!(
            target.home.kind,
            crate::native::pomodoro_homes::HomeKind::Area
        );
    }

    fn config(
        calendar_root: Option<&str>,
        color_root: Option<&str>,
        default_root: &str,
    ) -> GcalConfig {
        GcalConfig::from_settings(GcalSettings {
            root: Some(default_root.to_string()),
            calendars: Some(vec![GcalCalendarSettings {
                id: "primary".to_string(),
                root: calendar_root.map(str::to_string),
            }]),
            colors: color_root
                .map(|root| {
                    BTreeMap::from([("tomato".to_string(), root.to_string())])
                })
                .unwrap_or_default(),
            ..GcalSettings::default()
        })
        .expect("config")
    }

    fn event(
        title: &str,
        description: Option<&str>,
        color_id: Option<&str>,
    ) -> GcalEvent {
        let mut value = json!({
            "id": "event-1",
            "summary": title,
            "status": "confirmed",
            "start": {"dateTime": "2026-10-30T08:00:00-04:00"},
            "end": {"dateTime": "2026-10-30T09:00:00-04:00"},
            "attendees": [{"self": true, "responseStatus": "accepted"}]
        });
        if let Some(description) = description {
            value["description"] = json!(description);
        }
        if let Some(color_id) = color_id {
            value["colorId"] = json!(color_id);
        }
        GcalEvent::from_google("primary", value).expect("event")
    }

    fn root_vault() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp vault");
        for area in ["meet", "body", "job", "health"] {
            write_note(
                dir.path(),
                &format!("{area}.md"),
                &format!("---\ntype: area\n---\n# {area}\n"),
            );
        }
        dir
    }

    #[test]
    fn route_token_precedes_color_calendar_and_default() {
        let vault = root_vault();
        let event = event("Dentist @body", Some("@job"), Some("11"));
        let root = choose_event_root(
            vault.path(),
            Path::new("2026/20261030.md"),
            &event,
            "Personal",
            &config(Some("health"), Some("job"), "meet"),
            None,
        );
        assert_eq!(root.value.as_deref(), Some("body"));
        assert_eq!(root.source.as_deref(), Some("@body"));
        assert_eq!(root.kind, RootSourceKind::Token);
        assert_eq!(root.title, "Dentist");
        assert!(root.warnings.is_empty());
    }

    #[test]
    fn unknown_title_route_stays_in_title_and_color_wins_next() {
        let vault = root_vault();
        let event = event("Dentist @missing", None, Some("11"));
        let root = choose_event_root(
            vault.path(),
            Path::new("2026/20261030.md"),
            &event,
            "Personal",
            &config(Some("health"), Some("job"), "meet"),
            None,
        );
        assert_eq!(root.value.as_deref(), Some("job"));
        assert_eq!(root.source.as_deref(), Some("colour tomato"));
        assert_eq!(root.kind, RootSourceKind::Colour);
        assert_eq!(root.title, "Dentist @missing");
        assert!(root.warnings[0].starts_with("route_unknown"));
    }

    #[test]
    fn invalid_color_falls_back_to_calendar_and_invalid_default_has_no_root() {
        let vault = root_vault();
        let meeting_event = event("Dentist", None, Some("11"));
        let calendar = choose_event_root(
            vault.path(),
            Path::new("2026/20261030.md"),
            &meeting_event,
            "Personal",
            &config(Some("health"), Some("missing"), "meet"),
            None,
        );
        assert_eq!(calendar.value.as_deref(), Some("health"));
        assert_eq!(calendar.source.as_deref(), Some("calendar Personal"));
        assert_eq!(calendar.kind, RootSourceKind::Calendar);
        assert!(calendar.warnings[0].contains("missing"));
        assert!(calendar.warnings[0].contains("fix:"));

        let invalid_default = choose_event_root(
            vault.path(),
            Path::new("2026/20261030.md"),
            &event("Dinner", None, None),
            "Personal",
            &config(None, None, "missing"),
            None,
        );
        assert_eq!(invalid_default.value, None);
        assert_eq!(invalid_default.kind, RootSourceKind::None);
        assert!(invalid_default.warnings[0].contains("default rejected"));
    }
}

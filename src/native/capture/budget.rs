//! Plan-budget reporting for `bob capture`.
//!
//! After a batch is planned (dry-run and real runs share the planner),
//! today's daily note is compared before and after. When its Pomodoros
//! section changed, a top-level `plan_budget` joins the result with
//! the before/after Task Link meter; a warning fires only when the link
//! count grows past its cap. Types are listed on the budget, never
//! budgeted: new types never warn and never refuse.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn pomodoros_section_text(contents: &str) -> Option<String> {
    let lines: Vec<&str> = contents.lines().collect();
    let range = pomodoro::pomodoros_section_range(&lines)?;
    Some(lines[range].join("\n"))
}

/// One cap warning inside [`CapturePlanBudget`]: only the link cap
/// ever fires here, and only while growing past the cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PlanBudgetWarning {
    pub(super) code: String,
    pub(super) message: String,
}

/// A before/after meter inside [`CapturePlanBudget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PlanBudgetMeter {
    pub(super) count: usize,
    pub(super) cap: u32,
    pub(super) over: bool,
    pub(super) before: usize,
}

/// Top-level `plan_budget` on [`CaptureResult`]: present only when the
/// batch changed today's Pomodoros section. It is not per item. The
/// `added_themes` list is hue data for clients, not a meter: nothing
/// about types ever warns or refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CapturePlanBudget {
    pub(super) status: plan_budget::PlanStatus,
    pub(super) links: PlanBudgetMeter,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) added_themes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) added_theme_refs: Vec<super::super::pomodoro_themes::ThemeRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<PlanBudgetWarning>,
}

#[cfg(test)]
pub(super) static THEME_CATALOG_LOADS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn reset_theme_catalog_loads() {
    THEME_CATALOG_LOADS.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(super) fn theme_catalog_loads() -> usize {
    THEME_CATALOG_LOADS.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
fn record_theme_catalog_load() {
    THEME_CATALOG_LOADS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Attach catalog-backed refs to capture destinations and the changed plan
/// budget. Kept best-effort so a damaged theme journal cannot block a capture.
pub(super) fn append_theme_refs(
    request: &CaptureRequest,
    batch: &mut PlannedCaptureBatch,
) -> BTreeMap<String, Vec<super::super::pomodoro_themes::ThemeRef>> {
    let mut names = BTreeSet::new();
    for item in &batch.items {
        for endpoint in [
            item.result.pomodoro_link_source.as_ref(),
            item.result.pomodoro_link_destination.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(name) = endpoint.name.as_deref() {
                names.insert(name.to_string());
            }
        }
    }
    if let Some(budget) = batch.plan_budget.as_ref() {
        names.extend(budget.added_themes.iter().cloned());
    }
    if names.is_empty() {
        return BTreeMap::new();
    }
    // Cheap no-vocabulary fast path: with no configured types there are no
    // refs to attach, so skip the full vault catalog load.
    if config::load_theme_sources().themes.is_none() {
        return BTreeMap::new();
    }

    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let today = super::super::capture_pomodoros_agenda::agenda_date(&day_file);
    #[cfg(test)]
    record_theme_catalog_load();
    let Ok(catalog) = super::super::pomodoro_themes::Catalog::load(
        &request.bob_dir,
        today,
        false,
    ) else {
        return BTreeMap::new();
    };
    let refs_by_name: BTreeMap<_, _> = names
        .into_iter()
        .map(|name| {
            let key = plan_budget::normalize_component(&name);
            (key, catalog.theme_refs_for_name(&name))
        })
        .collect();
    if let Some(budget) = batch.plan_budget.as_mut() {
        budget.added_theme_refs = budget
            .added_themes
            .iter()
            .flat_map(|name| catalog.theme_refs_for_name(name))
            .collect();
    }
    refs_by_name
}

/// Structured `theme_not_configured` warnings for names whose entry
/// count grew on a staged canonical daily note (or today's day file).
/// An invalid `themes:` block is a plain-string warning. Never blocks.
pub(super) fn append_unconfigured_theme_warnings(
    request: &CaptureRequest,
    batch: &mut PlannedCaptureBatch,
) {
    // Cheap pre-checks avoid the full vault catalog load on the common
    // no-growth path. Only the config warning and the type vocabulary are
    // needed before the daily-note loop.
    let sources = config::load_theme_sources();
    let push_config_warning = |batch: &mut PlannedCaptureBatch,
                               warning: &str| {
        let already = batch
            .warnings
            .iter()
            .any(|existing| existing.message() == warning);
        if !already {
            batch.warnings.push(warning.to_string().into());
        }
    };
    if sources.themes.is_none() {
        if let Some(warning) = sources.warning.as_deref() {
            push_config_warning(batch, warning);
        }
        return;
    }
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let close_created = close_created_component_counts(batch);
    let mut any_grew = false;
    for file in &batch.text_files {
        if !is_daily_note(&request.bob_dir, &file.target, &day_file) {
            continue;
        }
        let before = component_entry_counts(&file.original_target);
        let after = component_entry_counts(&file.updated_target);
        for (key, _) in authored_components_in_order(&file.updated_target) {
            let grew = after.get(&key).copied().unwrap_or(0)
                > before.get(&key).copied().unwrap_or(0)
                    + close_created.get(&key).copied().unwrap_or(0);
            if grew {
                any_grew = true;
                break;
            }
        }
        if any_grew {
            break;
        }
    }
    if !any_grew {
        if let Some(warning) = sources.warning.as_deref() {
            push_config_warning(batch, warning);
        }
        return;
    }
    let today = super::super::capture_pomodoros_agenda::agenda_date(&day_file);
    #[cfg(test)]
    record_theme_catalog_load();
    let Ok(catalog) = super::super::pomodoro_themes::Catalog::load(
        &request.bob_dir,
        today,
        false,
    ) else {
        return;
    };
    if let Some(warning) = catalog.config_warning() {
        push_config_warning(batch, warning);
    }
    if !catalog.has_configured() {
        return;
    }

    let close_created = close_created_component_counts(batch);
    let mut seen = BTreeSet::new();
    for file in &batch.text_files {
        if !is_daily_note(&request.bob_dir, &file.target, &day_file) {
            continue;
        }
        let before = component_entry_counts(&file.original_target);
        let after = component_entry_counts(&file.updated_target);
        for (key, name) in authored_components_in_order(&file.updated_target) {
            // Closing (and its inherited next-Pomodoro row) does not
            // count as using the name; only a real entry-count increase
            // beyond those rows does.
            let grew = after.get(&key).copied().unwrap_or(0)
                > before.get(&key).copied().unwrap_or(0)
                    + close_created.get(&key).copied().unwrap_or(0);
            if !grew || !catalog.unconfigured(&key) || !seen.insert(key.clone())
            {
                continue;
            }
            let similar = catalog.similar_to(&name);
            batch
                .warnings
                .push(CaptureWarning::Structured(StructuredCaptureWarning {
                code: super::super::pomodoro_themes::THEME_NOT_CONFIGURED,
                label: Some(name.clone()),
                count: None,
                message:
                    super::super::pomodoro_themes::theme_not_configured_message(
                        &name, &similar,
                    ),
            }));
        }
    }
}

/// Next-Pomodoro rows a close creates inherit the closed name; those
/// extra entries are not a use of the theme.
fn close_created_component_counts(
    batch: &PlannedCaptureBatch,
) -> BTreeMap<String, u32> {
    let mut counts = BTreeMap::new();
    for item in &batch.items {
        let Some(close) = item.result.pomodoro_close.as_ref() else {
            continue;
        };
        let Some(next) = close.next_pomodoro.as_ref() else {
            continue;
        };
        if !next.created {
            continue;
        }
        let Some(name) = next.name.as_deref() else {
            continue;
        };
        for component in plan_budget::split_components(name) {
            let key = plan_budget::normalize_component(&component);
            if key.is_empty() {
                continue;
            }
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
}

fn is_daily_note(bob_dir: &Path, path: &Path, day_file: &Path) -> bool {
    if path == day_file {
        return true;
    }
    let Ok(relative) = path.strip_prefix(bob_dir) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    crate::native::stats::index::canonical_daily_date(&relative).is_some()
}

fn component_entry_counts(contents: &str) -> BTreeMap<String, u32> {
    let mut counts = BTreeMap::new();
    let scan = capture_pomodoros::scan(contents);
    for entry in &scan.entries {
        let Some(name) = entry.name.as_deref() else {
            continue;
        };
        for component in plan_budget::split_components(name) {
            let key = plan_budget::normalize_component(&component);
            if key.is_empty() {
                continue;
            }
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
}

fn authored_components_in_order(contents: &str) -> Vec<(String, String)> {
    let scan = capture_pomodoros::scan(contents);
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in &scan.entries {
        let Some(name) = entry.name.as_deref() else {
            continue;
        };
        for component in plan_budget::split_components(name) {
            let key = plan_budget::normalize_component(&component);
            if key.is_empty() || !seen.insert(key.clone()) {
                continue;
            }
            names.push((key, component));
        }
    }
    names
}

/// Compare today's daily note before and after planning and attach
/// `plan_budget` when the Pomodoros section changed. An invalid plan
/// config skips the budget and pushes one plain string into the
/// existing `warnings`.
pub(super) fn append_plan_budget(
    request: &CaptureRequest,
    batch: &mut PlannedCaptureBatch,
) -> Result<(), CaptureError> {
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let Some(staged) =
        batch.text_files.iter().find(|file| file.target == day_file)
    else {
        return Ok(());
    };
    if pomodoros_section_text(&staged.original_target)
        == pomodoros_section_text(&staged.updated_target)
    {
        return Ok(());
    }
    let config = match config::load_plan_config(&config::config_path()) {
        Ok(config) => config,
        Err(error) => {
            let message = match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            };
            batch
                .warnings
                .push(format!("plan budget unavailable: {message}").into());
            return Ok(());
        }
    };
    let daily_key = day_file
        .strip_prefix(&request.bob_dir)
        .map(|path| path.display().to_string())
        .ok()
        .or_else(|| plan_budget::daily_key_from_path(&day_file));
    let before = plan_budget::compute_for_daily(
        &staged.original_target,
        &config,
        daily_key.as_deref(),
    );
    let after = plan_budget::compute_for_daily(
        &staged.updated_target,
        &config,
        daily_key.as_deref(),
    );
    let links_grew = after.links.count > before.links.count;

    let mut before_keys = std::collections::BTreeSet::new();
    for name in &before.theme_names {
        before_keys.insert(plan_budget::normalize_component(name));
    }
    let added_themes = after
        .theme_names
        .iter()
        .filter(|name| {
            !before_keys.contains(&plan_budget::normalize_component(name))
        })
        .cloned()
        .collect::<Vec<_>>();

    let mut warnings = Vec::new();
    if after.links.over && links_grew {
        warnings.push(PlanBudgetWarning {
            code: plan_budget::LINT_LINK_CAP.to_string(),
            message: link_warning(after.links.count, after.links.cap),
        });
    }

    batch.plan_budget = Some(CapturePlanBudget {
        status: after.status,
        links: PlanBudgetMeter {
            count: after.links.count,
            cap: after.links.cap,
            over: after.links.over,
            before: before.links.count,
        },
        added_themes,
        added_theme_refs: Vec::new(),
        warnings,
    });
    Ok(())
}

/// `today's plan now has 11/10 links; queue it with ^ or defer with
/// $<N>`.
fn link_warning(count: usize, cap: u32) -> String {
    format!(
        "today's plan now has {count}/{cap} links; queue it with ^ or defer \
        with $<N>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;

    #[test]
    fn no_grown_daily_component_skips_theme_catalog() {
        let temp = tempfile::tempdir().expect("temp vault");
        let vault = temp.path().join("vault");
        std::fs::create_dir_all(&vault).expect("vault dir");
        let settings = vault.join(".obsidian/plugins/obsidian-tasks-plugin");
        std::fs::create_dir_all(&settings).expect("tasks settings dir");
        std::fs::write(
            settings.join("data.json"),
            r##"{"globalFilter":"#task","statusSettings":{"coreStatuses":[{"symbol":" ","name":"Todo","type":"TODO"},{"symbol":"x","name":"Done","type":"DONE"}],"customStatuses":[]}}"##,
        )
        .expect("tasks settings");
        let day_file = vault.join("20261024.md");
        std::fs::write(&day_file, "## Pomodoros\n").expect("day file");
        std::fs::write(vault.join("sase.md"), "- [ ] #task Work ^foo\n")
            .expect("note");
        let config = temp.path().join("config.yml");
        std::fs::write(
            &config,
            "plan:\nthemes:\n  BUILD: yellow\n  PLAN: blue\n",
        )
        .expect("config");
        let _guard = TestEnvGuard::set(&[
            ("BOB_CONFIG_FILE", Some(config.as_os_str())),
            ("BOB_DAY_FILE", Some(day_file.as_os_str())),
            ("BOB_NOW", Some(std::ffi::OsStr::new("2026-10-24 08:00:00"))),
        ]);
        let request = CaptureRequest {
            bob_dir: vault.clone(),
            dry_run: true,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route: None,
            forced_section: None,
            forced_sub_bullet_target: None,
            forced_drawer: None,
            inbox_routes: Vec::new(),
            move_in: None,
            no_clip: true,
            no_ref: true,
            pull_in: None,
            refuse_override: false,
            require_inbox_route_choice: false,
            require_pull_in_choice: false,
            template_origin: None,
            raw_text: "hello @sase".to_string(),
            selection: crate::native::task_complete::SuccessorSelection::None,
        };
        let mut batch =
            plan_capture_batch(&request, None).expect("plan simple capture");
        reset_theme_catalog_loads();
        append_unconfigured_theme_warnings(&request, &mut batch);
        assert_eq!(
            theme_catalog_loads(),
            0,
            "no grown daily-note component must not load the theme catalog"
        );
    }

    #[test]
    fn destination_roles_follow_the_contract() {
        assert_eq!(destination_role(true, None, None), "created");
        assert_eq!(destination_role(true, Some("x"), None), "created");
        assert_eq!(destination_role(false, Some("x"), None), "named");
        assert_eq!(
            destination_role(false, Some("x"), Some("0900-0930")),
            "named"
        );
        assert_eq!(destination_role(false, None, Some("0900-0930")), "current");
        assert_eq!(destination_role(false, None, None), "next_up");
    }
}

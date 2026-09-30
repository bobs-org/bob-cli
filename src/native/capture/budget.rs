//! Plan-budget reporting and strict mode for `bob capture`.
//!
//! After a batch is planned (dry-run and real runs share the planner),
//! today's daily note is compared before and after. When its Pomodoros
//! section changed, a top-level `plan_budget` joins the result with
//! before/after meters; warnings fire only when a meter grows past its
//! cap. With `plan.strict: true` the whole batch is refused, atomically,
//! when a non-start item creates a new named Pomodoro past the theme cap.
use super::*;

fn pomodoros_section_text(contents: &str) -> Option<String> {
    let lines: Vec<&str> = contents.lines().collect();
    let range = pomodoro::pomodoros_section_range(&lines)?;
    Some(lines[range].join("\n"))
}

/// One cap warning inside [`CapturePlanBudget`]: only the theme and
/// link caps ever fire here, and only while growing past the cap.
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
/// batch changed today's Pomodoros section. It is not per item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CapturePlanBudget {
    pub(super) status: plan_budget::PlanStatus,
    pub(super) themes: PlanBudgetMeter,
    pub(super) links: PlanBudgetMeter,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) added_themes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<PlanBudgetWarning>,
}

/// Compare today's daily note before and after planning, attach
/// `plan_budget` when the Pomodoros section changed, and enforce
/// strict mode. An invalid plan config skips the budget and pushes
/// one plain string into the existing `warnings`.
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
                .push(format!("plan budget unavailable: {message}"));
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
    let themes_grew = after.themes.count > before.themes.count;
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
    if after.themes.over && themes_grew {
        warnings.push(PlanBudgetWarning {
            code: plan_budget::LINT_THEME_CAP.to_string(),
            message: theme_warning(
                after.themes.count,
                after.themes.cap,
                &added_themes,
            ),
        });
    }
    if after.links.over && links_grew {
        warnings.push(PlanBudgetWarning {
            code: plan_budget::LINT_LINK_CAP.to_string(),
            message: link_warning(after.links.count, after.links.cap),
        });
    }

    if config.strict()
        && after.themes.over
        && themes_grew
        && batch.items.iter().any(|item| {
            item.result.creates_pomodoro == Some(true)
                && item.result.pomodoro_start.is_none()
        })
    {
        return Err(CaptureError::strict(theme_refusal(
            before.theme_names.as_slice(),
            after.themes.count,
            after.themes.cap,
            &added_themes,
        )));
    }

    batch.plan_budget = Some(CapturePlanBudget {
        status: after.status,
        themes: PlanBudgetMeter {
            count: after.themes.count,
            cap: after.themes.cap,
            over: after.themes.over,
            before: before.themes.count,
        },
        links: PlanBudgetMeter {
            count: after.links.count,
            cap: after.links.cap,
            over: after.links.over,
            before: before.links.count,
        },
        added_themes,
        warnings,
    });
    Ok(())
}

/// `today's plan now has 4/3 themes (adds BOB); queue it with ^ or
/// defer with p:<N>`.
fn theme_warning(count: usize, cap: u32, added: &[String]) -> String {
    let adds = if added.is_empty() {
        String::new()
    } else {
        format!(" (adds {})", added.join(", "))
    };
    format!(
        "today's plan now has {count}/{cap} themes{adds}; queue it with ^ \
        or defer with p:<N>"
    )
}

/// `today's plan now has 11/10 links; queue it with ^ or defer with
/// p:<N>`.
fn link_warning(count: usize, cap: u32) -> String {
    format!(
        "today's plan now has {count}/{cap} links; queue it with ^ or defer \
        with p:<N>"
    )
}

/// Strict-mode refusal: names the current (pre-batch) themes and the
/// added ones, with the same `^` / `p:<N>` hint.
fn theme_refusal(
    current: &[String],
    count: usize,
    cap: u32,
    added: &[String],
) -> String {
    let adds = if added.is_empty() {
        String::new()
    } else {
        format!(" (adds {})", added.join(", "))
    };
    let current = if current.is_empty() {
        "none".to_string()
    } else {
        current.join(", ")
    };
    format!(
        "refusing capture: today's plan would grow to {count}/{cap} \
        themes{adds}; current themes: {current}; queue it with ^ or defer \
        with p:<N>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_warning_names_added_themes_with_hint() {
        assert_eq!(
            theme_warning(4, 3, &["BOB".to_string()]),
            "today's plan now has 4/3 themes (adds BOB); queue it with ^ \
            or defer with p:<N>"
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

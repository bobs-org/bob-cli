//! Shared, planner-aware copy for moving a parent task into a new project.
//!
//! The picker asks what each move would do, while `moved_tasks[].detail`
//! describes what a completed move actually did. Both use the same suffix
//! rules so a Ready task is the only unblocked lane raised by a new Task Link,
//! and blocked tasks never receive a link promise the guard would refuse.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkGuard {
    /// The link gesture can complete without consuming any blocker.
    Link,
    /// The task's own single future date is retired by the link gesture.
    PullForward { date: String },
    /// Capture drops the task's own open prerequisite links, then links it.
    DropPrerequisites {
        count: usize,
        pulled_forward: Option<String>,
    },
    /// The guard consumes the gesture and leaves the task unlinked.
    Consume { reason: String },
    /// Status-only copy: capture-complete cannot inspect the vault-wide graph.
    StatusOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PredictionInput {
    pub(crate) sigil: char,
    pub(crate) status: char,
    pub(crate) destination: String,
    pub(crate) pomodoro_name: Option<String>,
    pub(crate) today_link: Option<String>,
    pub(crate) own_scheduled: Option<String>,
    pub(crate) project_starts: Option<String>,
    pub(crate) guard: LinkGuard,
    /// A choice row can offer the ID that will be minted if selected.
    pub(crate) id_to_add: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prediction {
    pub(crate) consequence: String,
    pub(crate) adds_link: bool,
}

/// Facts observed after a move and its optional link gesture have been
/// planned. `describe` returns the suffix stored in `moved_tasks[].detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) sigil: char,
    pub(crate) previous_status: char,
    pub(crate) status: char,
    pub(crate) existing_today_link: Option<String>,
    pub(crate) added_today_link: Option<String>,
    pub(crate) own_scheduled: Option<String>,
    pub(crate) project_starts: Option<String>,
    pub(crate) guard: LinkGuard,
    pub(crate) id_added: Option<String>,
}

/// Predict the user-facing consequence for one choice.
pub(crate) fn predict(input: &PredictionInput) -> Prediction {
    let components = consequence_components(
        input.sigil,
        input.status,
        input.today_link.as_deref(),
        input.own_scheduled.as_deref(),
        input.project_starts.as_deref(),
        &input.guard,
        input.pomodoro_name.as_deref(),
    );
    let mut consequence = if input.sigil == '^' {
        format!("Moves into {}", input.destination)
    } else {
        "Moves in".to_string()
    };
    for component in components.copy {
        let component = match component {
            CopyComponent::ExistingLink(name) if input.sigil == '^' => {
                format!("its {} link follows", name.to_uppercase())
            }
            CopyComponent::ExistingLink(name) => {
                format!("keeps its {} link", name.to_uppercase())
            }
            component => component.as_text(),
        };
        consequence.push_str(" · ");
        consequence.push_str(&component);
    }
    if let Some(id) = input.id_to_add.as_deref() {
        consequence.push_str(" · adds ^");
        consequence.push_str(id);
    }
    Prediction {
        consequence,
        adds_link: components.adds_link,
    }
}

/// Describe an actual outcome with the same suffix vocabulary as `predict`.
pub(crate) fn describe(outcome: &Outcome) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(name) = outcome.existing_today_link.as_deref() {
        if outcome.sigil == '^' {
            // Preserve the established terminal/JSON detail wording. The
            // picker adds the grammatical "its" after the move head.
            parts.push(format!("{} link follows", name.to_uppercase()));
        } else {
            parts.push(format!("keeps its {} link", name.to_uppercase()));
        }
    } else if outcome.sigil == '^' {
        if outcome.status == '?' {
            parts.push(match outcome.own_scheduled.as_deref() {
                Some(date) => format!("stays Blocked until {date}"),
                None => "stays Blocked".to_string(),
            });
        }
    } else if let Some(date) = outcome.project_starts.as_deref() {
        parts.push(format!("stays unlinked until the project starts {date}"));
    } else {
        match &outcome.guard {
            LinkGuard::Consume { reason } => {
                parts.push(format!("stays unlinked: {reason}"));
            }
            LinkGuard::DropPrerequisites {
                count,
                pulled_forward,
            } if outcome.added_today_link.is_some() => {
                parts.extend(link_effect_texts(
                    outcome.previous_status,
                    *count,
                    pulled_forward.as_deref(),
                    outcome.added_today_link.as_deref(),
                ));
            }
            LinkGuard::PullForward { date }
                if outcome.added_today_link.is_some() =>
            {
                parts.extend(link_effect_texts(
                    outcome.previous_status,
                    0,
                    Some(date),
                    outcome.added_today_link.as_deref(),
                ));
            }
            LinkGuard::Link if outcome.added_today_link.is_some() => {
                parts.extend(link_effect_texts(
                    outcome.previous_status,
                    0,
                    None,
                    outcome.added_today_link.as_deref(),
                ));
            }
            LinkGuard::StatusOnly if outcome.status == '?' => {
                parts.push("links only if Bob can unblock it".to_string());
            }
            _ => {}
        }
    }
    if let Some(id) = outcome.id_added.as_deref() {
        parts.push(format!("adds ^{id}"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

enum CopyComponent {
    Text(String),
    ExistingLink(String),
}

impl CopyComponent {
    fn as_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::ExistingLink(name) => {
                format!("{} link follows", name.to_uppercase())
            }
        }
    }
}

struct Components {
    copy: Vec<CopyComponent>,
    adds_link: bool,
}

fn consequence_components(
    sigil: char,
    status: char,
    today_link: Option<&str>,
    own_scheduled: Option<&str>,
    project_starts: Option<&str>,
    guard: &LinkGuard,
    pomodoro_name: Option<&str>,
) -> Components {
    let mut copy = Vec::new();
    let mut adds_link = false;
    if sigil == '^' {
        if let Some(name) = today_link {
            copy.push(CopyComponent::ExistingLink(name.to_string()));
        } else if status == '?' {
            copy.push(CopyComponent::Text(match own_scheduled {
                Some(date) => format!("stays Blocked until {date}"),
                None => "stays Blocked".to_string(),
            }));
        }
        return Components { copy, adds_link };
    }

    if let Some(name) = today_link {
        copy.push(CopyComponent::ExistingLink(name.to_string()));
        return Components { copy, adds_link };
    }
    if let Some(date) = project_starts {
        copy.push(CopyComponent::Text(format!(
            "stays unlinked until the project starts {date}"
        )));
        return Components { copy, adds_link };
    }

    match guard {
        LinkGuard::StatusOnly if status == '?' => {
            copy.push(CopyComponent::Text(
                "links only if Bob can unblock it".to_string(),
            ));
        }
        LinkGuard::Consume { reason } => {
            copy.push(CopyComponent::Text(format!("stays unlinked: {reason}")));
        }
        LinkGuard::StatusOnly => {
            append_link_effects(&mut copy, status, 0, None, pomodoro_name);
            adds_link = true;
        }
        LinkGuard::Link => {
            append_link_effects(&mut copy, status, 0, None, pomodoro_name);
            adds_link = true;
        }
        LinkGuard::PullForward { date } => {
            append_link_effects(
                &mut copy,
                status,
                0,
                Some(date),
                pomodoro_name,
            );
            adds_link = true;
        }
        LinkGuard::DropPrerequisites {
            count,
            pulled_forward,
        } => {
            append_link_effects(
                &mut copy,
                status,
                *count,
                pulled_forward.as_deref(),
                pomodoro_name,
            );
            adds_link = true;
        }
    }
    Components { copy, adds_link }
}

fn append_link_effects(
    parts: &mut Vec<CopyComponent>,
    previous_status: char,
    dropped_prerequisites: usize,
    pulled_forward: Option<&str>,
    pomodoro_name: Option<&str>,
) {
    parts.extend(
        link_effect_texts(
            previous_status,
            dropped_prerequisites,
            pulled_forward,
            pomodoro_name,
        )
        .into_iter()
        .map(CopyComponent::Text),
    );
}

fn link_effect_texts(
    previous_status: char,
    dropped_prerequisites: usize,
    pulled_forward: Option<&str>,
    pomodoro_name: Option<&str>,
) -> Vec<String> {
    let mut parts = Vec::new();
    if dropped_prerequisites > 0 {
        let noun = if dropped_prerequisites == 1 {
            "prerequisite"
        } else {
            "prerequisites"
        };
        parts.push(format!("drops {dropped_prerequisites} {noun}"));
    }
    if let Some(date) = pulled_forward {
        parts.push(format!("pulls {date} forward"));
    }
    if previous_status == ' '
        || (previous_status == '?'
            && (dropped_prerequisites > 0 || pulled_forward.is_some()))
    {
        parts.push("becomes Next".to_string());
    }
    let destination = pomodoro_name
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!("links under {}", name.to_uppercase()))
        .unwrap_or_else(|| "links into today's plan".to_string());
    parts.push(destination);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(sigil: char, status: char, guard: LinkGuard) -> PredictionInput {
        PredictionInput {
            sigil,
            status,
            destination: "cash_exit.md".to_string(),
            pomodoro_name: Some("admin".to_string()),
            today_link: None,
            own_scheduled: None,
            project_starts: None,
            guard,
            id_to_add: None,
        }
    }

    #[test]
    fn predicts_sticky_lanes_and_guarded_blocked_moves() {
        assert_eq!(
            predict(&input('^', '/', LinkGuard::Link)).consequence,
            "Moves into cash_exit.md"
        );
        assert_eq!(
            predict(&input(':', '/', LinkGuard::Link)).consequence,
            "Moves in · links under ADMIN"
        );
        assert_eq!(
            predict(&input(':', ' ', LinkGuard::Link)).consequence,
            "Moves in · becomes Next · links under ADMIN"
        );
        let blocked = predict(&input(':', '?', LinkGuard::StatusOnly));
        assert_eq!(
            blocked.consequence,
            "Moves in · links only if Bob can unblock it"
        );
        assert!(!blocked.adds_link);
    }

    #[test]
    fn predicts_dependency_drop_pull_forward_and_existing_link() {
        assert_eq!(
            predict(&input(
                ':',
                '?',
                LinkGuard::DropPrerequisites {
                    count: 1,
                    pulled_forward: None,
                },
            ))
            .consequence,
            "Moves in · drops 1 prerequisite · becomes Next · links under ADMIN"
        );
        assert_eq!(
            predict(&input(
                ':',
                '?',
                LinkGuard::PullForward {
                    date: "Nov 9".to_string(),
                },
            ))
            .consequence,
            "Moves in · pulls Nov 9 forward · becomes Next · links under ADMIN"
        );
        let mut existing = input(':', '*', LinkGuard::Link);
        existing.today_link = Some("WORK".to_string());
        let prediction = predict(&existing);
        assert_eq!(prediction.consequence, "Moves in · keeps its WORK link");
        assert!(!prediction.adds_link);
    }

    #[test]
    fn actual_details_share_prediction_copy_and_preserve_followed_link_text() {
        let outcome = Outcome {
            sigil: ':',
            previous_status: '?',
            status: '*',
            existing_today_link: None,
            added_today_link: Some("ADMIN".to_string()),
            own_scheduled: None,
            project_starts: None,
            guard: LinkGuard::DropPrerequisites {
                count: 1,
                pulled_forward: None,
            },
            id_added: None,
        };
        assert_eq!(
            describe(&outcome).as_deref(),
            Some("drops 1 prerequisite · becomes Next · links under ADMIN")
        );
        let mut unnamed_destination = outcome.clone();
        unnamed_destination.added_today_link = Some(String::new());
        assert_eq!(
            describe(&unnamed_destination).as_deref(),
            Some(
                "drops 1 prerequisite · becomes Next · links into today's plan"
            )
        );
        let followed = Outcome {
            sigil: '^',
            previous_status: '*',
            status: '*',
            existing_today_link: Some("WORK".to_string()),
            added_today_link: None,
            own_scheduled: None,
            project_starts: None,
            guard: LinkGuard::Link,
            id_added: None,
        };
        assert_eq!(describe(&followed).as_deref(), Some("WORK link follows"));
    }
}

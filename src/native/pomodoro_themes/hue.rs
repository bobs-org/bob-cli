use std::collections::{BTreeMap, BTreeSet};

use super::{Hue, HueSource};

/// PT5 revised: config color, then pin, then exempt, then the unconfigured
/// mute (only when a `themes:` vocabulary is configured and the key is not
/// in it and is not an inventory label), then top-level area stem, then
/// name hash. `config_color` is `Some` only when the vocabulary sets a
/// palette slot for the key; `auto`/null falls through. Inventory labels
/// never take the unconfigured step.
pub(crate) fn resolve_hue(
    key: &str,
    pin: Option<Hue>,
    exempt: &BTreeSet<String>,
    areas: &BTreeMap<String, Hue>,
    config_color: Option<Hue>,
    unconfigured: bool,
) -> (Option<Hue>, HueSource) {
    if let Some(hue) = config_color {
        return (Some(hue), HueSource::Config);
    }
    if let Some(hue) = pin {
        return (Some(hue), HueSource::Pin);
    }
    if exempt.contains(key) {
        return (None, HueSource::Exempt);
    }
    if unconfigured {
        return (None, HueSource::Unconfigured);
    }
    if let Some(hue) = areas.get(key) {
        return (Some(*hue), HueSource::Area);
    }
    (
        Some(crate::native::pomodoro_homes::fallback_hue(key)),
        HueSource::Hash,
    )
}

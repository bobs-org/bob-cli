use std::collections::{BTreeMap, BTreeSet};

use super::{Hue, HueSource};

/// PT5: pin, then exempt, then top-level area stem, then name hash.
pub(crate) fn resolve_hue(
    key: &str,
    pin: Option<Hue>,
    exempt: &BTreeSet<String>,
    areas: &BTreeMap<String, Hue>,
) -> (Option<Hue>, HueSource) {
    if let Some(hue) = pin {
        return (Some(hue), HueSource::Pin);
    }
    if exempt.contains(key) {
        return (None, HueSource::Exempt);
    }
    if let Some(hue) = areas.get(key) {
        return (Some(*hue), HueSource::Area);
    }
    (
        Some(crate::native::pomodoro_homes::fallback_hue(key)),
        HueSource::Hash,
    )
}

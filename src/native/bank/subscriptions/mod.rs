//! Typed subscription registry and deterministic description matching.

pub(crate) mod matching;
pub(crate) mod registry;

pub(crate) use matching::{resolve_match, SubscriptionMatch};
pub(crate) use registry::{
    parse_subscription_snapshot, InvalidNeedle, NeedleInvalidReason,
    SubscriptionAlbum, SubscriptionAvailability, SubscriptionCadence,
    SubscriptionDiagnostic, SubscriptionPricing, SubscriptionRecord,
    SubscriptionRegistry, SubscriptionStatus,
};

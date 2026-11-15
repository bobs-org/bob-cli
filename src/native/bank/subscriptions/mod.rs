//! Typed subscription registry, matching, attribution, and ledger.

pub(crate) mod attribution;
pub(crate) mod cadence;
pub(crate) mod coverage;
pub(crate) mod ledger;
pub(crate) mod loader;
pub(crate) mod matching;
pub(crate) mod projection;
pub(crate) mod registry;

#[allow(unused_imports)]
pub(crate) use matching::{resolve_match, SubscriptionMatch};
#[allow(unused_imports)]
pub(crate) use registry::{
    parse_subscription_snapshot, InvalidNeedle, NeedleInvalidReason,
    SubscriptionAlbum, SubscriptionAvailability, SubscriptionCadence,
    SubscriptionDiagnostic, SubscriptionPricing, SubscriptionRecord,
    SubscriptionRegistry, SubscriptionStatus,
};

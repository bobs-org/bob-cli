//! URL routing for the reading queue: intent classification,
//! routing policy, and offline library verdicts.
//!
//! Every path (capture, Keep pull, previews) shares this classifier
//! and these verdicts. Nothing here touches the network.

pub(crate) mod intent;
pub(crate) mod policy;
pub(crate) mod verdict;

pub(crate) use intent::{
    classify_token, display_url, is_url_list_line, RouteHint, UrlIntent,
};
pub(crate) use policy::{
    normalize_exclude_host, RoutingEntry, UrlRoutingPolicy,
    DEFAULT_EXCLUDE_HOSTS,
};
pub(crate) use verdict::{library_verdicts, LibraryVerdict, Verdict};

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test over the module's re-exported phase API: the
    /// classifier, display form, list detection, policy defaults and
    /// normalization, and the offline verdict entry point.
    #[test]
    fn phase_api_hangs_together() {
        let intent =
            classify_token("https://example.com/post").expect("classify");
        assert_eq!(intent.route_hint, RouteHint::Article);
        assert_eq!(display_url(&intent.cleaned), "example.com/post");
        assert!(is_url_list_line("https://example.com/post"));
        assert_eq!(
            normalize_exclude_host("HTTPS://WWW.Example.COM./"),
            Some("example.com".to_string()),
        );
        assert!(DEFAULT_EXCLUDE_HOSTS.contains(&"youtube.com"));
        let policy = UrlRoutingPolicy::default();
        assert!(policy.admits(&intent, RoutingEntry::Capture));
        assert_eq!(Verdict::NotFound.as_str(), "not_found");
        let verdicts = library_verdicts(
            std::path::Path::new("/definitely/missing/vault"),
            &[&intent],
        );
        assert_eq!(verdicts.len(), 1);
        let _: Option<LibraryVerdict> = None;
    }
}

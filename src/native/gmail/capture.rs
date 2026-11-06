//! Offline capture metadata and saved-thread substitutions. No gog calls.

use std::path::Path;

use serde::Serialize;

use super::{
    config::GmailConfig,
    ids::{self, GmailRef, GmailUrlMatch, GmailVerdict, GmailWrapper},
    note::{index::MailIndex, label},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Preview {
    pub(crate) url: String,
    pub(crate) verdict: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) date_hint: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SpanMetadata {
    pub(crate) verdict: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) date_hint: Option<String>,
}

#[derive(Default)]
pub(crate) struct OfflineContext {
    account: Option<String>,
    index: MailIndex,
}

impl OfflineContext {
    pub(crate) fn load(vault: &Path) -> Self {
        let Ok(config) = GmailConfig::resolve() else {
            return Self::default();
        };
        Self {
            account: config
                .account()
                .map(str::to_ascii_lowercase)
                .or_else(super::worker::login_account),
            index: super::note::index::scan(
                vault,
                Path::new(config.folder()),
                config.link_label(),
            )
            .unwrap_or_default(),
        }
    }

    pub(crate) fn preview(&self, found: &GmailUrlMatch) -> Preview {
        let mut result = Preview {
            url: found.url.clone(),
            verdict: "unresolved",
            thread_id: None,
            note: None,
            link: None,
            date_hint: None,
            message: String::new(),
        };
        match &found.verdict {
            GmailVerdict::Resolvable(GmailRef::Thread { hex }) => {
                result.thread_id = Some(hex.clone());
                result.verdict = "decoded";
                let date = ids::thread_date_hint(hex);
                result.date_hint = date.map(|date| date.to_rfc3339());
                if let Some(entry) = self.account.as_ref().and_then(|account| {
                    self.index.by_identity.get(&(account.clone(), hex.clone()))
                }) {
                    result.verdict = "known";
                    result.note = Some(entry.path.clone());
                    let alias = match &found.wrapper {
                        GmailWrapper::MarkdownLink { label: authored } => {
                            label::alias_safe(authored)
                        }
                        _ => entry.label.clone(),
                    };
                    result.link = Some(format!(
                        "[[{}|{alias}]]",
                        entry.path.trim_end_matches(".md")
                    ));
                    result.message = format!("{} · already saved", entry.label);
                } else {
                    let date = date
                        .map(|date| format!(" · {}", date.format("%b %-d, %Y")))
                        .unwrap_or_default();
                    result.message = format!(
                        "✉️ Gmail thread{date} · will link after capture"
                    );
                }
            }
            GmailVerdict::Resolvable(GmailRef::Message { .. }) => {
                result.verdict = "decoded";
                result.message =
                    "✉️ Gmail message · will link after capture".into();
            }
            GmailVerdict::Resolvable(GmailRef::RfcMessageId { .. }) => {
                result.verdict = "search";
                result.message =
                    "✉️ Gmail message search · will link after capture".into();
            }
            GmailVerdict::Unresolvable { reason }
            | GmailVerdict::Refused { reason } => {
                if matches!(found.verdict, GmailVerdict::Refused { .. }) {
                    result.verdict = "refused";
                }
                result.message = format!("✉️ {reason} · URL kept");
            }
        }
        result
    }

    /// Run only after capture grammar has parsed the user's text. Sender
    /// labels must never become grammar input.
    pub(crate) fn rewrite(&self, text: &mut String) -> Vec<Preview> {
        let matches: Vec<_> = ids::find_gmail_urls(text)
            .into_iter()
            .filter(|found| !super::link::inline_link_destination(text, found))
            .collect();
        let previews: Vec<_> =
            matches.iter().map(|found| self.preview(found)).collect();
        for (found, preview) in matches.iter().zip(&previews).rev() {
            if let Some(link) = &preview.link {
                text.replace_range(found.start..found.end, link);
            }
        }
        previews
    }
}

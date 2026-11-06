//! Public pure renderer: HTML/plain/snippet selection with folded history.
//!
//! This is the assembly point the outer mail-note, gmail-save, and
//! search/peek phases consume. `render_message` is pure: it reads one
//! [`GmailMessage`] plus already-fetched bytes and returns inert Markdown.
//! No network, filesystem, capture parser, subprocess, clock, or global
//! configuration access happens here, and no sender bytes leak through
//! errors, logs, or diagnostics ([`BodyError`] carries only typed reasons
//! and numeric structural locations).
//!
//! Trust boundary, for the outer phases:
//!
//! * Every prose field of [`RenderedBody`] (`markdown`, `quoted`,
//!   `signature`) has passed through the inert serializer last. There is no
//!   path that concatenates an already-sanitized string with unchecked sender
//!   bytes. Mail-note owns the card/callout wrappers; it must place these
//!   fields inside its own renderer-owned structure verbatim, never
//!   re-interpreting them.
//! * `source` tells which candidate won: `Html`, `Plain`, or `Snippet`.
//!   Snippet prose is single-line inert text, safe to embed on one line.
//! * `bulletin` selects the folded newsletter layout (list mail, bulk
//!   precedence, Gmail promotion/update/forum categories). It is derived from
//!   header presence and exact opaque label IDs, never from body content.
//! * `inline_images` are typed descriptors (CID, MIME, size, filename, and an
//!   explicit bytes-or-attachment-ID source), never local paths or embeds.
//!   Only referenced, non-tracking resources are returned, in stable
//!   occurrence order. Gmail-save copies bytes into the vault and mints the
//!   embed; no sender string may become an embed through
//!   `verified_local_image_embed` without passing its writer-path check.
//! * `attachments` keep original IDs in stable MIME order with 1-based
//!   `index` values scoped to this message. The note layer assigns
//!   thread-wide display numbers in document order. Raw filenames stay
//!   untrusted path inputs: display them only through `inert_single_line`,
//!   and let gmail-save own slug/path policy.
//! * `needs_fetch` identifies body parts whose bytes were not available.
//!   Such a message still renders a provisional fallback (plain, then
//!   snippet), and re-rendering the same message with the bytes in
//!   [`FetchedBodies`] resolves those entries without mutating the model. A
//!   missing part never masquerades as a successfully saved empty body.
//! * `DRAFT` messages are never importable: [`importable_messages`] filters
//!   them from threads, and a direct render attempt fails with the typed
//!   [`BodyErrorKind::DraftRefused`] refusal so callers cannot import one by
//!   accident. Label IDs are opaque: `DRAFT`, `CATEGORY_*`, and `Precedence`
//!   matching never lowercases or trims the stored label.
//! * Header/snippet/filename display text is raw sender text until the
//!   consumer neutralizes it. [`inert_metadata`] applies the single-line
//!   helper to one header kind; attachment filenames need the same treatment
//!   at display time.

use super::BodySource;
use super::{
    html::{render_html, HtmlRender},
    inert::inert_single_line,
    mime::collect_message_bodies,
    plain::render_plain,
    Attachment, FetchedBodies, RenderedBody,
};
use crate::native::gmail::model::{
    BodyError, BodyErrorKind, GmailMessage, GmailThread, HeaderKind,
};

/// Render one message into inert Markdown with folded history.
///
/// Selection, in order: usable HTML wins; otherwise the plain candidate;
/// otherwise the inert snippet. A missing fetchable part keeps its
/// `needs_fetch` entry even while a provisional fallback is shown.
/// Returns [`BodyErrorKind::DraftRefused`] for `DRAFT` messages.
pub(crate) fn render_message(
    message: &GmailMessage,
    fetched: &FetchedBodies,
) -> Result<RenderedBody, BodyError> {
    if is_draft(message) {
        return Err(BodyError::new(BodyErrorKind::DraftRefused, Vec::new()));
    }
    let inventory = collect_message_bodies(message, fetched)?;
    let bulletin = is_bulletin(message);
    let plain_text = inventory
        .plain_candidates
        .last()
        .map(|candidate| candidate.text.as_str());

    if let Some(candidate) = inventory.html_candidates.last() {
        let html =
            render_html(&candidate.text, &inventory.inline_images, bulletin);
        if html_is_useful(&html, plain_text) {
            return Ok(RenderedBody {
                markdown: html.markdown,
                quoted: html.quoted,
                signature: html.signature,
                source: BodySource::Html,
                bulletin,
                inline_images: html.inline_images,
                attachments: index_attachments(inventory.attachments),
                needs_fetch: inventory.needs_fetch,
            });
        }
    }

    if let Some(text) = plain_text {
        let plain = render_plain(text);
        if !plain.markdown.is_empty()
            || plain.quoted.is_some()
            || plain.signature.is_some()
        {
            return Ok(RenderedBody {
                markdown: plain.markdown,
                quoted: plain.quoted,
                signature: plain.signature,
                source: BodySource::Plain,
                bulletin,
                inline_images: Vec::new(),
                attachments: index_attachments(inventory.attachments),
                needs_fetch: inventory.needs_fetch,
            });
        }
    }

    Ok(RenderedBody {
        markdown: message
            .snippet
            .as_deref()
            .map(inert_single_line)
            .unwrap_or_default(),
        quoted: None,
        signature: None,
        source: BodySource::Snippet,
        bulletin,
        inline_images: Vec::new(),
        attachments: index_attachments(inventory.attachments),
        needs_fetch: inventory.needs_fetch,
    })
}

/// True when the message is bulk or list mail: a `List-Unsubscribe` or
/// `List-Id` header is present, `Precedence` is `bulk`, or an exact Gmail
/// category label (`CATEGORY_PROMOTIONS`, `CATEGORY_UPDATES`,
/// `CATEGORY_FORUMS`) is attached. Opaque label IDs are matched exactly and
/// never lowercased.
pub(crate) fn is_bulletin(message: &GmailMessage) -> bool {
    message
        .header_values_of(HeaderKind::ListUnsubscribe)
        .next()
        .is_some()
        || message
            .header_values_of(HeaderKind::ListId)
            .next()
            .is_some()
        || message
            .header_values_of(HeaderKind::Precedence)
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("bulk"))
        || message.label_ids.iter().any(|label| {
            matches!(
                label.as_str(),
                "CATEGORY_PROMOTIONS" | "CATEGORY_UPDATES" | "CATEGORY_FORUMS"
            )
        })
}

/// True when the message carries the exact opaque `DRAFT` label.
pub(crate) fn is_draft(message: &GmailMessage) -> bool {
    message.label_ids.iter().any(|label| label == "DRAFT")
}

/// The messages of a thread that may be imported, in thread order. `DRAFT`
/// messages are excluded so save phases never import one by iterating.
pub(crate) fn importable_messages(
    thread: &GmailThread,
) -> impl Iterator<Item = &GmailMessage> {
    thread.messages.iter().filter(|message| !is_draft(message))
}

/// Format one header kind for safe display: every value passes through the
/// inert single-line helper exactly once, joined with `; `. Empty when the
/// header is absent. Filesystem slug/path policy stays with the save phase.
pub(crate) fn inert_metadata(
    message: &GmailMessage,
    kind: HeaderKind,
) -> String {
    message
        .header_values_of(kind)
        .map(inert_single_line)
        .collect::<Vec<_>>()
        .join("; ")
}

/// HTML wins when it is non-empty and either carries renderer-owned structure
/// (links, tables, referenced images) or says something the plain candidate
/// does not. Both sides are compared after inert rendering so escaping cannot
/// manufacture a difference. A bare tag wrapper around identical text falls
/// back to plain; sanitization that leaves no useful HTML does the same.
fn html_is_useful(html: &HtmlRender, plain_text: Option<&str>) -> bool {
    if html.markdown.is_empty() {
        return false;
    }
    if !html.inline_images.is_empty()
        || html.markdown.contains("](")
        || html.markdown.contains('|')
    {
        return true;
    }
    match plain_text {
        None => true,
        Some(plain) => {
            normalize_visible(&html.markdown)
                != normalize_visible(&render_plain(plain).markdown)
        }
    }
}

/// Lowercase alphanumeric skeleton for the HTML/plain equivalence check.
/// Escaping inserts punctuation but never letters, so equal visible text
/// compares equal across the two render paths.
fn normalize_visible(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Number attachment descriptors 1-based in stable MIME order.
fn index_attachments(attachments: Vec<Attachment>) -> Vec<Attachment> {
    attachments
        .into_iter()
        .enumerate()
        .map(|(position, mut attachment)| {
            attachment.index = position.saturating_add(1) as u32;
            attachment
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::collect_done::block_ids_in_markdown;
    use crate::native::gmail::model::{Header, MimeBody, MimePart};
    use crate::native::task_dependencies::raw_wikilink_spans;
    use crate::native::task_fields::all_inline_fields;
    use crate::native::task_status_hooks::{
        parse_tasks, TaskStatusType, TasksSettings,
    };
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use std::collections::{BTreeMap, BTreeSet};

    fn encode(text: &str) -> String {
        URL_SAFE_NO_PAD.encode(text.as_bytes())
    }

    fn header(name: &str, value: &str) -> Header {
        Header {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    fn text_part(mime: &str, text: &str) -> MimePart {
        MimePart {
            part_id: None,
            mime_type: mime.to_string(),
            filename: None,
            headers: vec![header(
                "Content-Type",
                &format!("{mime}; charset=utf-8"),
            )],
            body: Some(MimeBody {
                size: Some(text.len() as u64),
                data: Some(encode(text)),
                attachment_id: None,
            }),
            parts: Vec::new(),
        }
    }

    fn wrapped_part(children: Vec<MimePart>, mime: &str) -> MimePart {
        MimePart {
            part_id: None,
            mime_type: mime.to_string(),
            filename: None,
            headers: Vec::new(),
            body: None,
            parts: children,
        }
    }

    fn attachment_part(filename: &str, attachment_id: &str) -> MimePart {
        MimePart {
            part_id: Some("att".to_string()),
            mime_type: "application/pdf".to_string(),
            filename: Some(filename.to_string()),
            headers: vec![header(
                "Content-Disposition",
                "attachment; filename=\"doc.pdf\"",
            )],
            body: Some(MimeBody {
                size: Some(1024),
                data: None,
                attachment_id: Some(attachment_id.to_string()),
            }),
            parts: Vec::new(),
        }
    }

    fn fetchable_part(mime: &str, attachment_id: &str) -> MimePart {
        MimePart {
            part_id: Some("body-part".to_string()),
            mime_type: mime.to_string(),
            filename: None,
            headers: Vec::new(),
            body: Some(MimeBody {
                size: Some(4096),
                data: None,
                attachment_id: Some(attachment_id.to_string()),
            }),
            parts: Vec::new(),
        }
    }

    fn test_message(payload: Option<MimePart>) -> GmailMessage {
        GmailMessage {
            id: "m1".to_string(),
            thread_id: "t".to_string(),
            label_ids: vec!["INBOX".to_string()],
            internal_date: Some("1762444800000".to_string()),
            snippet: None,
            headers: vec![
                header("From", "Pat <pat@example.test>"),
                header("Subject", "Hello"),
            ],
            payload,
        }
    }

    fn label(message: &mut GmailMessage, label: &str) {
        message.label_ids.push(label.to_string());
    }

    fn with_header(
        mut message: GmailMessage,
        name: &str,
        value: &str,
    ) -> GmailMessage {
        if let Some(payload) = message.payload.as_mut() {
            payload.headers.push(header(name, value));
        }
        message.headers.push(header(name, value));
        message
    }

    fn render(message: &GmailMessage, fetched: &FetchedBodies) -> RenderedBody {
        render_message(message, fetched).expect("fixture must render")
    }

    /// Independent observation: Bob's own scanners must find nothing
    /// actionable in the joined prose fields.
    fn assert_inert_doc(doc: &str) {
        let settings = TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        };
        assert!(
            parse_tasks(doc, &settings).is_empty(),
            "tasks in rendered body:\n{doc}"
        );
        for line in doc.split('\n') {
            assert!(
                all_inline_fields(line).is_empty(),
                "inline field in {line:?} of:\n{doc}"
            );
            assert!(
                raw_wikilink_spans(line).is_empty(),
                "wikilink in {line:?} of:\n{doc}"
            );
        }
        assert!(
            block_ids_in_markdown(doc).is_empty(),
            "block ids in rendered body:\n{doc}"
        );
        for line in doc.split('\n') {
            for forbidden in ["<%", "<!--", "![", "[["] {
                assert!(
                    !line.contains(forbidden),
                    "forbidden {forbidden:?} in {line:?} of:\n{doc}"
                );
            }
        }
    }

    fn joined(body: &RenderedBody) -> String {
        let mut doc = body.markdown.clone();
        if let Some(quoted) = body.quoted.as_deref() {
            doc.push_str("\n\n");
            doc.push_str(quoted);
        }
        if let Some(signature) = body.signature.as_deref() {
            doc.push_str("\n\n");
            doc.push_str(signature);
        }
        doc
    }

    #[test]
    fn drafts_are_refused_and_excluded_from_import() {
        let mut draft = test_message(Some(text_part("text/plain", "secret")));
        label(&mut draft, "DRAFT");
        let error = render_message(&draft, &FetchedBodies::default())
            .err()
            .expect("draft must be refused");
        assert_eq!(error.kind, BodyErrorKind::DraftRefused);
        assert!(!error.to_string().contains("secret"));

        let mut thread = GmailThread {
            id: "t".to_string(),
            history_id: None,
            messages: vec![
                draft,
                test_message(Some(text_part("text/plain", "real"))),
            ],
        };
        thread.messages[0].label_ids.retain(|id| id != "INBOX");
        let importable: Vec<_> = importable_messages(&thread)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(importable, vec!["m1"]);
    }

    #[test]
    fn bulletin_detection_uses_headers_and_exact_labels() {
        let base = test_message(Some(text_part("text/plain", "hi")));
        assert!(!is_bulletin(&base));

        let listed =
            with_header(test_message(None), "List-Unsubscribe", "<mailto:x>");
        assert!(is_bulletin(&listed));
        let listed_id = with_header(test_message(None), "List-ID", "news.x");
        assert!(is_bulletin(&listed_id));
        let bulk = with_header(test_message(None), "Precedence", "Bulk");
        assert!(is_bulletin(&bulk));
        let not_bulk = with_header(test_message(None), "Precedence", "list");
        assert!(!is_bulletin(&not_bulk));

        let mut promo = test_message(None);
        label(&mut promo, "CATEGORY_PROMOTIONS");
        assert!(is_bulletin(&promo));
        let mut updates = test_message(None);
        label(&mut updates, "CATEGORY_UPDATES");
        assert!(is_bulletin(&updates));
        let mut forums = test_message(None);
        label(&mut forums, "CATEGORY_FORUMS");
        assert!(is_bulletin(&forums));
        // Opaque label IDs are never lowercased.
        let mut lower = test_message(None);
        label(&mut lower, "category_promotions");
        assert!(!is_bulletin(&lower));
    }

    #[test]
    fn html_only_message_renders_html_source() {
        let body = render(
            &test_message(Some(text_part(
                "text/html",
                "<p>Hello <b>world</b></p>",
            ))),
            &FetchedBodies::default(),
        );
        assert_eq!(body.source, BodySource::Html);
        assert!(!body.bulletin);
        assert!(body.markdown.contains("Hello"));
        assert!(body.needs_fetch.is_empty());
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn trivial_html_wrapper_falls_back_to_plain() {
        let message = test_message(Some(wrapped_part(
            vec![
                text_part("text/plain", "Same text here"),
                text_part("text/html", "<div><p>Same text here</p></div>"),
            ],
            "multipart/alternative",
        )));
        let body = render(&message, &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Plain);
        assert_eq!(body.markdown, "Same text here");
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn html_with_links_tables_or_images_wins_over_matching_plain() {
        let linked = test_message(Some(wrapped_part(
            vec![
                text_part("text/plain", "Read the report"),
                text_part(
                    "text/html",
                    "<p>Read the <a href=\"https://example.test/r\">report</a></p>",
                ),
            ],
            "multipart/alternative",
        )));
        let body = render(&linked, &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Html);
        assert!(body.markdown.contains("](<https://example.test/r>)"));

        let receipt = test_message(Some(text_part(
            "text/html",
            "<table><thead><tr><th>Item</th><th>Qty</th></tr></thead>\
             <tbody><tr><td>Tea</td><td>2</td></tr></tbody></table>",
        )));
        let body = render(&receipt, &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Html);
        assert!(body.markdown.contains("| Item | Qty |"));
        assert!(body.markdown.contains("| Tea | 2 |"));
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn plain_reply_chain_folds_history_and_signature() {
        let message = test_message(Some(text_part(
            "text/plain",
            "Ship it Friday.\n\n- [ ] confirm with Pat [[plans]]\n\n-- \nPat\nOn Tue, Sam wrote:\n> Old proposal\n> - [ ] stale task\n",
        )));
        let body = render(&message, &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Plain);
        assert!(body.markdown.contains("Ship it Friday."));
        assert!(!body.markdown.contains("Old proposal"));
        let quoted = body.quoted.as_deref().expect("history folds");
        assert!(quoted.contains("Old proposal"));
        assert_eq!(body.signature.as_deref(), Some("Pat"));
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn newsletter_renders_in_bulletin_mode_without_images() {
        let message = with_header(
            test_message(Some(text_part(
                "text/html",
                "<table><tr><td>Deal</td><td>$4</td></tr></table>\
                 <p><img src=\"https://example.test/i.png\" alt=\"chart\"></p>\
                 <p>Shop now</p>",
            ))),
            "List-Unsubscribe",
            "<https://example.test/u>",
        );
        let body = render(&message, &FetchedBodies::default());
        assert!(body.bulletin);
        assert_eq!(body.source, BodySource::Html);
        assert!(body.markdown.contains("Shop now"));
        assert!(!body.markdown.contains("!["));
        assert!(!body.markdown.contains("[🖼"));
        assert!(body.inline_images.is_empty());
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn cid_images_resolve_to_typed_descriptors_in_occurrence_order() {
        let html = "<p><img src=\"cid:logo\" alt=\"Brand\"><img src=\"cid:missing\" alt=\"Gone\"></p>";
        let image_part = |cid: &str| MimePart {
            part_id: None,
            mime_type: "image/png".to_string(),
            filename: Some("logo.png".to_string()),
            headers: vec![header("Content-ID", cid)],
            body: Some(MimeBody {
                size: Some(3),
                data: Some(encode("PNG")),
                attachment_id: None,
            }),
            parts: Vec::new(),
        };
        let payload = MimePart {
            part_id: None,
            mime_type: "multipart/related".to_string(),
            filename: None,
            headers: Vec::new(),
            body: None,
            parts: vec![text_part("text/html", html), image_part("<logo>")],
        };
        let body =
            render(&test_message(Some(payload)), &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Html);
        assert_eq!(body.inline_images.len(), 1);
        assert_eq!(body.inline_images[0].content_id.as_deref(), Some("<logo>"));
        assert_eq!(body.inline_images[0].filename.as_deref(), Some("logo.png"));
        assert!(body.markdown.contains("🖼 Brand"));
        assert!(body.markdown.contains("🖼 Gone"));
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn attachment_backed_body_hydrates_on_second_render() {
        let mut message = test_message(Some(wrapped_part(
            vec![fetchable_part("text/plain", "opaque-body")],
            "multipart/mixed",
        )));
        message.snippet = Some("Provisional preview".to_string());
        let unresolved = render(&message, &FetchedBodies::default());
        assert_eq!(unresolved.needs_fetch.len(), 1);
        assert_eq!(unresolved.source, BodySource::Snippet);
        assert!(unresolved.markdown.contains("Provisional preview"));

        let mut fetched = FetchedBodies::default();
        fetched.insert("m1", "opaque-body", b"hydrated body".to_vec());
        let resolved = render(&message, &fetched);
        assert!(resolved.needs_fetch.is_empty());
        assert_eq!(resolved.source, BodySource::Plain);
        assert_eq!(resolved.markdown, "hydrated body");
        // The model is untouched: callers re-render with fetched bytes.
        assert!(message.payload.is_some());
    }

    #[test]
    fn windows_1252_plain_decodes() {
        use base64::engine::general_purpose::URL_SAFE;
        let part = MimePart {
            part_id: None,
            mime_type: "text/plain".to_string(),
            filename: None,
            headers: vec![header(
                "Content-Type",
                "text/plain; charset=windows-1252",
            )],
            body: Some(MimeBody {
                size: Some(4),
                data: Some(URL_SAFE.encode(b"caf\xe9")),
                attachment_id: None,
            }),
            parts: Vec::new(),
        };
        let body = render(&test_message(Some(part)), &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Plain);
        assert_eq!(body.markdown, "café");
    }

    #[test]
    fn outlook_plain_reply_folds_forward() {
        let body = render(
            &test_message(Some(text_part(
                "text/plain",
                "See below.\n---------- Forwarded message ---------\nFrom: Pat\r\nSubject: Fwd: hello\r\n\r\nForwarded body #tag [[x]]\n",
            ))),
            &FetchedBodies::default(),
        );
        assert_eq!(body.source, BodySource::Plain);
        assert_eq!(body.markdown, "See below.");
        assert!(body.quoted.as_deref().unwrap().contains("Forwarded body"));
        assert_inert_doc(&joined(&body));
    }

    #[test]
    fn empty_body_renders_empty_without_fetch() {
        let empty = MimePart {
            part_id: None,
            mime_type: "text/plain".to_string(),
            filename: None,
            headers: Vec::new(),
            body: Some(MimeBody {
                size: Some(0),
                data: None,
                attachment_id: None,
            }),
            parts: Vec::new(),
        };
        let body =
            render(&test_message(Some(empty)), &FetchedBodies::default());
        assert!(body.markdown.is_empty());
        assert!(body.needs_fetch.is_empty());

        let missing = render(&test_message(None), &FetchedBodies::default());
        assert!(missing.markdown.is_empty());
        assert_eq!(missing.source, BodySource::Snippet);
    }

    #[test]
    fn attachment_indices_are_one_based_in_mime_order() {
        let payload = wrapped_part(
            vec![
                text_part("text/plain", "Body"),
                attachment_part("b.pdf", "id-b"),
                attachment_part("a.pdf", "id-a"),
            ],
            "multipart/mixed",
        );
        let body =
            render(&test_message(Some(payload)), &FetchedBodies::default());
        assert_eq!(body.attachments.len(), 2);
        assert_eq!(body.attachments[0].index, 1);
        assert_eq!(body.attachments[0].filename.as_deref(), Some("b.pdf"));
        assert_eq!(body.attachments[0].attachment_id.as_deref(), Some("id-b"));
        assert_eq!(body.attachments[1].index, 2);
        assert_eq!(body.attachments[1].filename.as_deref(), Some("a.pdf"));
    }

    #[test]
    fn six_message_requoting_chain_keeps_new_prose_in_main() {
        let mut history = String::new();
        for round in 1..=6u32 {
            let text = format!("Update {round}\n{history}");
            let body = render(
                &test_message(Some(text_part("text/plain", &text))),
                &FetchedBodies::default(),
            );
            assert!(
                body.markdown.contains(&format!("Update {round}")),
                "round {round} main lost:\n{}",
                body.markdown
            );
            for prior in 1..round {
                assert!(
                    !body.markdown.contains(&format!("Update {prior}")),
                    "round {round} main leaks Update {prior}:\n{}",
                    body.markdown
                );
            }
            if round > 1 {
                let quoted = body.quoted.as_deref().expect("history folds");
                assert!(quoted.contains(&format!("Update {}", round - 1)));
            }
            assert_inert_doc(&joined(&body));
            history = format!("> Update {round}\n{history}");
        }
    }

    #[test]
    fn hostile_everything_stays_inert() {
        let hostile_subject =
            "Re: [[plans]] <%todo%> <!--x--> #update $5-$10 ^abc";
        let hostile_from = "Evil <evil@example.test> [key:: v]";
        let html = text_part(
            "text/html",
            "<script>alert(1)</script><p>Click [[here]] #now</p>\
             <p><a href=\"javascript:alert(1)\">evil label [[x]]</a></p>\
             ```dataview\nTABLE FROM #x\n```\n\
             <div class=\"gmail_quote\">quoted [[history]] - [ ] task</div>\
             <div class=\"gmail_signature\">sig [key:: v] needle-hostile</div>",
        );
        let mut message = test_message(Some(wrapped_part(
            vec![html, attachment_part("evil [[x]] - [ ].pdf", "id-1")],
            "multipart/mixed",
        )));
        message.headers.push(header("Subject", hostile_subject));
        message.headers.push(header("From", hostile_from));
        if let Some(payload) = message.payload.as_mut() {
            payload.headers.push(header("Subject", hostile_subject));
        }
        message.snippet = Some("snippet [[x]] - [ ] task <%y%>".to_string());
        let body = render(&message, &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Html);
        assert_inert_doc(&joined(&body));
        assert!(joined(&body).contains("needle-hostile"));

        let metadata = inert_metadata(&message, HeaderKind::Subject);
        assert!(metadata.contains("plans"));
        assert!(metadata.contains("update"));
        assert_inert_doc(&metadata);
        let missing = inert_metadata(&message, HeaderKind::MessageId);
        assert!(missing.is_empty());

        let raw_name = body.attachments[0].filename.as_deref().unwrap();
        assert!(raw_name.contains("[[x]]"));
        assert_inert_doc(&crate::native::gmail::body::inert_single_line(
            raw_name,
        ));
    }

    #[test]
    fn gog_thread_round_trip_with_hydration() {
        let envelope = format!(
            r#"{{"thread":{{"id":"t9","historyId":"42","messages":[
                {{"id":"m1","threadId":"t9","labelIds":["INBOX"],"payload":{{"mimeType":"text/plain","body":{{"size":2,"data":"{plain}"}},"headers":[{{"name":"Subject","value":"Hi"}}]}}}},
                {{"id":"m2","threadId":"t9","labelIds":["INBOX","DRAFT"],"payload":{{"mimeType":"text/plain","body":{{"size":2,"data":"{plain}"}}}}}},
                {{"id":"m3","threadId":"t9","labelIds":["INBOX"],"payload":{{"mimeType":"text/plain","body":{{"size":8,"attachmentId":"body-3"}}}}}}
            ]}}}}"#,
            plain = encode("Hi"),
        );
        let thread = GmailThread::from_gog_json(envelope.as_bytes()).unwrap();
        let ids: Vec<_> = importable_messages(&thread)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, vec!["m1", "m3"]);

        let pending =
            render_message(&thread.messages[2], &FetchedBodies::default())
                .unwrap();
        assert_eq!(pending.needs_fetch.len(), 1);

        let mut fetched = FetchedBodies::default();
        fetched.insert("m3", "body-3", b"fetched text".to_vec());
        let resolved = render_message(&thread.messages[2], &fetched).unwrap();
        assert!(resolved.needs_fetch.is_empty());
        assert_eq!(resolved.markdown, "fetched text");
        assert_eq!(render(&thread.messages[0], &fetched).markdown, "Hi");
    }

    #[test]
    fn consumer_shaped_note_fragment_holds_concatenation() {
        let mut message = test_message(Some(text_part(
            "text/plain",
            "Main [[x]] - [ ] task\n> quoted #tag [key:: v]\n-- \nsig ^abc <%t%>",
        )));
        message
            .headers
            .push(header("Subject", "Re: [[plans]] $5-$10"));
        if let Some(payload) = message.payload.as_mut() {
            payload
                .headers
                .push(header("Subject", "Re: [[plans]] $5-$10"));
        }
        let body = render(&message, &FetchedBodies::default());
        let subject = crate::native::gmail::body::inert_single_line(
            "Re: [[plans]] $5-$10",
        );

        // Trusted wrapper syntax owned by mail-note; sender bytes only ever
        // appear through inert fields.
        let alias_line = "[[2026-11-05|Quarterly review]]";
        let quote_block = |text: &str| {
            text.split('\n')
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let doc = format!(
            "---\nsubject: {subject}\n---\n# {subject}\n\n{main}\n\n{quoted}\n\n{sig}\n\n{alias}\n",
            main = body.markdown,
            quoted = body
                .quoted
                .as_deref()
                .map_or_else(String::new, quote_block),
            sig = body.signature.as_deref().map_or_else(String::new, quote_block),
            alias = alias_line,
        );
        assert_inert_doc(&doc.replace(alias_line, ""));
        assert!(doc.contains("Quarterly"), "trusted alias text lost");
        assert!(doc.contains("Main"), "sender prose lost");
    }

    /// Deterministic pseudo-random generator (xorshift64* with a fixed seed):
    /// no new dev-dependencies, reproducible adversarial coverage.
    struct XorShift64 {
        state: u64,
    }

    impl XorShift64 {
        fn next(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.state = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    #[test]
    fn adversarial_corpus_over_public_entries_stays_inert() {
        let alphabet = [
            "[[",
            "]]",
            "![[",
            "[",
            "]",
            "(",
            ")",
            "#",
            "^",
            "%%",
            "$$",
            "`",
            "```",
            "<",
            ">",
            "> ",
            "-- ",
            "On Tue, Pat wrote:",
            "!",
            "=",
            "$=",
            ":",
            "::",
            ";",
            "-",
            "*",
            "+",
            "|",
            "~",
            "\\",
            "&",
            "&lt;",
            "&#32;",
            "key:: value",
            "- [ ]",
            "<%",
            "<!--",
            "bob:gmail",
            "needle42",
            "hello",
            "wörld",
            "日本語",
            "🎉",
            "🍅",
            "\u{a0}",
            "\u{200b}",
            "\u{200c}",
            "\u{2028}",
            "\u{0}",
            "\u{7}",
            "\r\n",
            "\t",
            " ",
            "\n\n",
            "https://example.test/a)b",
            "javascript:alert(1)",
            "/relative/path",
            "mailto:a@example.test",
            "cid:logo",
            "data:text/html,hi",
        ];
        let mut rng = XorShift64 {
            state: 0x9e37_79b9_7f4a_7c15,
        };
        for _ in 0..300 {
            let pieces = rng.below(8) + 1;
            let mut input = String::new();
            for _ in 0..pieces {
                input.push_str(alphabet[rng.below(alphabet.len())]);
            }
            input.push_str(" needle42 ");

            let mut message = test_message(Some(text_part(
                "text/html",
                &format!(
                    "<div><a href=\"{input}\">label {input}</a></div>\
                     <p>{input}</p>\
                     <div class=\"gmail_quote\">history {input}</div>\
                     <div class=\"gmail_signature\">sig {input}</div>"
                ),
            )));
            message.headers.push(header("Subject", &input));
            message.headers.push(header("From", &input));
            if let Some(payload) = message.payload.as_mut() {
                payload.headers.push(header("Subject", &input));
                payload.parts.push(attachment_part(&input, "adv-id"));
            }
            message.snippet = Some(input.clone());

            let first = render(&message, &FetchedBodies::default());
            let second = render(&message, &FetchedBodies::default());
            assert_eq!(first.markdown, second.markdown);
            assert_eq!(first.quoted, second.quoted);
            assert_eq!(first.signature, second.signature);

            let doc = joined(&first);
            assert_inert_doc(&doc);
            assert!(
                doc.contains("needle42"),
                "marker lost for {input:?} in:\n{doc}"
            );

            // Every public entry point neutralizes the same input.
            let plain = render_plain(&format!(
                "note {input}\n> quoted {input}\n-- \nsig {input}"
            ));
            assert_inert_doc(&joined(&RenderedBody {
                markdown: plain.markdown,
                quoted: plain.quoted,
                signature: plain.signature,
                source: BodySource::Plain,
                bulletin: false,
                inline_images: Vec::new(),
                attachments: Vec::new(),
                needs_fetch: Vec::new(),
            }));
            let metadata = inert_metadata(&message, HeaderKind::Subject);
            assert_inert_doc(&metadata);
            assert!(metadata.contains("needle42"));
            let raw_name = format!("{input}.pdf");
            assert_inert_doc(&crate::native::gmail::body::inert_single_line(
                &raw_name,
            ));
        }
    }

    #[test]
    fn snippet_fallback_covers_bodiless_messages() {
        let mut message = test_message(None);
        message.snippet =
            Some("preview [[x]] - [ ] task <%y%> needle-snippet".to_string());
        let body = render(&message, &FetchedBodies::default());
        assert_eq!(body.source, BodySource::Snippet);
        assert!(body.markdown.contains("needle-snippet"));
        assert!(body.quoted.is_none());
        assert!(body.signature.is_none());
        assert_inert_doc(&body.markdown);
    }
}

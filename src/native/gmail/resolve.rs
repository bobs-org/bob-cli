//! Read-only resolution of user locators to confirmed Gmail thread identities.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use super::{
    account,
    gog::{GmailLabel, Gog},
    ids::{self, GmailRef, GmailVerdict},
    model::GmailThread,
    note::{self, index::MailIndex, NoteError},
    GmailError,
};

const RFC_SEARCH_PAGE_SIZE: u32 = 500;
const RFC_SEARCH_MAX_PAGES: usize = 10;

/// The read-only part of gog used by the resolver and save preparation. This
/// small boundary keeps identity and hydration logic unit-testable without
/// invoking OAuth or the Gmail service.
pub(crate) trait GmailRead {
    /// Fetch one attachment's bytes through the narrowly allowed Discovery
    /// read. Implementations return raw bytes once; errors stay body-free.
    fn attachment_bytes(
        &self,
        account: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GmailError>;
    fn thread_get(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<GmailThread, GmailError>;

    fn message_metadata(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<serde_json::Value, GmailError>;

    fn search_page(
        &self,
        account: &str,
        query: &str,
        max: u32,
        page: Option<&str>,
    ) -> Result<serde_json::Value, GmailError>;

    fn labels_list(&self, account: &str)
        -> Result<Vec<GmailLabel>, GmailError>;
}

impl GmailRead for Gog {
    fn attachment_bytes(
        &self,
        account: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GmailError> {
        Gog::attachment_bytes(self, account, message_id, attachment_id)
    }

    fn thread_get(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<GmailThread, GmailError> {
        Gog::thread_get(self, account, thread_id)
    }

    fn message_metadata(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<serde_json::Value, GmailError> {
        Gog::message_metadata(self, account, message_id)
    }

    fn search_page(
        &self,
        account: &str,
        query: &str,
        max: u32,
        page: Option<&str>,
    ) -> Result<serde_json::Value, GmailError> {
        Gog::search_page(self, account, query, max, page)
    }

    fn labels_list(
        &self,
        account: &str,
    ) -> Result<Vec<GmailLabel>, GmailError> {
        Gog::labels_list(self, account)
    }
}

/// A thread identity confirmed by the selected account. `known_path` is an
/// already indexed saved note, or the explicit note path supplied by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedIdentity {
    pub(crate) account: String,
    pub(crate) thread_id: String,
    pub(crate) known_path: Option<String>,
}

/// A confirmed thread fetched without writes, suitable for preparation by a
/// later save phase. It intentionally has no `Debug` or serialization impl.
pub(crate) struct ConfirmedThread {
    pub(crate) identity: ResolvedIdentity,
    pub(crate) thread: GmailThread,
}

/// Select the account for a network operation using the shared noninteractive
/// Google account resolver. A missing grant is a typed, body-free resolution
/// failure for save/link callers.
pub(crate) fn resolve_account(
    gog: &Gog,
    override_email: Option<&str>,
    configured_email: Option<&str>,
) -> Result<String, GmailError> {
    account::resolve_command(gog, override_email.or(configured_email))
        .map_err(account_error)
}

/// Scan the frontmatter-only mail index once for a batch, keeping message
/// bodies out of the resolver's path lookup.
pub(crate) fn scan_index(
    vault: &Path,
    folder: &Path,
    link_template: &str,
) -> Result<MailIndex, GmailError> {
    note::index::scan(vault, folder, link_template).map_err(|error| {
        if error == NoteError::InvalidFolder {
            GmailError::setup(
                "config",
                "gmail.folder must be a relative path without parent segments"
                    .to_string(),
            )
        } else {
            GmailError::runtime(
                "unresolvable",
                "could not read the saved mail index".to_string(),
            )
        }
    })
}

/// Resolve a bare ID, Gmail URL, or existing mail-note path to a thread ID.
/// Direct thread IDs are only candidates here; `fetch_confirmed_thread`
/// checks that Gmail returns the exact same identity before the caller can
/// prepare or write anything.
pub(crate) fn resolve_identity<R: GmailRead>(
    reader: &R,
    account: &str,
    vault: &Path,
    folder: &Path,
    index: &MailIndex,
    input: &str,
) -> Result<ResolvedIdentity, GmailError> {
    let selected_account = account.trim();
    if selected_account.is_empty() || !selected_account.contains('@') {
        return Err(GmailError::setup(
            "config",
            "a valid Gmail account is required".to_string(),
        ));
    }

    let locator = parse_locator(input)?;
    let note_identity = if locator.is_none() {
        resolve_note_path(vault, folder, input)?
    } else {
        None
    };
    let (thread_id, explicit_path) =
        if let Some((path, note_account, id)) = note_identity {
            if !note_account.eq_ignore_ascii_case(selected_account) {
                return Err(GmailError::runtime(
                    "other_account",
                    "the selected mail note belongs to another Gmail account"
                        .to_string(),
                ));
            }
            (id, Some(path))
        } else {
            let locator = locator.ok_or_else(|| {
                unresolvable(
                    "input is not a saved mail note, Gmail URL, or thread ID"
                        .to_string(),
                )
            })?;
            let thread_id = match locator {
                GmailRef::Thread { hex } => hex,
                GmailRef::Message { hex } => {
                    resolve_message_thread(reader, selected_account, &hex)?
                }
                GmailRef::RfcMessageId { id } => {
                    resolve_rfc_thread(reader, selected_account, &id)?
                }
            };
            (thread_id, None)
        };

    let known_path = match explicit_path {
        Some(path) => {
            check_explicit_index_identity(
                index,
                selected_account,
                &thread_id,
                &path,
            )?;
            Some(path)
        }
        None => indexed_path(index, selected_account, &thread_id)?,
    };

    Ok(ResolvedIdentity {
        account: selected_account.to_string(),
        thread_id,
        known_path,
    })
}

/// Fetch the full thread only after locator resolution. The returned
/// `thread` envelope, thread ID, and every child message's thread ID must
/// agree exactly with the confirmed account-qualified identity.
pub(crate) fn fetch_confirmed_thread<R: GmailRead>(
    reader: &R,
    identity: ResolvedIdentity,
) -> Result<ConfirmedThread, GmailError> {
    let thread = reader
        .thread_get(&identity.account, &identity.thread_id)
        .map_err(resolution_remote_error)?;
    if thread.id != identity.thread_id
        || thread
            .messages
            .iter()
            .any(|message| message.thread_id != identity.thread_id)
    {
        return Err(unresolvable(
            "Gmail returned a different thread identity".to_string(),
        ));
    }
    Ok(ConfirmedThread { identity, thread })
}

fn parse_locator(input: &str) -> Result<Option<GmailRef>, GmailError> {
    if let Some(parsed) = ids::parse_gmail_url(input) {
        return match parsed.verdict {
            GmailVerdict::Resolvable(reference) => Ok(Some(reference)),
            GmailVerdict::Unresolvable { reason } => Err(unresolvable(reason)),
            GmailVerdict::Refused { reason } => {
                Err(GmailError::runtime("refused", reason))
            }
        };
    }
    let trimmed = input.trim();
    if is_hex_id(trimmed) {
        return Ok(Some(GmailRef::Thread {
            hex: trimmed.to_ascii_lowercase(),
        }));
    }
    Ok(None)
}

fn resolve_message_thread<R: GmailRead>(
    reader: &R,
    account: &str,
    message_id: &str,
) -> Result<String, GmailError> {
    let message = reader
        .message_metadata(account, message_id)
        .map_err(resolution_remote_error)?;
    let returned_id = message.get("id").and_then(serde_json::Value::as_str);
    let thread_id = message.get("threadId").and_then(serde_json::Value::as_str);
    let (Some(returned_id), Some(thread_id)) = (returned_id, thread_id) else {
        return Err(unresolvable(
            "Gmail returned incomplete message identity metadata".to_string(),
        ));
    };
    if returned_id != message_id || thread_id.is_empty() {
        return Err(unresolvable(
            "Gmail returned a different message identity".to_string(),
        ));
    }
    Ok(thread_id.to_string())
}

fn resolve_rfc_thread<R: GmailRead>(
    reader: &R,
    account: &str,
    message_id: &str,
) -> Result<String, GmailError> {
    let query = format!("rfc822msgid:{message_id}");
    let mut page = None::<String>;
    let mut seen_pages = BTreeSet::new();
    let mut matches = BTreeSet::new();

    for page_number in 0..RFC_SEARCH_MAX_PAGES {
        let result = reader
            .search_page(account, &query, RFC_SEARCH_PAGE_SIZE, page.as_deref())
            .map_err(resolution_remote_error)?;
        let threads = result
            .get("threads")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                GmailError::runtime(
                    "bad_json",
                    "gog returned an invalid Gmail search result".to_string(),
                )
            })?;
        for item in threads {
            let id = item
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    GmailError::runtime(
                        "bad_json",
                        "gog returned a search result without a thread ID"
                            .to_string(),
                    )
                })?;
            matches.insert(id.to_string());
            if matches.len() > 1 {
                return Err(ambiguous(
                    "the RFC message ID matches more than one thread"
                        .to_string(),
                ));
            }
        }

        let next = match result.get("nextPageToken") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(token)) if token.is_empty() => None,
            Some(serde_json::Value::String(token)) => Some(token.clone()),
            Some(_) => {
                return Err(GmailError::runtime(
                    "bad_json",
                    "gog returned an invalid Gmail search page token"
                        .to_string(),
                ));
            }
        };
        let Some(next) = next else {
            return matches.into_iter().next().ok_or_else(|| {
                unresolvable("the RFC message ID matched no thread".to_string())
            });
        };
        if !seen_pages.insert(next.clone()) {
            return Err(ambiguous(
                "the RFC message ID search could not be exhausted".to_string(),
            ));
        }
        if page_number + 1 == RFC_SEARCH_MAX_PAGES {
            return Err(ambiguous(
                "the RFC message ID search has more pages than can be verified"
                    .to_string(),
            ));
        }
        page = Some(next);
    }
    Err(ambiguous(
        "the RFC message ID search could not be exhausted".to_string(),
    ))
}

fn resolve_note_path(
    vault: &Path,
    folder: &Path,
    input: &str,
) -> Result<Option<(String, String, String)>, GmailError> {
    let query = input.trim();
    if query.is_empty() {
        return Ok(None);
    }
    let query_path = PathBuf::from(query.replace('\\', "/"));
    let original_vault = vault;
    let vault = vault.canonicalize().map_err(|_| {
        GmailError::runtime(
            "unresolvable",
            "the vault path is unavailable".to_string(),
        )
    })?;
    let query_has_extension = query_path.extension().is_some();
    let (relative, was_absolute) = if query_path.is_absolute() {
        let relative = query_path
            .strip_prefix(original_vault)
            .or_else(|_| query_path.strip_prefix(&vault))
            .map_err(|_| refused_path())?;
        (normalize_relative(relative)?, true)
    } else {
        (normalize_relative(&query_path)?, false)
    };

    let mut candidates = BTreeSet::new();
    add_path_variants(&mut candidates, &relative);
    if !was_absolute {
        // A short path may be relative to the configured mail folder as well
        // as the vault root, matching the rest of Bob's note path inputs.
        add_path_variants(&mut candidates, &folder.join(&relative));
    }
    let path_like = was_absolute
        || query.contains('/')
        || query.contains('\\')
        || query_has_extension
        || candidates.iter().any(|path| vault.join(path).exists());
    if !path_like {
        return Ok(None);
    }

    let mut found = BTreeSet::new();
    for relative in candidates {
        if let Some(contents) = read_safe_note(&vault, &relative)? {
            let parsed = note::parse::parse(&contents).map_err(|_| {
                unresolvable(
                    "the selected note has no valid mail identity".to_string(),
                )
            })?;
            found.insert((
                path_to_slashes(&relative).ok_or_else(|| {
                    unresolvable(
                        "the selected mail path is not valid UTF-8".to_string(),
                    )
                })?,
                parsed.account,
                parsed.thread_id,
            ));
        }
    }
    match found.len() {
        0 => {
            if was_absolute || query_has_extension || query.contains('/') {
                Err(unresolvable(
                    "the selected path is not a valid saved mail note"
                        .to_string(),
                ))
            } else {
                Ok(None)
            }
        }
        1 => Ok(found.into_iter().next()),
        _ => Err(ambiguous(
            "the note path matches more than one saved mail note".to_string(),
        )),
    }
}

fn add_path_variants(candidates: &mut BTreeSet<PathBuf>, path: &Path) {
    candidates.insert(path.to_path_buf());
    if path.extension().is_none() {
        candidates.insert(path.with_extension("md"));
    }
}

fn normalize_relative(path: &Path) -> Result<PathBuf, GmailError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(refused_path());
            }
        }
    }
    Ok(normalized)
}

/// Return note bytes only when every path component below the canonical vault
/// is a real, non-symlink directory/file. `None` means the candidate does not
/// exist as a regular file.
fn read_safe_note(
    vault: &Path,
    relative: &Path,
) -> Result<Option<String>, GmailError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(refused_path());
    }
    let mut current = vault.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(_) => {
                return Err(unresolvable(
                    "could not read the selected mail note".to_string(),
                ));
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(refused_path());
        }
        let is_last = index + 1 == components.len();
        if is_last {
            if !metadata.is_file() {
                return Ok(None);
            }
        } else if !metadata.is_dir() {
            return Ok(None);
        }
    }
    fs::read_to_string(current).map(Some).map_err(|_| {
        unresolvable("could not read the selected mail note".to_string())
    })
}

fn indexed_path(
    index: &MailIndex,
    account: &str,
    thread_id: &str,
) -> Result<Option<String>, GmailError> {
    check_index_identity(index, account, thread_id)?;
    let matching: Vec<_> = index
        .by_identity
        .iter()
        .filter(|((known_account, known_id), _)| {
            known_account.eq_ignore_ascii_case(account)
                && ids_equal(known_id, thread_id)
        })
        .map(|(_, entry)| entry.path.clone())
        .collect();
    match matching.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err(ambiguous(
            "duplicate saved notes have this account and thread identity"
                .to_string(),
        )),
    }
}

fn check_index_identity(
    index: &MailIndex,
    account: &str,
    thread_id: &str,
) -> Result<(), GmailError> {
    if index.duplicates.iter().any(|(known_account, known_id, _)| {
        known_account.eq_ignore_ascii_case(account)
            && ids_equal(known_id, thread_id)
    }) {
        return Err(ambiguous(
            "duplicate saved notes have this account and thread identity"
                .to_string(),
        ));
    }
    Ok(())
}

fn check_explicit_index_identity(
    index: &MailIndex,
    account: &str,
    thread_id: &str,
    explicit_path: &str,
) -> Result<(), GmailError> {
    check_index_identity(index, account, thread_id)?;
    if index
        .by_identity
        .iter()
        .any(|((known_account, known_id), entry)| {
            known_account.eq_ignore_ascii_case(account)
                && ids_equal(known_id, thread_id)
                && entry.path != explicit_path
        })
    {
        return Err(ambiguous(
            "duplicate saved notes have this account and thread identity"
                .to_string(),
        ));
    }
    Ok(())
}

fn ids_equal(left: &str, right: &str) -> bool {
    if is_hex_id(left) && is_hex_id(right) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn path_to_slashes(path: &Path) -> Option<String> {
    path.components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

fn is_hex_id(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Fallback when the classified failure carried no hint of its own.
const SIGN_IN_HINT: &str = "run `bob gmail login`";

fn resolution_remote_error(error: GmailError) -> GmailError {
    match error.kind() {
        "not_found" => GmailError::runtime(
            "gone",
            "Google has no matching Gmail item".to_string(),
        ),
        "invalid_grant" | "not_logged_in" | "insufficient_scope"
        | "no_client" | "invalid_client" => GmailError::runtime(
            "awaiting_sign_in",
            "the bob-gmail read grant needs attention".to_string(),
        )
        .with_hint(error.hint().unwrap_or(SIGN_IN_HINT)),
        "network" | "timeout" | "api_disabled" | "bad_json" => error,
        _ => error,
    }
}

fn account_error(error: GmailError) -> GmailError {
    match error.kind() {
        "not_logged_in" => GmailError::runtime(
            "awaiting_sign_in",
            "no bob-gmail Gmail grant is available".to_string(),
        )
        .with_hint(error.hint().unwrap_or(SIGN_IN_HINT)),
        _ => error,
    }
}

fn unresolvable(message: String) -> GmailError {
    GmailError::runtime("unresolvable", message)
}

fn ambiguous(message: String) -> GmailError {
    GmailError::runtime("ambiguous", message)
}

fn refused_path() -> GmailError {
    GmailError::runtime(
        "refused",
        "saved mail input must be an in-vault, non-symlink path".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, fs};

    #[derive(Default)]
    struct MockRead {
        thread: Option<String>,
        message: Option<serde_json::Value>,
        pages: Vec<serde_json::Value>,
        page_calls: RefCell<Vec<Option<String>>>,
        thread_error: Option<GmailError>,
        message_error: Option<GmailError>,
    }

    impl GmailRead for MockRead {
        fn attachment_bytes(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, GmailError> {
            Err(GmailError::runtime(
                "bad_json",
                "mock transport has no attachment bytes".to_string(),
            ))
        }

        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            if let Some(error) = &self.thread_error {
                return Err(error.clone());
            }
            GmailThread::from_gog_json(
                self.thread
                    .as_deref()
                    .unwrap_or(
                        r#"{"thread":{"id":"0123456789abcdef","messages":[]}}"#,
                    )
                    .as_bytes(),
            )
            .map_err(|error| GmailError::runtime("bad_json", error.to_string()))
        }

        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            if let Some(error) = &self.message_error {
                return Err(error.clone());
            }
            Ok(self.message.clone().unwrap_or_else(|| {
                serde_json::json!({"id":"1111111111111111","threadId":"0123456789abcdef"})
            }))
        }

        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            page: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            self.page_calls.borrow_mut().push(page.map(str::to_string));
            let index = self.page_calls.borrow().len() - 1;
            self.pages.get(index).cloned().ok_or_else(|| {
                GmailError::runtime("bad_json", "missing mock page".to_string())
            })
        }

        fn labels_list(&self, _: &str) -> Result<Vec<GmailLabel>, GmailError> {
            Ok(Vec::new())
        }
    }

    fn empty_index() -> MailIndex {
        MailIndex::default()
    }

    #[test]
    fn confirms_direct_thread_and_rejects_returned_identity_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let identity = resolve_identity(
            &MockRead::default(),
            "Me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "0123456789ABCDEF",
        )
        .expect("candidate resolves");
        assert_eq!(identity.thread_id, "0123456789abcdef");
        assert_eq!(identity.account, "Me@example.com");
        assert!(fetch_confirmed_thread(&MockRead::default(), identity).is_ok());

        let wrong = MockRead {
            thread: Some(
                r#"{"thread":{"id":"aaaaaaaaaaaaaaaa","messages":[]}}"#
                    .to_string(),
            ),
            ..MockRead::default()
        };
        let identity = ResolvedIdentity {
            account: "Me@example.com".to_string(),
            thread_id: "0123456789abcdef".to_string(),
            known_path: None,
        };
        let error = match fetch_confirmed_thread(&wrong, identity) {
            Ok(_) => panic!("mismatch unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), "unresolvable");
    }

    #[test]
    fn message_resolution_uses_metadata_and_requires_exact_id() {
        let root = tempfile::tempdir().unwrap();
        let reader = MockRead::default();
        let identity = resolve_identity(
            &reader,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/?permmsgid=msg-f:1229782938247303441",
        )
        .expect("message URL resolves");
        assert_eq!(identity.thread_id, "0123456789abcdef");

        let wrong = MockRead {
            message: Some(serde_json::json!({
                "id":"2222222222222222",
                "threadId":"0123456789abcdef",
                "payload":{"body":{"data":"POISON BODY"}}
            })),
            ..MockRead::default()
        };
        let error = resolve_identity(
            &wrong,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/?permmsgid=msg-f:1229782938247303441",
        )
        .expect_err("identity mismatch");
        assert_eq!(error.kind(), "unresolvable");
        assert!(!error.message().contains("POISON"));
    }

    #[test]
    fn rfc_search_requires_one_exhausted_thread() {
        let root = tempfile::tempdir().unwrap();
        let reader = MockRead {
            pages: vec![
                serde_json::json!({"threads":[{"id":"0123456789abcdef"}],"nextPageToken":"p2"}),
                serde_json::json!({"threads":[],"nextPageToken":null}),
            ],
            ..MockRead::default()
        };
        let identity = resolve_identity(
            &reader,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/#search/rfc822msgid%3Aabc%40example.com",
        )
        .expect("one result on exhausted search");
        assert_eq!(identity.thread_id, "0123456789abcdef");
        assert_eq!(
            &*reader.page_calls.borrow(),
            &[None, Some("p2".to_string())]
        );

        let many = MockRead {
            pages: vec![serde_json::json!({"threads":[
                {"id":"0123456789abcdef"},{"id":"aaaaaaaaaaaaaaaa"}
            ]})],
            ..MockRead::default()
        };
        let error = resolve_identity(
            &many,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/#search/rfc822msgid%3Aabc%40example.com",
        )
        .expect_err("many results");
        assert_eq!(error.kind(), "ambiguous");

        let none = MockRead {
            pages: vec![serde_json::json!({"threads":[],"nextPageToken":null})],
            ..MockRead::default()
        };
        let error = resolve_identity(
            &none,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/#search/rfc822msgid%3Aabc%40example.com",
        )
        .expect_err("no result");
        assert_eq!(error.kind(), "unresolvable");

        let stuck = MockRead {
            pages: (0..RFC_SEARCH_MAX_PAGES)
                .map(|_| {
                    serde_json::json!({
                        "threads":[{"id":"0123456789abcdef"}],
                        "nextPageToken":"same-token"
                    })
                })
                .collect(),
            ..MockRead::default()
        };
        let error = resolve_identity(
            &stuck,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/#search/rfc822msgid%3Aabc%40example.com",
        )
        .expect_err("stuck pagination is ambiguous");
        assert_eq!(error.kind(), "ambiguous");

        let malformed = MockRead {
            pages: vec![serde_json::json!({
                "threads":[],
                "nextPageToken":42
            })],
            ..MockRead::default()
        };
        let error = resolve_identity(
            &malformed,
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "https://mail.google.com/mail/#search/rfc822msgid%3Aabc%40example.com",
        )
        .expect_err("malformed pagination token");
        assert_eq!(error.kind(), "bad_json");
    }

    #[test]
    fn paths_are_in_vault_non_symlink_and_mail_identity_checked() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("mail")).unwrap();
        fs::write(
            root.path().join("mail/saved.md"),
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: 0123456789abcdef\ntitle: Saved\nwho: Dana\n---\n# Saved\n· 1 message\n\n## Notes\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg 0123456789abcdef - -->\n> [!mail+] Dana\nbody\n^m-0123456789abcdef\n<!-- bob:gmail:end -->\n",
        )
        .unwrap();
        let note =
            resolve_note_path(root.path(), Path::new("mail"), "mail/saved")
                .unwrap();
        assert_eq!(note.expect("note path").2, "0123456789abcdef");
        let short =
            resolve_note_path(root.path(), Path::new("mail"), "saved").unwrap();
        assert_eq!(
            short.expect("optional folder-relative note").2,
            "0123456789abcdef"
        );

        let other_account = resolve_identity(
            &MockRead::default(),
            "other@example.com",
            root.path(),
            Path::new("mail"),
            &empty_index(),
            "mail/saved.md",
        )
        .expect_err("note account mismatch");
        assert_eq!(other_account.kind(), "other_account");

        let mut duplicate_index = MailIndex::default();
        duplicate_index.by_identity.insert(
            ("me@example.com".to_string(), "0123456789abcdef".to_string()),
            note::index::MailIndexEntry {
                account: "me@example.com".to_string(),
                thread_id: "0123456789abcdef".to_string(),
                path: "mail/other-copy.md".to_string(),
                title: "Saved".to_string(),
                who: "Dana".to_string(),
                label: "Saved".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 0,
                turn: None,
                turn_since: None,
            },
        );
        let duplicate = resolve_identity(
            &MockRead::default(),
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &duplicate_index,
            "mail/saved.md",
        )
        .expect_err("explicit note and index point to duplicate identities");
        assert_eq!(duplicate.kind(), "ambiguous");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                root.path().join("mail/saved.md"),
                root.path().join("mail/link.md"),
            )
            .unwrap();
            assert_eq!(
                resolve_note_path(
                    root.path(),
                    Path::new("mail"),
                    "mail/link.md"
                )
                .expect_err("symlink refused")
                .kind(),
                "refused"
            );
        }
        assert_eq!(
            resolve_note_path(
                root.path(),
                Path::new("mail"),
                "../mail/saved.md"
            )
            .expect_err("escape refused")
            .kind(),
            "refused"
        );
        assert_eq!(
            resolve_note_path(root.path(), Path::new("mail"), "/etc/passwd")
                .expect_err("outside refused")
                .kind(),
            "refused"
        );
    }

    #[test]
    fn index_duplicate_identity_is_ambiguous_and_404_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let index = MailIndex {
            duplicates: vec![(
                "ME@example.com".to_string(),
                "0123456789abcdef".to_string(),
                vec!["mail/a.md".to_string(), "mail/b.md".to_string()],
            )],
            ..MailIndex::default()
        };
        let error = resolve_identity(
            &MockRead::default(),
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &index,
            "0123456789abcdef",
        )
        .expect_err("duplicate identity");
        assert_eq!(error.kind(), "ambiguous");

        let gone = MockRead {
            thread_error: Some(GmailError::runtime(
                "not_found",
                "private remote detail must not escape".to_string(),
            )),
            ..MockRead::default()
        };
        let identity = ResolvedIdentity {
            account: "me@example.com".to_string(),
            thread_id: "0123456789abcdef".to_string(),
            known_path: None,
        };
        let error = match fetch_confirmed_thread(&gone, identity) {
            Ok(_) => panic!("gone unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), "gone");
        assert!(!error.message().contains("private remote detail"));

        let mut index = MailIndex::default();
        index.by_identity.insert(
            ("ME@example.com".to_string(), "0123456789abcdef".to_string()),
            note::index::MailIndexEntry {
                account: "ME@example.com".to_string(),
                thread_id: "0123456789abcdef".to_string(),
                path: "mail/existing.md".to_string(),
                title: "Saved".to_string(),
                who: "Dana".to_string(),
                label: "Saved".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 0,
                turn: None,
                turn_since: None,
            },
        );
        let known = resolve_identity(
            &MockRead::default(),
            "me@example.com",
            root.path(),
            Path::new("mail"),
            &index,
            "0123456789abcdef",
        )
        .expect("known identity");
        assert_eq!(known.known_path.as_deref(), Some("mail/existing.md"));
    }

    #[test]
    fn message_url_decimal_normalization_and_malformed_ids() {
        assert!(parse_locator("0123456789abcdef").unwrap().is_some());
        assert!(parse_locator("not-an-id").unwrap().is_none());
        assert_eq!(
            parse_locator("https://mail.google.com/mail/#compose=new")
                .expect_err("compose refused")
                .kind(),
            "refused"
        );
    }
}

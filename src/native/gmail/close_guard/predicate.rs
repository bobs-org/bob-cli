//! Gmail Task predicate over one task block, plus loose mail-note facts.
//!
//! Placement reuses the watch scan helpers (`line_wikilinks`,
//! `direct_children`, `link_only_bullet`, `link_target`) so the guard and
//! the watch agree on what a task links: own-line wikilinks plus direct
//! link-only child bullets. Nested note bullets, prose children, embeds
//! (`![[…]]`), inline code, frontmatter, and fenced code never qualify.
//! Struck child bullets fail the link-only check, exactly like the scan.

use std::path::Path;

use serde_yaml::Value;

use std::collections::BTreeSet;

use crate::native::gmail::note::parse as note_parse;
use crate::native::gmail::sync::scan::{self as watch_scan, WatchScan};
use crate::native::vault_links::VaultLinkResolver;

/// Loose mail-note facts read from frontmatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailNoteFacts {
    /// Vault-relative mail-note path, `/`-separated.
    pub(crate) note: String,
    pub(crate) title: String,
    pub(crate) who: String,
    /// `yours`, `theirs`, or `None` for any other value.
    pub(crate) turn: Option<String>,
    pub(crate) turn_since: Option<String>,
    pub(crate) gmail_url: String,
}

/// Whether parsed frontmatter names a mail note under the loose glossary
/// rule: `type` is `"[[mail]]"` or `gmail_thread_id` is a non-empty
/// string. Both values come through [`note_parse::scalar`], so numbers
/// stringify the same way the save path reads them.
pub(crate) fn is_mail_note_frontmatter(frontmatter: &Value) -> bool {
    if note_parse::scalar(frontmatter, "type").as_deref() == Some("[[mail]]") {
        return true;
    }
    note_parse::scalar(frontmatter, "gmail_thread_id")
        .is_some_and(|value| !value.trim().is_empty())
}

/// Read a vault file's loose mail facts. Returns `None` when the file is
/// missing, has no frontmatter, or is not a mail note under
/// [`is_mail_note_frontmatter`].
pub(crate) fn read_mail_facts(
    vault: &Path,
    note: &Path,
) -> Option<MailNoteFacts> {
    let text = std::fs::read_to_string(vault.join(note)).ok()?;
    let frontmatter = note_parse::parse_frontmatter_only(&text)?;
    if !is_mail_note_frontmatter(&frontmatter) {
        return None;
    }
    let scalar =
        |key: &str| note_parse::scalar(&frontmatter, key).unwrap_or_default();
    let turn = note_parse::scalar(&frontmatter, "turn")
        .filter(|turn| matches!(turn.as_str(), "yours" | "theirs"));
    let turn_since = note_parse::scalar(&frontmatter, "turn_since");
    Some(MailNoteFacts {
        note: slashes(note),
        title: scalar("title"),
        who: scalar("who"),
        turn,
        turn_since,
        gmail_url: scalar("gmail_url"),
    })
}

/// Mail notes a task block links, in source order, each once: every
/// non-embedded wikilink on the task's own line, then every direct
/// link-only child bullet. `lines` is the note split into lines without
/// endings, `task_index` is the task row, and `hidden` reports fenced or
/// frontmatter lines to skip. Resolution and the mail check are injected
/// so the conformance vectors can run without a vault.
pub(crate) fn linked_mail_notes_in_block(
    line: &str,
    child_inners: &[&str],
    resolve: &dyn Fn(&str) -> Option<String>,
    is_mail: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    let mut push_inner = |inner: &str| {
        let Some(target) = watch_scan::link_target(inner) else {
            return;
        };
        let Some(note) = resolve(target) else {
            return;
        };
        if !is_mail(&note) {
            return;
        }
        if !notes.iter().any(|existing: &String| existing == &note) {
            notes.push(note);
        }
    };
    for inner in watch_scan::line_wikilinks(line) {
        push_inner(inner);
    }
    for inner in child_inners {
        push_inner(inner);
    }
    notes
}

/// The direct link-only child inners of the task at `task_index`.
pub(crate) fn direct_link_only_children(
    lines: &[&str],
    task_index: usize,
    task_width: usize,
    hidden: &dyn Fn(usize) -> bool,
) -> Vec<String> {
    watch_scan::direct_children(lines, task_index, task_width, hidden)
        .into_iter()
        .filter_map(|index| {
            watch_scan::link_only_bullet(lines[index]).map(str::to_string)
        })
        .collect()
}

/// Mail notes a task block in a real vault links, in source order, each
/// once. Returns an empty vector when the row is not a `#task` row.
pub(crate) fn linked_mail_notes(
    vault: &Path,
    note_relative: &Path,
    lines: &[&str],
    task_index: usize,
    resolver: &VaultLinkResolver,
) -> Vec<MailNoteFacts> {
    let line = lines.get(task_index).copied().unwrap_or("");
    let Some(task) = watch_scan::parse_task_row(line) else {
        return Vec::new();
    };
    let hidden = |index: usize| {
        crate::native::markdown::strictly_closed_frontmatter_end(lines)
            .is_some_and(|end| index <= end)
            || watch_scan::fenced_lines(
                lines,
                crate::native::markdown::strictly_closed_frontmatter_end(lines)
                    .map_or(0, |end| end + 1),
            )
            .contains(&index)
    };
    let children =
        direct_link_only_children(lines, task_index, task.width, &hidden);
    let resolve = |target: &str| match resolver.resolve(note_relative, target) {
        crate::native::vault_links::LinkResolution::Found(path) => {
            Some(slashes(&path))
        }
        _ => None,
    };
    let mut notes = Vec::new();
    for note in linked_mail_notes_in_block(
        line,
        &children.iter().map(String::as_str).collect::<Vec<_>>(),
        &|target| resolve(target),
        &|note| {
            read_mail_facts(vault, Path::new(note)).is_some_and(|facts| {
                facts.note == note || slashes(Path::new(&facts.note)) == note
            })
        },
    ) {
        if let Some(facts) = read_mail_facts(vault, Path::new(&note)) {
            notes.push(facts);
        }
    }
    notes
}

/// Other open tasks linking `note`, excluding the tasks being closed.
///
/// `scan` is the vault [`WatchScan`](watch_scan::WatchScan) (built by
/// `scan_watching_tasks`, which walks the same placements this predicate
/// uses); `exclude` holds the `(path, line)` rows this gesture closes.
/// A count of zero means the gesture orphans the thread.
pub(crate) fn other_open_linker_count(
    note: &str,
    exclude: &BTreeSet<(String, usize)>,
    scan: &WatchScan,
) -> usize {
    scan.tasks()
        .iter()
        .filter(|task| {
            task.targets.iter().any(|target| target.note == note)
                && !exclude.contains(&(task.path.clone(), task.line))
        })
        .count()
}

fn slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Build an in-test resolver closure from a link→path map plus the set of
/// vault files, mirroring [`VaultLinkResolver`]: an exact vault-relative
/// hit wins, otherwise a unique case-insensitive basename wins, and an
/// ambiguous basename resolves to nothing.
#[cfg(test)]
pub(crate) fn test_resolver<'a>(
    link_to_path: &'a std::collections::BTreeMap<String, String>,
    vault_files: &'a std::collections::BTreeSet<String>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |target: &str| {
        if let Some(mapped) = link_to_path.get(target) {
            return Some(mapped.clone());
        }
        crate::native::vault_links::target_to_markdown_path(target).and_then(
            |candidate| {
                let slashed = candidate
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                if vault_files.contains(&slashed) {
                    return Some(slashed);
                }
                let stem = candidate.file_stem()?.to_str()?.to_lowercase();
                let mut hits: Vec<String> = vault_files
                    .iter()
                    .filter(|path| {
                        Path::new(path.as_str())
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .is_some_and(|candidate| {
                                candidate.to_lowercase() == stem
                            })
                    })
                    .cloned()
                    .collect();
                hits.sort();
                hits.dedup();
                (hits.len() == 1).then(|| hits.remove(0))
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::note::index::{MailIndex, MailIndexEntry};
    use crate::native::gmail::sync::scan::{
        scan_note_tasks, scan_watching_tasks, MailLinks,
    };
    use crate::native::gmail::sync::ThreadKey;

    fn write(vault: &Path, relative: &str, text: &str) {
        let path = vault.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const ACCOUNT: &str = "me@example.com";

    fn loose_mail(title: &str, who: &str) -> String {
        format!(
            "---\ntype: \"[[mail]]\"\ntitle: {title}\nwho: {who}\nturn: yours\nturn_since: 2026-11-07T14:00:00\ngmail_url: https://mail.google.com/mail/u/0/#inbox/abc123\n---\nbody\n"
        )
    }

    #[test]
    fn read_mail_facts_uses_the_loose_rule() {
        let vault = tempfile::tempdir().unwrap();
        let root = vault.path();
        write(
            root,
            "mail/2026/261107_lease.md",
            &loose_mail("Lease renewal", "Ada"),
        );
        let facts =
            read_mail_facts(root, Path::new("mail/2026/261107_lease.md"))
                .expect("loose type-only note is mail");
        assert_eq!(facts.title, "Lease renewal");
        assert_eq!(facts.who, "Ada");
        assert_eq!(facts.turn.as_deref(), Some("yours"));
        assert_eq!(facts.turn_since.as_deref(), Some("2026-11-07T14:00:00"));
        assert!(facts.gmail_url.contains("mail.google.com"));

        write(
            root,
            "mail/2026/thread_only.md",
            "---\ngmail_thread_id: abc123\ntitle: Thread\n---\nbody\n",
        );
        assert!(
            read_mail_facts(root, Path::new("mail/2026/thread_only.md"))
                .is_some(),
            "a string gmail_thread_id is mail without type"
        );

        write(
            root,
            "mail/2026/not_mail.md",
            "---\ncontent: untrusted-email\ntitle: Lease\n---\nbody\n",
        );
        assert!(
            read_mail_facts(root, Path::new("mail/2026/not_mail.md")).is_none(),
            "strict-only fields without the loose rule are not mail"
        );
        assert!(
            read_mail_facts(root, Path::new("mail/2026/missing.md")).is_none()
        );
    }

    #[test]
    fn linked_mail_notes_follow_scan_placement() {
        let vault = tempfile::tempdir().unwrap();
        let root = vault.path();
        write(
            root,
            "mail/2026/261107_lease.md",
            &loose_mail("Lease renewal", "Ada"),
        );
        write(
            root,
            "mail/2026/261108_oak.md",
            &loose_mail("Oak offer", "Bo"),
        );
        write(
            root,
            "job.md",
            "- [ ] #task Reply [[mail/2026/261107_lease]]\n  - [[mail/2026/261108_oak]]\n  - notes\n    - [[mail/2026/261108_oak]]\n",
        );
        let text = std::fs::read_to_string(root.join("job.md")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let resolver = VaultLinkResolver::new(root);
        let notes =
            linked_mail_notes(root, Path::new("job.md"), &lines, 0, &resolver);
        assert_eq!(
            notes
                .iter()
                .map(|facts| facts.note.clone())
                .collect::<Vec<_>>(),
            vec![
                "mail/2026/261107_lease.md".to_string(),
                "mail/2026/261108_oak.md".to_string(),
            ],
            "own line first, then the link-only child; the nested bullet never counts"
        );
    }

    #[test]
    fn orphaned_count_excludes_the_closing_rows() {
        let vault = tempfile::tempdir().unwrap();
        let root = vault.path().to_path_buf();
        write(
            &root,
            "mail/2026/lease.md",
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: aa01\ntitle: Lease\nwho: Dana\n---\nbody\n",
        );
        write(
            &root,
            "job.md",
            "- [ ] #task Renew [[mail/2026/lease]]\n- [ ] #task Also [[mail/2026/lease]]\n",
        );
        let mut index = MailIndex::default();
        index.by_identity.insert(
            (ACCOUNT.to_string(), "aa01".to_string()),
            MailIndexEntry {
                account: ACCOUNT.to_string(),
                thread_id: "aa01".to_string(),
                path: "mail/2026/lease.md".to_string(),
                title: "Lease".to_string(),
                who: "Dana".to_string(),
                label: "Lease".to_string(),
                started: String::new(),
                updated: String::new(),
                messages: 1,
                turn: None,
                turn_since: None,
            },
        );
        let scan =
            scan_watching_tasks(&root, Path::new("mail"), &index).unwrap();
        assert_eq!(scan.tasks().len(), 2);
        let key = ThreadKey::new(ACCOUNT, "aa01").unwrap();
        assert_eq!(scan.watchers(&key).count(), 2);

        let mut closing = BTreeSet::new();
        closing.insert(("job.md".to_string(), 1));
        assert_eq!(
            other_open_linker_count("mail/2026/lease.md", &closing, &scan),
            1
        );
        closing.insert(("job.md".to_string(), 2));
        assert_eq!(
            other_open_linker_count("mail/2026/lease.md", &closing, &scan),
            0,
            "closing every linker orphans the thread"
        );

        // The scan-touching imports stay honest: one note scan through
        // MailLinks agrees with the vault scan.
        let mut links = MailLinks::new(&root, &index);
        let text = std::fs::read_to_string(root.join("job.md")).unwrap();
        let tasks = scan_note_tasks("job.md", &text, &mut links, true, None);
        assert_eq!(tasks.len(), 2);
    }
}

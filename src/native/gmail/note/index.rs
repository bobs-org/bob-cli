//! Cheap mail-note index built from frontmatter only.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_yaml::Value;

use super::{label, parse, NoteError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailIndexEntry {
    pub(crate) account: String,
    pub(crate) thread_id: String,
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) who: String,
    pub(crate) label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MailIndex {
    pub(crate) by_identity: BTreeMap<(String, String), MailIndexEntry>,
    pub(crate) duplicates: Vec<(String, String, Vec<String>)>,
}

/// Scan Markdown files below `folder`, returning paths relative to `vault`.
/// Only each file's frontmatter is read; note bodies can contain megabytes of
/// mail text and never enter this index.
pub(crate) fn scan(
    vault: &Path,
    folder: &Path,
    link_template: &str,
) -> Result<MailIndex, NoteError> {
    let folder_abs = vault.join(folder);
    super::path::validate_folder(folder)?;
    let mut files = Vec::new();
    collect_markdown(&folder_abs, &mut files)?;
    files.sort();

    let mut index = MailIndex::default();
    let mut found: BTreeMap<(String, String), Vec<MailIndexEntry>> =
        BTreeMap::new();
    for path in files {
        let Some(frontmatter) = read_frontmatter(&path)? else {
            continue;
        };
        if field(&frontmatter, "content").as_deref() != Some("untrusted-email")
        {
            continue;
        }
        if field(&frontmatter, "type").as_deref() != Some("[[mail]]") {
            continue;
        }
        let (Some(account), Some(thread_id), Some(title), Some(who)) = (
            field(&frontmatter, "gmail_account"),
            field(&frontmatter, "gmail_thread_id"),
            field(&frontmatter, "title"),
            field(&frontmatter, "who"),
        ) else {
            continue;
        };
        if account.is_empty()
            || thread_id.is_empty()
            || !thread_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            continue;
        }
        let relative = path.strip_prefix(vault).map_err(|_| NoteError::Io)?;
        let relative = path_to_slashes(relative);
        let entry = MailIndexEntry {
            account: account.clone(),
            thread_id: thread_id.to_ascii_lowercase(),
            path: relative,
            title: title.clone(),
            who: who.clone(),
            label: label::render_saved(link_template, &title, &who),
        };
        found
            .entry((account.to_ascii_lowercase(), entry.thread_id.clone()))
            .or_default()
            .push(entry);
    }
    for (identity, mut entries) in found {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        if entries.len() == 1 {
            index.by_identity.insert(identity, entries.remove(0));
        } else {
            index.duplicates.push((
                identity.0,
                identity.1,
                entries.into_iter().map(|entry| entry.path).collect(),
            ));
        }
    }
    Ok(index)
}

fn collect_markdown(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), NoteError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(())
        }
        Err(_) => return Err(NoteError::Io),
    };
    for entry in entries {
        let entry = entry.map_err(|_| NoteError::Io)?;
        let file_type = entry.file_type().map_err(|_| NoteError::Io)?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_markdown(&path, files)?;
        } else if file_type.is_file()
            && path.extension().is_some_and(|extension| extension == "md")
        {
            files.push(path);
        }
    }
    Ok(())
}

fn read_frontmatter(path: &Path) -> Result<Option<Value>, NoteError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Err(NoteError::Io),
    };
    let mut reader = BufReader::new(file);
    let mut first = String::new();
    reader.read_line(&mut first).map_err(|_| NoteError::Io)?;
    if first.trim() != "---" {
        return Ok(None);
    }
    let mut yaml = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|_| NoteError::Io)?;
        if read == 0 {
            return Ok(None);
        }
        if line.trim() == "---" {
            break;
        }
        yaml.push_str(&line);
    }
    Ok(parse::parse_frontmatter_only(&format!("---\n{yaml}---\n")))
}

fn field(frontmatter: &Value, key: &str) -> Option<String> {
    let value = frontmatter
        .as_mapping()?
        .get(Value::String(key.to_string()))?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_nested_mail_notes_from_frontmatter_and_ignores_other_notes() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let folder = vault.join("mail");
        fs::create_dir_all(folder.join("2026/nested")).unwrap();
        fs::write(
            folder.join("2026/nested/thread.md"),
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: AABBCCDD\ntitle: Lease\nwho: Dana\n---\nvery long body that should not be read by the index\n",
        ).unwrap();
        fs::write(
            folder.join("ordinary.md"),
            "---\ntitle: Not mail\n---\nbody",
        )
        .unwrap();
        let index =
            scan(vault, Path::new("mail"), "✉️ {subject} · {who}").unwrap();
        let entry = index
            .by_identity
            .get(&("me@example.com".to_string(), "aabbccdd".to_string()))
            .unwrap();
        assert_eq!(entry.path, "mail/2026/nested/thread.md");
        assert_eq!(entry.title, "Lease");
        assert_eq!(entry.who, "Dana");
        assert!(entry.label.contains("Lease"));
        assert_eq!(index.by_identity.len(), 1);
    }

    #[test]
    fn duplicate_thread_identities_are_reported_and_not_selected() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        fs::create_dir_all(vault.join("mail")).unwrap();
        for name in ["a.md", "b.md"] {
            fs::write(
                vault.join("mail").join(name),
                "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: aabbccdd\ntitle: Lease\nwho: Dana\n---\nbody\n",
            ).unwrap();
        }
        let index =
            scan(vault, Path::new("mail"), "✉️ {subject} · {who}").unwrap();
        assert!(index.by_identity.is_empty());
        assert_eq!(index.duplicates.len(), 1);
        assert_eq!(index.duplicates[0].2.len(), 2);
    }

    #[test]
    fn duplicate_accounts_are_compared_case_insensitively() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        fs::create_dir_all(vault.join("mail")).unwrap();
        for (name, account) in
            [("a.md", "me@example.com"), ("b.md", "ME@example.com")]
        {
            fs::write(
                vault.join("mail").join(name),
                format!(
                    "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: {account}\ngmail_thread_id: aabbccdd\ntitle: Lease\nwho: Dana\n---\nbody\n"
                ),
            )
            .unwrap();
        }
        let index =
            scan(vault, Path::new("mail"), "✉️ {subject} · {who}").unwrap();
        assert!(index.by_identity.is_empty());
        assert_eq!(index.duplicates.len(), 1);
    }
}

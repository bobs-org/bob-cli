//! Task-safe, alias-safe labels for mail-note wikilinks.

use crate::native::task_text_escape;

use super::text::subject_without_prefixes;

const SUBJECT_LABEL_CHARS: usize = 60;
const WHO_LABEL_CHARS: usize = 24;

/// Build a label using the configured template. The selected epic branch is
/// `subject_who`, whose default is `✉️ {subject} · {who}`.
pub(crate) fn render(template: &str, subject: &str, who: &str) -> String {
    let subject =
        truncate_label(&subject_without_prefixes(subject), SUBJECT_LABEL_CHARS);
    let subject = if subject.is_empty() {
        "(no subject)".to_string()
    } else {
        subject
    };
    let who = truncate_label(who.trim(), WHO_LABEL_CHARS);
    let safe_subject = task_text_escape::escape_task_text(&subject);
    let safe_who = task_text_escape::escape_task_text(&who);
    let expanded = template
        .replace("{subject}", &safe_subject)
        .replace("{who}", &safe_who);
    alias_safe(&expanded)
}

/// Render a label from the inert values already stored in mail-note
/// frontmatter. Re-applying the task escaper would double-escape those values.
pub(crate) fn render_saved(template: &str, subject: &str, who: &str) -> String {
    let subject = truncate_label(subject, SUBJECT_LABEL_CHARS);
    let subject = if subject.is_empty() {
        "(no subject)".to_string()
    } else {
        subject
    };
    let who = truncate_label(who, WHO_LABEL_CHARS);
    let expanded = template
        .replace("{subject}", &subject)
        .replace("{who}", &who);
    alias_safe(&expanded)
}

fn truncate_label(raw: &str, limit: usize) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = collapsed.chars().count();
    if count <= limit {
        return collapsed;
    }
    let mut byte_end = 0;
    let mut last_space = None;
    for (index, character) in collapsed.char_indices() {
        if collapsed[..index].chars().count() >= limit {
            break;
        }
        if character.is_whitespace() {
            last_space = Some(index);
        }
        byte_end = index + character.len_utf8();
    }
    let cut = last_space.filter(|index| *index > 0).unwrap_or(byte_end);
    format!("{}…", collapsed[..cut].trim_end())
}

/// Obsidian aliases cannot safely contain these delimiters. Break the
/// forbidden pairs without changing ordinary labels; the zero-width
/// non-joiner is invisible when rendered.
fn alias_safe(text: &str) -> String {
    text.replace('|', "¦")
        .replace("[[", "[\u{200c}[")
        .replace("]]", "]\u{200c}]")
        .replace("<%", "< %")
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::body::inert::inert_single_line;

    #[test]
    fn uses_both_parts_and_is_alias_safe() {
        let label = render(
            "✉️ {subject} · {who}",
            "Re: Q3 #budget [draft] | v2",
            "Dana [[ops]] <% mail",
        );
        assert!(label.starts_with("✉️ Q3 "));
        assert!(label.contains("Dana [\u{200c}[ops]"));
        assert!(!label.contains('|'));
        assert!(!label.contains("[["));
        assert!(!label.contains("]]"));
        assert!(!label.contains("<%"));
        assert!(!label.contains('\n'));
    }

    #[test]
    fn handles_empty_long_unicode_and_subject_only_templates() {
        assert_eq!(render("✉️ {subject}", "Re:", "someone"), "✉️ (no subject)");
        assert!(render("✉️ {subject}", &"é😀 ".repeat(30), "x").ends_with('…'));
        assert!(
            render("✉️ {subject}", &"word ".repeat(20), "x")
                .chars()
                .count()
                < 65
        );
    }

    #[test]
    fn labels_recomputed_from_saved_fields_stay_alias_safe() {
        let subject = "Q3 #budget";
        let who = "Dana";
        let template = "✉️ {subject} · {who}";
        let label = render_saved(
            template,
            &inert_single_line(subject),
            &inert_single_line(who),
        );
        assert!(label.contains("\\#budget"));
        assert!(!label.contains('|'));
        assert!(!label.contains("[["));
        assert!(!label.contains("]]"));
        assert!(!label.contains("<%"));
    }
}

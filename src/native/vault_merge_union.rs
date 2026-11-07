//! Same-point insertion union for Markdown vault-sync conflicts.
//!
//! `bob vault-sync` resolves a both-modified conflict by keeping the remote
//! file and quarantining the local copy. When every conflicting hunk of a
//! Markdown note is two pure insertions at the same point (typically a capture
//! on one host racing a `bob ref create` append on another), both insertions
//! can be kept instead. This module makes that decision from strings alone: it
//! parses `git merge-file --diff3` output, runs the safety checks, picks an
//! outline-safe order for each hunk, and assembles the result. It never touches
//! Git or the filesystem; the caller falls back to quarantine on `Refused`.

use std::collections::HashMap;

/// Marker width the caller passes to `git merge-file --marker-size`. Any blob
/// line starting with this many copies of a marker character refuses the
/// union, so every such line in the merge output is a real marker.
pub(super) const MARKER_SIZE: usize = 41;

const MARKER_CHARS: [u8; 4] = [b'<', b'|', b'=', b'>'];
const TAB_WIDTH: usize = 4;

/// `git merge-file` truncates its conflict count exit status to this value.
const MAX_REPORTED_CONFLICTS: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UnionOutcome {
    /// Every hunk was unioned; `text` is the full merged note.
    Merged { text: String, hunks: usize },
    /// The conflict is not a safe same-point insertion union.
    Refused(String),
}

/// The three index stages of one conflicted note plus Git's diff3 merge.
pub(super) struct UnionInput<'a> {
    pub(super) local: &'a str,
    pub(super) base: &'a str,
    pub(super) remote: &'a str,
    /// `git merge-file -p --diff3 --marker-size=MARKER_SIZE` output labelled
    /// `local`, `base`, and `remote`.
    pub(super) merged: &'a str,
    /// Conflict count from `git merge-file`'s exit status.
    pub(super) reported_conflicts: usize,
}

pub(super) fn union_same_point_insertions(
    input: &UnionInput<'_>,
) -> UnionOutcome {
    match try_union(input) {
        Ok((text, hunks)) => UnionOutcome::Merged { text, hunks },
        Err(reason) => UnionOutcome::Refused(reason),
    }
}

#[derive(Debug, Default)]
struct Block<'a> {
    local: Vec<&'a str>,
    base: Vec<&'a str>,
    remote: Vec<&'a str>,
}

#[derive(Debug)]
enum Segment<'a> {
    Clean(Vec<&'a str>),
    Conflict(Block<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    Open,
    Base,
    Separator,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Clean,
    Local,
    Base,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frontmatter {
    /// No line seen yet.
    Start,
    /// Line 1 opened YAML frontmatter that has not closed yet.
    Inside,
    /// Past the frontmatter, or the note has none.
    Body,
}

fn try_union(input: &UnionInput<'_>) -> Result<(String, usize), String> {
    for (label, blob) in [
        ("local", input.local),
        ("base", input.base),
        ("remote", input.remote),
    ] {
        if let Some(line_number) = marker_like_line(blob) {
            return Err(format!(
                "{label} line {line_number} looks like a merge marker"
            ));
        }
    }

    let segments = parse_merge_output(input.merged)?;
    let hunks = segments
        .iter()
        .filter(|segment| matches!(segment, Segment::Conflict(_)))
        .count();
    if hunks == 0 {
        return Err("git merge-file found no conflicting hunk".to_string());
    }
    let reported = input.reported_conflicts;
    let count_matches = if reported < MAX_REPORTED_CONFLICTS {
        hunks == reported
    } else {
        hunks >= reported
    };
    if !count_matches {
        return Err(format!(
            "parsed {hunks} conflicting hunk(s) but git reported {reported}"
        ));
    }

    let mut frontmatter = Frontmatter::Start;
    let mut hunk = 0;
    let mut text = String::with_capacity(input.merged.len());
    for (index, segment) in segments.iter().enumerate() {
        match segment {
            Segment::Clean(lines) => {
                for line in lines {
                    frontmatter = next_frontmatter_state(frontmatter, line);
                    text.push_str(line);
                }
            }
            Segment::Conflict(block) => {
                hunk += 1;
                check_block(hunk, block, frontmatter)?;
                frontmatter = Frontmatter::Body;
                let following = following_indent(&segments[index + 1..]);
                let (first, second) = choose_order(hunk, block, following)?;
                for line in first.iter().chain(second) {
                    text.push_str(line);
                }
            }
        }
    }

    check_block_ids(&text, input.local, input.remote)?;
    Ok((text, hunks))
}

fn parse_merge_output(merged: &str) -> Result<Vec<Segment<'_>>, String> {
    let mut segments = Vec::new();
    let mut clean = Vec::new();
    let mut block = Block::default();
    let mut state = ParseState::Clean;

    for (index, line) in merged.split_inclusive('\n').enumerate() {
        let line_number = index + 1;
        let marker = marker_kind(line).map_err(|message| {
            format!("merge output line {line_number}: {message}")
        })?;
        state = match (state, marker) {
            (ParseState::Clean, None) => {
                clean.push(line);
                ParseState::Clean
            }
            (ParseState::Clean, Some(Marker::Open)) => {
                if !clean.is_empty() {
                    segments.push(Segment::Clean(std::mem::take(&mut clean)));
                }
                ParseState::Local
            }
            (ParseState::Local, None) => {
                block.local.push(line);
                ParseState::Local
            }
            (ParseState::Local, Some(Marker::Base)) => ParseState::Base,
            (ParseState::Base, None) => {
                block.base.push(line);
                ParseState::Base
            }
            (ParseState::Base, Some(Marker::Separator)) => ParseState::Remote,
            (ParseState::Remote, None) => {
                block.remote.push(line);
                ParseState::Remote
            }
            (ParseState::Remote, Some(Marker::Close)) => {
                segments.push(Segment::Conflict(std::mem::take(&mut block)));
                ParseState::Clean
            }
            (_, Some(_)) => {
                return Err(format!(
                    "merge output line {line_number}: unexpected merge marker"
                ));
            }
        };
    }

    if state != ParseState::Clean {
        return Err("merge output ends inside a conflict block".to_string());
    }
    if !clean.is_empty() {
        segments.push(Segment::Clean(clean));
    }
    Ok(segments)
}

/// Classify one merge-output line. A line that starts with `MARKER_SIZE`
/// copies of a marker character must be exactly one of Git's four markers.
fn marker_kind(line: &str) -> Result<Option<Marker>, String> {
    let content = strip_terminator(line);
    let bytes = content.as_bytes();
    let Some(&first) = bytes.first() else {
        return Ok(None);
    };
    let (marker, label) = match first {
        b'<' => (Marker::Open, " local"),
        b'|' => (Marker::Base, " base"),
        b'=' => (Marker::Separator, ""),
        b'>' => (Marker::Close, " remote"),
        _ => return Ok(None),
    };
    if bytes.len() < MARKER_SIZE
        || !bytes[..MARKER_SIZE].iter().all(|byte| *byte == first)
    {
        return Ok(None);
    }
    if &content[MARKER_SIZE..] == label {
        Ok(Some(marker))
    } else {
        Err("malformed merge marker".to_string())
    }
}

fn marker_like_line(text: &str) -> Option<usize> {
    text.split_inclusive('\n')
        .position(|line| {
            let bytes = line.as_bytes();
            bytes.len() >= MARKER_SIZE
                && MARKER_CHARS.iter().any(|marker| {
                    bytes[..MARKER_SIZE].iter().all(|byte| byte == marker)
                })
        })
        .map(|index| index + 1)
}

fn check_block(
    hunk: usize,
    block: &Block<'_>,
    frontmatter: Frontmatter,
) -> Result<(), String> {
    if !block.base.is_empty() {
        return Err(format!(
            "hunk {hunk} changes existing lines (base side is not empty)"
        ));
    }
    for (label, side) in [("local", &block.local), ("remote", &block.remote)] {
        if !side.iter().any(|line| !is_blank(line)) {
            return Err(format!("hunk {hunk} {label} side inserts no text"));
        }
        if side.iter().any(|line| !line.ends_with('\n')) {
            return Err(format!(
                "hunk {hunk} {label} side has an unterminated line"
            ));
        }
        if side.iter().filter(|line| is_fence_line(line)).count() % 2 != 0 {
            return Err(format!(
                "hunk {hunk} {label} side has an unbalanced code fence"
            ));
        }
    }
    match frontmatter {
        Frontmatter::Inside => {
            return Err(format!(
                "hunk {hunk} starts inside the YAML frontmatter"
            ));
        }
        Frontmatter::Start
            if [&block.local, &block.remote].iter().any(|side| {
                side.first()
                    .is_some_and(|line| strip_terminator(line) == "---")
            }) =>
        {
            return Err(format!("hunk {hunk} inserts YAML frontmatter"));
        }
        Frontmatter::Start | Frontmatter::Body => {}
    }
    let local_lines = block
        .local
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| line.trim_end())
        .collect::<Vec<_>>();
    if block
        .remote
        .iter()
        .filter(|line| !is_blank(line))
        .any(|line| local_lines.contains(&line.trim_end()))
    {
        return Err(format!(
            "hunk {hunk} sides share an identical line (a union would duplicate it)"
        ));
    }
    Ok(())
}

/// Pick which side goes first so neither side's lines change parents.
fn choose_order<'b, 'a>(
    hunk: usize,
    block: &'b Block<'a>,
    following: Option<usize>,
) -> Result<(&'b [&'a str], &'b [&'a str]), String> {
    if let Some(next) = following.filter(|indent| *indent > 0) {
        for (label, side) in
            [("local", &block.local), ("remote", &block.remote)]
        {
            if non_blank_indents(side).any(|indent| indent < next) {
                return Err(format!(
                    "hunk {hunk} {label} side would re-parent the indented line after it"
                ));
            }
        }
    }
    if order_is_safe(&block.local, &block.remote) {
        return Ok((block.local.as_slice(), block.remote.as_slice()));
    }
    if order_is_safe(&block.remote, &block.local) {
        return Ok((block.remote.as_slice(), block.local.as_slice()));
    }
    Err(format!("hunk {hunk} has no outline-safe order"))
}

/// "first then second" keeps `second` attached to its own parents when no
/// line of `first` is shallower than `second`'s first non-blank line.
fn order_is_safe(first: &[&str], second: &[&str]) -> bool {
    let Some(anchor) = non_blank_indents(second).next() else {
        return true;
    };
    non_blank_indents(first).all(|indent| indent >= anchor)
}

/// Indent of the first non-blank line after a hunk. When the next non-blank
/// line belongs to another hunk, use the deeper of its two sides' first lines.
fn following_indent(segments: &[Segment<'_>]) -> Option<usize> {
    for segment in segments {
        match segment {
            Segment::Clean(lines) => {
                if let Some(indent) = non_blank_indents(lines).next() {
                    return Some(indent);
                }
            }
            Segment::Conflict(block) => {
                return non_blank_indents(&block.local)
                    .next()
                    .into_iter()
                    .chain(non_blank_indents(&block.remote).next())
                    .max();
            }
        }
    }
    None
}

fn check_block_ids(
    result: &str,
    local: &str,
    remote: &str,
) -> Result<(), String> {
    let local_counts = block_id_counts(local);
    let remote_counts = block_id_counts(remote);
    let mut result_counts =
        block_id_counts(result).into_iter().collect::<Vec<_>>();
    result_counts.sort_unstable();
    for (id, count) in result_counts {
        let allowed = local_counts
            .get(id)
            .copied()
            .unwrap_or(0)
            .max(remote_counts.get(id).copied().unwrap_or(0));
        if count > allowed {
            return Err(format!(
                "union would repeat block id ^{id} ({count} copies)"
            ));
        }
    }
    Ok(())
}

fn block_id_counts(text: &str) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for id in text.lines().filter_map(trailing_block_id) {
        *counts.entry(id).or_insert(0) += 1;
    }
    counts
}

/// The Obsidian block id (`^id`) ending a line, without its caret.
fn trailing_block_id(line: &str) -> Option<&str> {
    let content = line.trim_end();
    let caret = content.rfind('^')?;
    let id = &content[caret + 1..];
    let before = &content[..caret];
    (!id.is_empty()
        && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        && (before.is_empty() || before.ends_with(char::is_whitespace)))
    .then_some(id)
}

fn next_frontmatter_state(state: Frontmatter, line: &str) -> Frontmatter {
    let is_delimiter = strip_terminator(line) == "---";
    match state {
        Frontmatter::Start if is_delimiter => Frontmatter::Inside,
        Frontmatter::Inside if !is_delimiter => Frontmatter::Inside,
        _ => Frontmatter::Body,
    }
}

fn non_blank_indents<'s>(
    lines: &'s [&str],
) -> impl Iterator<Item = usize> + 's {
    lines
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| indent_width(line))
}

/// Leading-whitespace width with a tab counted as `TAB_WIDTH` columns.
fn indent_width(line: &str) -> usize {
    line.chars()
        .map_while(|ch| match ch {
            ' ' => Some(1),
            '\t' => Some(TAB_WIDTH),
            _ => None,
        })
        .sum()
}

fn is_fence_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn strip_terminator(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One region of a synthetic three-way merge.
    enum Piece<'a> {
        Clean(&'a str),
        Insert {
            local: &'a str,
            remote: &'a str,
        },
        Change {
            base: &'a str,
            local: &'a str,
            remote: &'a str,
        },
    }

    struct Case {
        local: String,
        base: String,
        remote: String,
        merged: String,
        conflicts: usize,
    }

    impl Case {
        fn outcome(&self) -> UnionOutcome {
            union_same_point_insertions(&UnionInput {
                local: &self.local,
                base: &self.base,
                remote: &self.remote,
                merged: &self.merged,
                reported_conflicts: self.conflicts,
            })
        }

        fn merged_text(&self) -> String {
            match self.outcome() {
                UnionOutcome::Merged { text, hunks } => {
                    assert_eq!(hunks, self.conflicts);
                    text
                }
                UnionOutcome::Refused(reason) => {
                    panic!("expected a union, got refusal: {reason}")
                }
            }
        }

        fn refusal(&self) -> String {
            match self.outcome() {
                UnionOutcome::Refused(reason) => reason,
                UnionOutcome::Merged { text, .. } => {
                    panic!("expected a refusal, got union:\n{text}")
                }
            }
        }
    }

    fn marker(ch: char, label: &str, eol: &str) -> String {
        format!("{}{label}{eol}", ch.to_string().repeat(MARKER_SIZE))
    }

    /// Build the three blobs and Git-shaped diff3 output for `pieces`.
    fn build(pieces: &[Piece<'_>], eol: &str) -> Case {
        let mut case = Case {
            local: String::new(),
            base: String::new(),
            remote: String::new(),
            merged: String::new(),
            conflicts: 0,
        };
        for piece in pieces {
            let (base, local, remote) = match piece {
                Piece::Clean(text) => {
                    for blob in [
                        &mut case.local,
                        &mut case.base,
                        &mut case.remote,
                        &mut case.merged,
                    ] {
                        blob.push_str(text);
                    }
                    continue;
                }
                Piece::Insert { local, remote } => ("", *local, *remote),
                Piece::Change {
                    base,
                    local,
                    remote,
                } => (*base, *local, *remote),
            };
            case.conflicts += 1;
            case.local.push_str(local);
            case.base.push_str(base);
            case.remote.push_str(remote);
            case.merged.push_str(&marker('<', " local", eol));
            case.merged.push_str(local);
            case.merged.push_str(&marker('|', " base", eol));
            case.merged.push_str(base);
            case.merged.push_str(&marker('=', "", eol));
            case.merged.push_str(remote);
            case.merged.push_str(&marker('>', " remote", eol));
        }
        case
    }

    const BOB_PREFIX: &str = "---\n\
        parent: \"[[dev]]\"\n\
        task_count: 59\n\
        ---\n\
        \n\
        - [ ] #task Every successor offer menu should allow you to add dependencies! [id:: bob__successor-deps]\n\
        \t- 🥀 **ROTTEN LOG**\n\
        \t\t- *2026-11-06* — 🍂 Next → Ready · after 2 keeps\n\
        \t\t- *2026-11-03* — 🤷 no reason given · kept 1× in Next\n";
    const BOB_LOCAL: &str = "- [*] #task Change parent project of `^gtd` daily task to [[gtd]]! [created::2026-11-07] ^gtd-parent-is-gtd\n\
        \t- 🪢 **LINKED FROM:** [[#^grey-gtd-daily]] (merged into)\n\
        \t- Make `gtd_daily` show as grey!\n\
        \t\t- IMPL: Add support for `color` property on area/project notes!\n";
    const BOB_REMOTE: &str = "- [ ] #task #ref [[ref/chat/gmail_thread_note_sync|How do tracked Gmail threads stay in sync with their Obsidian notes?]] [created::2026-11-07] ^ref-gmail-thread-note-sync\n";
    const NEXT_HEADING: &str =
        "\n### Next & In Progress\n\n- [/] #task Busy ^busy\n";

    #[test]
    fn bob_md_shape_unions_local_then_remote() {
        let case = build(
            &[
                Piece::Clean(BOB_PREFIX),
                Piece::Insert {
                    local: BOB_LOCAL,
                    remote: BOB_REMOTE,
                },
                Piece::Clean(NEXT_HEADING),
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            format!("{BOB_PREFIX}{BOB_LOCAL}{BOB_REMOTE}{NEXT_HEADING}")
        );
    }

    #[test]
    fn october_sase_md_shape_unions_one_line_each() {
        let prefix = "- [ ] #task Fill four cards! [h:: 25664e10] [created::2026-10-23]\n";
        let local = "- [ ] #task Use “Paste-ready block” for tasks?! [h:: 98b0c781a40faa60] [created::2026-10-23]\n";
        let remote = "- [ ] #task #ref [[ref/chat/sase_1n6_e2_stall_culprits|Who stalled the sase-1n6 epic?]] [created::2026-10-23] ^ref-sase-1n6-e2-stall-culprits\n";
        let case = build(
            &[
                Piece::Clean(prefix),
                Piece::Insert { local, remote },
                Piece::Clean(NEXT_HEADING),
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            format!("{prefix}{local}{remote}{NEXT_HEADING}")
        );
    }

    #[test]
    fn indented_remote_child_goes_before_local_top_level_task() {
        let prefix = "- [ ] #task Parent ^parent\n";
        let local = "- [ ] #task New top-level task\n";
        let remote = "\t- note under the parent\n";
        let case = build(
            &[
                Piece::Clean(prefix),
                Piece::Insert { local, remote },
                Piece::Clean("\n## Next\n"),
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            format!("{prefix}{remote}{local}\n## Next\n")
        );
    }

    #[test]
    fn several_hunks_each_pick_their_own_order() {
        let case = build(
            &[
                Piece::Clean("- [ ] #task A ^a\n"),
                Piece::Insert {
                    local: "- [ ] #task L1\n",
                    remote: "\t- child of A\n",
                },
                Piece::Clean("\n## Later\n\n"),
                Piece::Insert {
                    local: "- [ ] #task L2\n",
                    remote: "- [ ] #task R2\n",
                },
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            "- [ ] #task A ^a\n\t- child of A\n- [ ] #task L1\n\n## Later\n\n- [ ] #task L2\n- [ ] #task R2\n"
        );
    }

    #[test]
    fn refuses_when_neither_order_is_outline_safe() {
        let case = build(
            &[
                Piece::Clean("- [ ] #task Parent\n"),
                Piece::Insert {
                    local: "\t- local child\n- [ ] #task Local top\n",
                    remote: "\t- remote child\n- [ ] #task Remote top\n",
                },
            ],
            "\n",
        );
        assert!(case.refusal().contains("no outline-safe order"));
    }

    #[test]
    fn refuses_to_reparent_an_indented_trailing_line() {
        let case = build(
            &[
                Piece::Clean("- [ ] #task Parent\n"),
                Piece::Insert {
                    local: "- [ ] #task Local\n",
                    remote: "\t- remote child\n",
                },
                Piece::Clean("\n\t- existing child of Parent\n"),
            ],
            "\n",
        );
        assert!(case.refusal().contains("re-parent the indented line"));
    }

    #[test]
    fn allows_indented_trailing_line_when_both_sides_are_as_deep() {
        let case = build(
            &[
                Piece::Clean("- [ ] #task Parent\n"),
                Piece::Insert {
                    local: "\t- local child\n",
                    remote: "\t- remote child\n",
                },
                Piece::Clean("\t- existing child\n"),
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            "- [ ] #task Parent\n\t- local child\n\t- remote child\n\t- existing child\n"
        );
    }

    #[test]
    fn refuses_a_modify_modify_hunk() {
        let case = build(
            &[
                Piece::Clean("# Note\n"),
                Piece::Change {
                    base: "- [ ] #task Base\n",
                    local: "- [x] #task Base\n",
                    remote: "- [-] #task Base\n",
                },
            ],
            "\n",
        );
        assert!(case.refusal().contains("base side is not empty"));
    }

    #[test]
    fn refuses_a_hunk_inside_frontmatter() {
        let case = build(
            &[
                Piece::Clean("---\ntask_count: 1\n"),
                Piece::Insert {
                    local: "open_task_count: 1\n",
                    remote: "status: wip\n",
                },
                Piece::Clean("---\n\nbody\n"),
            ],
            "\n",
        );
        assert!(case.refusal().contains("inside the YAML frontmatter"));
    }

    #[test]
    fn refuses_a_hunk_that_inserts_frontmatter_at_line_one() {
        let case = build(
            &[
                Piece::Insert {
                    local: "---\nparent: \"[[a]]\"\n---\n",
                    remote: "# Title\n",
                },
                Piece::Clean("body\n"),
            ],
            "\n",
        );
        assert!(case.refusal().contains("inserts YAML frontmatter"));
    }

    #[test]
    fn hunk_after_closed_frontmatter_is_allowed() {
        let case = build(
            &[
                Piece::Clean("---\nparent: \"[[a]]\"\n---\n"),
                Piece::Insert {
                    local: "- [ ] #task L\n",
                    remote: "- [ ] #task R\n",
                },
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            "---\nparent: \"[[a]]\"\n---\n- [ ] #task L\n- [ ] #task R\n"
        );
    }

    #[test]
    fn refuses_an_unbalanced_code_fence() {
        let case = build(
            &[
                Piece::Clean("# Note\n"),
                Piece::Insert {
                    local: "```rust\nfn main() {}\n",
                    remote: "- [ ] #task R\n",
                },
            ],
            "\n",
        );
        assert!(case.refusal().contains("unbalanced code fence"));
    }

    #[test]
    fn balanced_fences_are_allowed() {
        let case = build(
            &[
                Piece::Clean("# Note\n"),
                Piece::Insert {
                    local: "~~~\ncode\n~~~\n",
                    remote: "- [ ] #task R\n",
                },
            ],
            "\n",
        );
        assert_eq!(
            case.merged_text(),
            "# Note\n~~~\ncode\n~~~\n- [ ] #task R\n"
        );
    }

    #[test]
    fn refuses_sides_sharing_an_identical_line() {
        let case = build(
            &[
                Piece::Clean("- [ ] **13:50** BUILD\n"),
                Piece::Insert {
                    local:
                        "\t- [[bob#^gtd-parent-is-gtd]]\n\t- [[bob#^other]]\n",
                    remote: "\t- [[bob#^gtd-parent-is-gtd]]\n",
                },
            ],
            "\n",
        );
        assert!(case.refusal().contains("share an identical line"));
    }

    #[test]
    fn refuses_a_duplicate_block_id() {
        let case = build(
            &[
                Piece::Clean("# Note\n"),
                Piece::Insert {
                    local: "- [ ] #task Local wording ^same-id\n",
                    remote: "- [ ] #task Remote wording ^same-id\n",
                },
            ],
            "\n",
        );
        assert!(case.refusal().contains("repeat block id ^same-id"));
    }

    #[test]
    fn refuses_marker_like_content() {
        let separator = format!("{}\n", "=".repeat(MARKER_SIZE));
        let case = build(
            &[
                Piece::Clean(&separator),
                Piece::Insert {
                    local: "- [ ] #task L\n",
                    remote: "- [ ] #task R\n",
                },
            ],
            "\n",
        );
        assert!(case.refusal().contains("looks like a merge marker"));
    }

    #[test]
    fn crlf_content_round_trips_unchanged() {
        let case = build(
            &[
                Piece::Clean(
                    "---\r\nparent: \"[[a]]\"\r\n---\r\n- [ ] #task A\r\n",
                ),
                Piece::Insert {
                    local: "- [ ] #task L ^l\r\n\t- child\r\n",
                    remote: "- [ ] #task R ^r\r\n",
                },
                Piece::Clean("\r\n## Next\r\n"),
            ],
            "\r\n",
        );
        assert_eq!(
            case.merged_text(),
            "---\r\nparent: \"[[a]]\"\r\n---\r\n- [ ] #task A\r\n- [ ] #task L ^l\r\n\t- child\r\n- [ ] #task R ^r\r\n\r\n## Next\r\n"
        );
    }

    #[test]
    fn refuses_malformed_or_unterminated_markers() {
        let mut case = build(
            &[
                Piece::Clean("a\n"),
                Piece::Insert {
                    local: "L\n",
                    remote: "R\n",
                },
            ],
            "\n",
        );
        let full = case.merged.clone();
        case.merged = full
            .lines()
            .take(4)
            .map(|line| format!("{line}\n"))
            .collect();
        assert!(case.refusal().contains("ends inside a conflict block"));

        case.merged = full.replace(" remote\n", " theirs\n");
        assert!(case.refusal().contains("malformed merge marker"));

        let missing_base = format!(
            "a\n{}L\n{}R\n{}",
            marker('<', " local", "\n"),
            marker('=', "", "\n"),
            marker('>', " remote", "\n")
        );
        case.merged = missing_base;
        assert!(case.refusal().contains("unexpected merge marker"));
    }

    #[test]
    fn refuses_when_parsed_hunks_disagree_with_git() {
        let mut case = build(
            &[
                Piece::Clean("a\n"),
                Piece::Insert {
                    local: "L\n",
                    remote: "R\n",
                },
            ],
            "\n",
        );
        case.conflicts = 2;
        assert!(case.refusal().contains("git reported 2"));
        case.conflicts = 0;
        assert!(case.refusal().contains("git reported 0"));
    }

    #[test]
    fn block_ids_and_indents_parse_as_documented() {
        assert_eq!(trailing_block_id("- [ ] #task A ^abc-1"), Some("abc-1"));
        assert_eq!(
            trailing_block_id("- [ ] #task A ^abc-1  \r"),
            Some("abc-1")
        );
        assert_eq!(trailing_block_id("^solo"), Some("solo"));
        assert_eq!(trailing_block_id("[[bob#^gtd-parent-is-gtd]]"), None);
        assert_eq!(trailing_block_id("x^glued"), None);
        assert_eq!(trailing_block_id("ends with ^"), None);
        assert_eq!(indent_width("\t\t- x"), 8);
        assert_eq!(indent_width("  \t- x"), 6);
        assert_eq!(indent_width("- x"), 0);
    }
}

//! Separator, badge-duplicate, and vendor-boilerplate filter.
//!
//! Runs inside [`super::filter_source_lines`] while blank and separator
//! lines are still present. Every pass drops whole lines only; prose is
//! never edited in place (design call 10: a vendor phrase inside an
//! ordinary agenda stays prose).
//!
//! Pass order:
//!
//! 1. Separators are fences and are always dropped.
//! 2. A fenced region (fence-to-fence, or fence-to-end) holding a primary
//!    vendor anchor drops the whole region including its fences.
//! 3. A header-anchored run (primary anchor, or secondary anchor once the
//!    description holds a primary anchor somewhere) drops lines until the
//!    next blank line or fence.
//! 4. A line that only repeats the event's `join_link` or `location` drops.
//! 5. When dropped vendor content held a join URL and the event has no
//!    `join_link` (no 📹 badge), one `Join … meeting` bullet survives.

use super::{DescriptionContext, Inline, SourceLine};

/// Drop boilerplate source lines, inserting at most one join-link bullet.
pub(super) fn filter(
    lines: Vec<SourceLine>,
    ctx: &DescriptionContext<'_>,
) -> Vec<SourceLine> {
    let count = lines.len();
    let fence: Vec<bool> = lines.iter().map(is_fence).collect();
    let blank: Vec<bool> =
        lines.iter().map(|line| line.kind_is_blank()).collect();
    let primary: Vec<bool> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            !blank[index] && !fence[index] && is_primary_anchor(line)
        })
        .collect();
    let has_primary = primary.iter().any(|flag| *flag);

    let mut drop = vec![false; count];
    let mut vendor_drop = vec![false; count];
    for (index, is_fence) in fence.iter().enumerate() {
        if *is_fence {
            drop[index] = true;
        }
    }
    drop_fenced_regions(&fence, &primary, &mut drop, &mut vendor_drop);
    drop_header_runs(
        &lines,
        &fence,
        &blank,
        &primary,
        has_primary,
        &mut drop,
        &mut vendor_drop,
    );
    for (index, line) in lines.iter().enumerate() {
        if !drop[index] && is_badge_duplicate(line, ctx) {
            drop[index] = true;
        }
    }

    let keep = if badge_absent(ctx) {
        first_vendor_join(&lines, &vendor_drop)
    } else {
        None
    };
    let Some((keep_url, keep_label)) = keep else {
        return lines
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !drop[*index])
            .map(|(_, line)| line)
            .collect();
    };
    let keep_at = vendor_drop.iter().position(|flag| *flag).unwrap_or(count);
    let keep_line = SourceLine::paragraph(vec![Inline::Link {
        text: keep_label.to_string(),
        url: keep_url,
    }]);
    let mut out = Vec::new();
    let mut inserted = false;
    for (index, line) in lines.into_iter().enumerate() {
        if drop[index] {
            continue;
        }
        if !inserted && index > keep_at {
            out.push(keep_line.clone());
            inserted = true;
        }
        out.push(line);
    }
    if !inserted {
        out.push(keep_line);
    }
    out
}

/// Drop fence-bounded regions holding a primary anchor, fences included.
fn drop_fenced_regions(
    fence: &[bool],
    primary: &[bool],
    drop: &mut [bool],
    vendor_drop: &mut [bool],
) {
    let fences: Vec<usize> = fence
        .iter()
        .enumerate()
        .filter(|(_, flag)| **flag)
        .map(|(index, _)| index)
        .collect();
    for pair in fences.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if (start + 1..end).any(|index| primary[index]) {
            for index in start..=end {
                drop[index] = true;
                vendor_drop[index] = true;
            }
        }
    }
    if let Some(&start) = fences.last()
        && (start + 1..fence.len()).any(|index| primary[index])
    {
        for index in start..fence.len() {
            drop[index] = true;
            vendor_drop[index] = true;
        }
    }
}

/// Drop header-anchored runs: a primary anchor line, or (once the
/// description holds a primary anchor) a secondary anchor line, through
/// to the next blank line or fence.
fn drop_header_runs(
    lines: &[SourceLine],
    fence: &[bool],
    blank: &[bool],
    primary: &[bool],
    has_primary: bool,
    drop: &mut [bool],
    vendor_drop: &mut [bool],
) {
    for (index, line) in lines.iter().enumerate() {
        if blank[index] || fence[index] {
            continue;
        }
        let anchor =
            primary[index] || (has_primary && is_secondary_anchor(line));
        if !anchor {
            continue;
        }
        let mut cursor = index;
        while cursor < lines.len() && !blank[cursor] && !fence[cursor] {
            drop[cursor] = true;
            vendor_drop[cursor] = true;
            cursor += 1;
        }
    }
}

/// A separator doubles as a fence: `<hr>`, a run of `-_=*~`, Google's
/// `-::~…::-` wall, or Webex's do-not-delete sentence.
fn is_fence(line: &SourceLine) -> bool {
    if line.kind_is_separator() {
        return true;
    }
    if line.kind_is_blank() {
        return false;
    }
    if line.kind_is_bullet_list_item() {
        // `- - -` tokenizes as a bullet whose body is `- -`.
        let body: String = line
            .project_text()
            .chars()
            .filter(|ch| *ch != ' ' && *ch != '\t')
            .collect();
        let body_len = body.chars().count();
        return !body.is_empty()
            && body_len + 1 >= 3
            && body.chars().all(is_separator_char);
    }
    if line.kind_is_list_item() {
        return false;
    }
    is_separator_text(&line.project_text())
}

fn is_separator_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed == "-- Do not delete or change any of the following text. --" {
        return true;
    }
    if trimmed.len() >= 6
        && trimmed.starts_with("-::")
        && trimmed.ends_with("::-")
        && trimmed
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch == '~')
    {
        return true;
    }
    let dense: String = trimmed
        .chars()
        .filter(|ch| *ch != ' ' && *ch != '\t')
        .collect();
    dense.chars().count() >= 3 && dense.chars().all(is_separator_char)
}

fn is_separator_char(ch: char) -> bool {
    matches!(ch, '-' | '_' | '=' | '*' | '~')
}

/// A primary anchor is a vendor header line or a line holding a vendor
/// join URL.
fn is_primary_anchor(line: &SourceLine) -> bool {
    if starts_with_any(&line.project_text(), PRIMARY_HEADERS) {
        return true;
    }
    line.inline_urls().any(|url| vendor_of(url).is_some())
}

/// Secondary anchors only start runs once the description holds a
/// primary anchor somewhere.
fn is_secondary_anchor(line: &SourceLine) -> bool {
    starts_with_any(&line.project_text(), SECONDARY_HEADERS)
}

fn starts_with_any(text: &str, anchors: &[&str]) -> bool {
    let trimmed = text.trim_start().to_ascii_lowercase();
    anchors.iter().any(|anchor| trimmed.starts_with(anchor))
}

const PRIMARY_HEADERS: &[&str] = &[
    "join with google meet",
    "join by phone",
    "more phone numbers",
    "join zoom meeting",
    "join by sip",
    "join by h.323",
    "join by skype for business",
    "microsoft teams meeting",
    "microsoft teams need help?",
    "join the meeting now",
    "join on your computer",
    "when it's time, join your webex meeting",
    "join by video system",
    "more ways to join",
];

const SECONDARY_HEADERS: &[&str] = &[
    "meeting id:",
    "passcode:",
    "pin:",
    "phone conference id:",
    "one tap mobile",
    "dial by your location",
    "find your local number",
    "or call in",
    "dial in by phone",
    "need help?",
    "meeting options",
    "access code:",
];

/// A line that only repeats the event's `join_link` or `location`: its
/// whole text equals one target, or it is a lone link to one target.
fn is_badge_duplicate(line: &SourceLine, ctx: &DescriptionContext<'_>) -> bool {
    let targets = [
        ctx.join_link.map(str::trim).filter(|link| !link.is_empty()),
        ctx.location
            .map(str::trim)
            .filter(|location| !location.is_empty()),
    ];
    if targets.iter().all(Option::is_none) {
        return false;
    }
    if targets
        .iter()
        .flatten()
        .any(|target| line.project_text().trim() == *target)
    {
        return true;
    }
    if let Some(only) = line.only_link_url() {
        let trimmed = only.trim();
        return targets.iter().flatten().any(|target| trimmed == *target);
    }
    false
}

fn badge_absent(ctx: &DescriptionContext<'_>) -> bool {
    ctx.join_link
        .map(str::trim)
        .is_none_or(|link| link.is_empty())
}

/// First vendor join URL in dropped vendor lines, with its keep label.
fn first_vendor_join(
    lines: &[SourceLine],
    vendor_drop: &[bool],
) -> Option<(String, &'static str)> {
    lines
        .iter()
        .zip(vendor_drop.iter())
        .filter(|(_, dropped)| **dropped)
        .flat_map(|(line, _)| line.inline_urls())
        .filter_map(|url| {
            vendor_of(url).map(|vendor| (keep_url(url), vendor.keep_label()))
        })
        .next()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vendor {
    Meet,
    Zoom,
    Teams,
    Webex,
}

impl Vendor {
    fn keep_label(self) -> &'static str {
        match self {
            Self::Meet => "Join Google Meet",
            Self::Zoom => "Join Zoom meeting",
            Self::Teams => "Join Teams meeting",
            Self::Webex => "Join Webex meeting",
        }
    }
}

/// Classify a link URL by vendor host, looking through Google's redirect
/// wrapper when present.
fn vendor_of(url: &str) -> Option<Vendor> {
    if let Some(target) = redirect_target(url)
        && let Some(vendor) = vendor_of_host(&target.to_ascii_lowercase())
    {
        return Some(vendor);
    }
    vendor_of_host(&url.to_ascii_lowercase())
}

fn vendor_of_host(lower: &str) -> Option<Vendor> {
    if lower.contains("meet.google.com/") {
        Some(Vendor::Meet)
    } else if lower.contains("zoom.us/j/") {
        Some(Vendor::Zoom)
    } else if lower.contains("teams.microsoft.com/l/meetup-join")
        || lower.contains("teams.live.com/meet")
    {
        Some(Vendor::Teams)
    } else if lower.contains("webex.com/") {
        Some(Vendor::Webex)
    } else {
        None
    }
}

/// Prefer the redirect target for the kept join link.
fn keep_url(url: &str) -> String {
    redirect_target(url).unwrap_or_else(|| url.trim().to_string())
}

/// Unwrap `https://www.google.com/url?q=<target>&…` to `<target>`.
fn redirect_target(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "google.com" && host != "www.google.com" {
        return None;
    }
    if parsed.path() != "/url" {
        return None;
    }
    parsed
        .query_pairs()
        .find(|(key, _)| key == "q")
        .map(|(_, value)| value.into_owned())
        .filter(|target| {
            target.starts_with("http://") || target.starts_with("https://")
        })
}

//! Publisher transcript parsing and paragraphing.
//!
//! [`parse_transcript`] turns one publisher transcript file (the bytes
//! behind a feed item's `<podcast:transcript>` URL) into a sanity-checked
//! [`Transcript`] of speaker-labelled paragraphs, or a [`TranscriptDrop`]
//! that says why the file was dropped. [`preferred_offer`] picks which of
//! an item's transcript files to download. Both are pure: no network, no
//! filesystem, no clock.
//!
//! - **Formats.** Podcasting 2.0 JSON (`segments[].startTime/endTime/
//!   speaker/body`), WebVTT, SRT, HTML (`<cite>` speaker, `<time>`,
//!   `<p>`), and plain text (blank-line paragraphs, no times), preferred
//!   in that order. A specific MIME type names the format; a generic one
//!   (`text/plain`, `application/octet-stream`, none) defers to the URL's
//!   extension.
//! - **Sanity checks.** A transcript under 200 words or over 120,000 words
//!   is dropped, and so is a timed one whose last cue starts before half
//!   of the known episode duration or after 110 % of it.
//! - **Speakers.** VTT voice tags, JSON speakers, and HTML `<cite>` labels
//!   are real names unless they are diarization tokens (`SPEAKER_00`,
//!   `Speaker 1`, `spk_0`, `S1`). Tokens become `Speaker 1`, `Speaker 2`,
//!   … in order of first appearance. With exactly two speakers, both
//!   diarized, one whose question rate (questions per word) is at least
//!   three times the other's and who asks at least ten questions is
//!   `Host` and the other `Guest`. Names are never guessed.
//! - **Paragraphs.** Consecutive same-speaker cues merge. A paragraph
//!   closes at the first sentence end once it holds 95 words and is
//!   forced closed at 160; a speaker change always closes it, and so does
//!   a plain-text blank line. Each paragraph keeps its first cue's start,
//!   except one that opens inside the cue that opened the paragraph
//!   before it: one long cue never stamps two paragraphs with one time.

use std::{collections::HashMap, fmt, sync::OnceLock};

use encoding_rs::{Encoding, UTF_8};
use regex::Regex;
use scraper::{node::Node, ElementRef, Html};
use serde::Serialize;

use super::{super::collection::format_count, clean_line, strip_invisibles};

/// A transcript under this many words is dropped.
const MIN_WORDS: usize = 200;

/// A transcript over this many words is dropped.
const MAX_WORDS: usize = 120_000;

/// A paragraph closes at the first sentence end once it holds this many
/// words.
const PARAGRAPH_SOFT_WORDS: usize = 95;

/// A paragraph is forced closed at this many words.
const PARAGRAPH_MAX_WORDS: usize = 160;

/// A host asks at least this many questions.
const MIN_HOST_QUESTIONS: usize = 10;

/// A host's question rate is at least this many times the guest's.
const HOST_RATE_FACTOR: usize = 3;

/// A transcript file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TranscriptFormat {
    /// Podcasting 2.0 JSON.
    Json,
    /// WebVTT.
    Vtt,
    /// SubRip.
    Srt,
    /// Podcasting 2.0 HTML.
    Html,
    /// Plain text.
    Text,
}

impl TranscriptFormat {
    /// Every format, most preferred first.
    pub(crate) const PREFERENCE: [Self; 5] =
        [Self::Json, Self::Vtt, Self::Srt, Self::Html, Self::Text];

    /// The payload spelling: `json`, `vtt`, `srt`, `html`, or `text`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Vtt => "vtt",
            Self::Srt => "srt",
            Self::Html => "html",
            Self::Text => "text",
        }
    }

    /// The reader-facing name, as in "Publisher transcript (VTT)".
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Vtt => "VTT",
            Self::Srt => "SRT",
            Self::Html => "HTML",
            Self::Text => "text",
        }
    }

    /// The format a transcript offer declares. A specific MIME type wins;
    /// a generic or missing one defers to the URL's file extension, and
    /// `text/plain` with no telling extension is plain text.
    pub(crate) fn detect(mime: &str, url: &str) -> Option<Self> {
        match Self::from_mime(mime) {
            Some(Self::Text) => Self::from_url(url).or(Some(Self::Text)),
            Some(format) => Some(format),
            None => Self::from_url(url),
        }
    }

    fn from_mime(mime: &str) -> Option<Self> {
        let essence = mime.split(';').next().unwrap_or("").trim();
        match essence.to_ascii_lowercase().as_str() {
            "application/json" | "text/json" => Some(Self::Json),
            "text/vtt" | "text/webvtt" => Some(Self::Vtt),
            "application/x-subrip"
            | "application/srt"
            | "text/srt"
            | "text/x-srt"
            | "application/x-srt" => Some(Self::Srt),
            "text/html" | "application/xhtml+xml" => Some(Self::Html),
            "text/plain" => Some(Self::Text),
            _ => None,
        }
    }

    fn from_url(url: &str) -> Option<Self> {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let name = path.rsplit('/').next().unwrap_or(path);
        let (_, extension) = name.rsplit_once('.')?;
        match extension.to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "vtt" => Some(Self::Vtt),
            "srt" => Some(Self::Srt),
            "html" | "htm" => Some(Self::Html),
            "txt" => Some(Self::Text),
            _ => None,
        }
    }

    fn rank(self) -> usize {
        Self::PREFERENCE
            .iter()
            .position(|format| *format == self)
            .unwrap_or(Self::PREFERENCE.len())
    }
}

/// A parsed, sanity-checked transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transcript {
    pub(crate) format: TranscriptFormat,
    pub(crate) paragraphs: Vec<Paragraph>,
    /// Words across every paragraph.
    pub(crate) words: usize,
    /// Two diarized speakers were labelled `Host` and `Guest`.
    pub(crate) roles_inferred: bool,
    /// At least one speaker label is a real name from the file.
    pub(crate) names_from_file: bool,
}

/// One transcript paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Paragraph {
    /// Where its first cue starts, in whole seconds; `None` when the
    /// file carries no time for it.
    pub(crate) start_secs: Option<u32>,
    /// `None` when the file names no speaker.
    pub(crate) speaker: Option<String>,
    pub(crate) text: String,
}

/// Why a transcript file was dropped. Its `Display` is the text after
/// `transcript dropped: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptDrop {
    /// Neither the MIME type nor the URL names a transcript format.
    Unsupported {
        mime: String,
    },
    /// The bytes do not decode as text.
    NotText,
    /// The file is not its declared format; `detail` completes "the
    /// <format> transcript …".
    Malformed {
        format: TranscriptFormat,
        detail: &'static str,
    },
    /// The file parsed, but holds no words.
    Empty {
        format: TranscriptFormat,
    },
    TooShort {
        words: usize,
    },
    TooLong {
        words: usize,
    },
    /// The last cue starts before half of the episode.
    EndsEarly {
        last_start_secs: u32,
        duration_secs: u32,
    },
    /// The last cue starts after 110 % of the episode.
    EndsLate {
        last_start_secs: u32,
        duration_secs: u32,
    },
}

impl fmt::Display for TranscriptDrop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { mime } if mime.is_empty() => {
                f.write_str("the file names no transcript format")
            }
            Self::Unsupported { mime } => {
                write!(f, "unsupported transcript type {mime}")
            }
            Self::NotText => f.write_str("the file is not text"),
            Self::Malformed { format, detail } => {
                write!(f, "the {} transcript {detail}", format.label())
            }
            Self::Empty { format } => {
                write!(f, "the {} transcript holds no text", format.label())
            }
            Self::TooShort { words } => write!(
                f,
                "only {} words (fewer than {})",
                format_count(*words as u64),
                format_count(MIN_WORDS as u64)
            ),
            Self::TooLong { words } => write!(
                f,
                "{} words (more than {})",
                format_count(*words as u64),
                format_count(MAX_WORDS as u64)
            ),
            Self::EndsEarly {
                last_start_secs,
                duration_secs,
            } => write!(
                f,
                "the last cue starts at {}, before half of the {} episode",
                format_clock(*last_start_secs),
                format_clock(*duration_secs)
            ),
            Self::EndsLate {
                last_start_secs,
                duration_secs,
            } => write!(
                f,
                "the last cue starts at {}, past 110% of the {} episode",
                format_clock(*last_start_secs),
                format_clock(*duration_secs)
            ),
        }
    }
}

/// `m:ss` under an hour, else `h:mm:ss`.
pub(crate) fn format_clock(secs: u32) -> String {
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The most preferred offer whose format is known, with that format;
/// among offers of one format, the first. `mime_and_url` reads an
/// offer's declared MIME type and URL.
pub(crate) fn preferred_offer<T>(
    offers: &[T],
    mime_and_url: impl Fn(&T) -> (&str, &str),
) -> Option<(&T, TranscriptFormat)> {
    offers
        .iter()
        .filter_map(|offer| {
            let (mime, url) = mime_and_url(offer);
            TranscriptFormat::detect(mime, url).map(|format| (offer, format))
        })
        .min_by_key(|(_, format)| format.rank())
}

/// Parse and sanity-check one transcript file. `mime` and `url` are the
/// offer's declared type and location; `duration_ms` is the episode's
/// length, when known.
pub(crate) fn parse_transcript(
    bytes: &[u8],
    mime: &str,
    url: &str,
    duration_ms: Option<u64>,
) -> Result<Transcript, TranscriptDrop> {
    let format = TranscriptFormat::detect(mime, url).ok_or_else(|| {
        TranscriptDrop::Unsupported {
            mime: mime.trim().to_string(),
        }
    })?;
    let text = decode(bytes, mime)?;
    let cues = match format {
        TranscriptFormat::Json => parse_json(&text)?,
        TranscriptFormat::Vtt | TranscriptFormat::Srt => {
            parse_cue_blocks(&text)
        }
        TranscriptFormat::Html => parse_html(&text),
        TranscriptFormat::Text => parse_text(&text)?,
    };
    let speakers = Speakers::label(&cues);
    let paragraphs = build_paragraphs(&cues, &speakers);
    let words: usize = paragraphs.iter().map(|p| count_words(&p.text)).sum();
    if words == 0 {
        return Err(TranscriptDrop::Empty { format });
    }
    if words < MIN_WORDS {
        return Err(TranscriptDrop::TooShort { words });
    }
    if words > MAX_WORDS {
        return Err(TranscriptDrop::TooLong { words });
    }
    check_last_cue(&cues, duration_ms)?;
    Ok(Transcript {
        format,
        paragraphs,
        words,
        roles_inferred: speakers.roles_inferred,
        names_from_file: speakers.names_from_file,
    })
}

/// A timed transcript's last cue must start within [50 %, 110 %] of a
/// known episode duration.
fn check_last_cue(
    cues: &[Cue],
    duration_ms: Option<u64>,
) -> Result<(), TranscriptDrop> {
    let Some(duration_ms) = duration_ms.filter(|ms| *ms > 0) else {
        return Ok(());
    };
    let Some(last_start) =
        cues.iter().filter_map(|cue| cue.start).reduce(f64::max)
    else {
        return Ok(());
    };
    let duration = duration_ms as f64 / 1000.0;
    let last_start_secs = whole_secs(last_start);
    let duration_secs = whole_secs(duration);
    if last_start < duration * 0.5 {
        return Err(TranscriptDrop::EndsEarly {
            last_start_secs,
            duration_secs,
        });
    }
    if last_start > duration * 1.1 {
        return Err(TranscriptDrop::EndsLate {
            last_start_secs,
            duration_secs,
        });
    }
    Ok(())
}

fn whole_secs(secs: f64) -> u32 {
    // `as` saturates, so a huge time cannot wrap.
    secs.max(0.0).floor() as u32
}

// ---------------------------------------------------------------------
// Decoding.
// ---------------------------------------------------------------------

/// Decode `bytes` by BOM, else the MIME `charset`, else UTF-8, with line
/// endings normalized to `\n`. Bytes with a NUL or more than 1 %
/// undecodable characters are not text.
fn decode(bytes: &[u8], mime: &str) -> Result<String, TranscriptDrop> {
    let (encoding, body) = match Encoding::for_bom(bytes) {
        Some((encoding, bom_len)) => (encoding, &bytes[bom_len..]),
        None => (charset(mime).unwrap_or(UTF_8), bytes),
    };
    let (text, _) = encoding.decode_without_bom_handling(body);
    let chars = text.chars().count();
    let replaced = text.chars().filter(|c| *c == '\u{FFFD}').count();
    if text.contains('\0') || replaced * 100 > chars {
        return Err(TranscriptDrop::NotText);
    }
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn charset(mime: &str) -> Option<&'static Encoding> {
    mime.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        Encoding::for_label(value.trim().trim_matches('"').as_bytes())
    })
}

// ---------------------------------------------------------------------
// Cues.
// ---------------------------------------------------------------------

/// One timed (or untimed) run of text from the file.
#[derive(Debug, Clone, PartialEq)]
struct Cue {
    /// Seconds from the episode start.
    start: Option<f64>,
    /// The raw speaker label from the file.
    speaker: Option<String>,
    text: String,
    /// The file breaks a paragraph before this cue (a plain-text blank
    /// line).
    breaks_before: bool,
}

impl Cue {
    fn new(start: Option<f64>, speaker: Option<String>, text: String) -> Self {
        Self {
            start,
            speaker,
            text,
            breaks_before: false,
        }
    }
}

/// `[h:]m:ss[.fff]`, with `.` or `,` before the fraction.
fn parse_clock(text: &str) -> Option<f64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^(?:(\d+):)?(\d{1,2}):(\d{2})(?:[.,](\d{1,9}))?$")
            .expect("clock regex")
    });
    let caps = re.captures(text.trim())?;
    let number = |index: usize| -> f64 {
        caps.get(index)
            .and_then(|m| m.as_str().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let seconds = number(3);
    if seconds >= 60.0 {
        return None;
    }
    let fraction = caps
        .get(4)
        .and_then(|m| format!("0.{}", m.as_str()).parse::<f64>().ok())
        .unwrap_or(0.0);
    Some(number(1) * 3600.0 + number(2) * 60.0 + seconds + fraction)
}

// ---------------------------------------------------------------------
// Podcasting 2.0 JSON.
// ---------------------------------------------------------------------

fn parse_json(text: &str) -> Result<Vec<Cue>, TranscriptDrop> {
    let malformed = |detail| TranscriptDrop::Malformed {
        format: TranscriptFormat::Json,
        detail,
    };
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| malformed("is not valid JSON"))?;
    let segments = value
        .get("segments")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| malformed("has no segments list"))?;
    Ok(segments
        .iter()
        .filter_map(|segment| {
            let body = segment.get("body")?.as_str()?;
            let start = segment.get("startTime").and_then(json_seconds);
            let speaker = segment
                .get("speaker")
                .and_then(serde_json::Value::as_str)
                .and_then(clean_line);
            Some(Cue::new(start, speaker, strip_invisibles(body)))
        })
        .collect())
}

/// A JSON time: seconds as a number or a string, or a clock string.
fn json_seconds(value: &serde_json::Value) -> Option<f64> {
    let secs = match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .or_else(|| parse_clock(text)),
        _ => None,
    }?;
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

// ---------------------------------------------------------------------
// WebVTT and SRT.
// ---------------------------------------------------------------------

/// Parse WebVTT or SRT cue blocks. The two share a shape: blocks
/// separated by blank lines, each an optional id line, a `start --> end`
/// timing line (VTT may add cue settings; SRT may add coordinates), and
/// payload lines. A VTT header block, NOTE, STYLE, and REGION blocks, and
/// blocks without a parseable timing line are skipped.
fn parse_cue_blocks(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    for (index, block) in blocks(text).into_iter().enumerate() {
        let mut lines = block.as_slice();
        if index == 0 && lines[0].starts_with("WEBVTT") {
            // The header and its metadata lines; a cue glued to the
            // header without a blank line still parses below.
            lines = &lines[1..];
        }
        let Some(first) = lines.first() else {
            continue;
        };
        if ["NOTE", "STYLE", "REGION"]
            .iter()
            .any(|keyword| is_block_keyword(first, keyword))
        {
            continue;
        }
        let Some(timing) = lines.iter().position(|line| line.contains("-->"))
        else {
            continue;
        };
        let Some(start) =
            lines[timing].split("-->").next().and_then(parse_clock)
        else {
            continue;
        };
        cues.extend(payload_cues(&lines[timing + 1..].join("\n"), start));
    }
    cues
}

/// Nonblank line runs.
fn blocks(text: &str) -> Vec<Vec<&str>> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line.trim_start_matches('\u{FEFF}'));
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// `line` is `keyword` alone or followed by whitespace.
fn is_block_keyword(line: &str, keyword: &str) -> bool {
    line.strip_prefix(keyword)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// Split one cue payload at its voice spans. `<v Name>` (or
/// `<v.class Name>`) opens a voice, `</v>` closes it; every other tag
/// (`<c>`, `<i>`, `<b>`, `<u>`, `<lang>`, `<ruby>`, timestamps, SRT
/// `<font>`) is dropped with its text kept, except `<rt>` ruby text, which
/// is dropped whole. SRT `{\…}` overrides are dropped, and entities are
/// decoded.
fn payload_cues(payload: &str, start: f64) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut voice: Option<String> = None;
    let mut text = String::new();
    let mut ruby_text_depth = 0usize;
    let mut flush = |voice: &Option<String>, text: &mut String| {
        let decoded = strip_invisibles(&decode_entities(text));
        text.clear();
        if !decoded.trim().is_empty() {
            cues.push(Cue::new(Some(start), voice.clone(), decoded));
        }
    };
    let mut rest = payload;
    while !rest.is_empty() {
        if let Some((tag, after)) = leading_tag(rest) {
            rest = after;
            let name = tag
                .split(['.', ' ', '\t'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            match name.as_str() {
                "v" => {
                    flush(&voice, &mut text);
                    voice =
                        tag.split_once([' ', '\t']).and_then(|(_, name)| {
                            clean_line(&decode_entities(name))
                        });
                }
                "/v" => {
                    flush(&voice, &mut text);
                    voice = None;
                }
                "rt" => ruby_text_depth += 1,
                "/rt" => ruby_text_depth = ruby_text_depth.saturating_sub(1),
                _ => {}
            }
            continue;
        }
        if rest.starts_with("{\\")
            && let Some(end) = rest.find('}')
        {
            rest = &rest[end + 1..];
            continue;
        }
        let next = rest
            .char_indices()
            .skip(1)
            .find(|(_, c)| matches!(c, '<' | '{'))
            .map_or(rest.len(), |(index, _)| index);
        if ruby_text_depth == 0 {
            text.push_str(&rest[..next]);
        }
        rest = &rest[next..];
    }
    flush(&voice, &mut text);
    cues
}

/// A `<tag>` at the start of `text`: its inside and what follows it. A
/// `<` not followed by a letter, digit, or `/` is literal text.
fn leading_tag(text: &str) -> Option<(&str, &str)> {
    let inside = text.strip_prefix('<')?;
    if !inside
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '/')
    {
        return None;
    }
    let end = inside.find('>')?;
    Some((inside[..end].trim(), &inside[end + 1..]))
}

/// Decode the character references cue text uses: `&amp;`, `&lt;`,
/// `&gt;`, `&quot;`, `&apos;`, `&nbsp;`, `&lrm;`/`&rlm;` (dropped), and
/// numeric references. Anything else stays literal.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let reference = after
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| Some((entity(&after[..end])?, end)));
        match reference {
            Some((decoded, end)) => {
                out.extend(decoded);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// One reference's character; `Some(None)` for a mark that is dropped.
fn entity(name: &str) -> Option<Option<char>> {
    let named = match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        "lrm" | "rlm" => None,
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            return char::from_u32(code).map(Some);
        }
    };
    Some(named)
}

// ---------------------------------------------------------------------
// Podcasting 2.0 HTML.
// ---------------------------------------------------------------------

/// Walk the document in order: `<cite>` sets the speaker (a trailing `:`
/// dropped), `<time>` sets the next paragraph's start, and each `<p>`
/// becomes a cue.
fn parse_html(text: &str) -> Vec<Cue> {
    let document = Html::parse_document(text);
    let mut walk = HtmlWalk::default();
    walk.children(document.root_element());
    walk.cues
}

#[derive(Default)]
struct HtmlWalk {
    speaker: Option<String>,
    start: Option<f64>,
    cues: Vec<Cue>,
}

impl HtmlWalk {
    fn children(&mut self, element: ElementRef<'_>) {
        for child in element.children().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "cite" => self.speaker = cite_label(child),
                "time" => self.start = time_of(child),
                "p" => self.paragraph(child),
                "head" | "script" | "style" | "template" => {}
                _ => self.children(child),
            }
        }
    }

    fn paragraph(&mut self, element: ElementRef<'_>) {
        let mut text = String::new();
        self.collect_text(element, &mut text);
        if !text.trim().is_empty() {
            self.cues.push(Cue::new(
                self.start.take(),
                self.speaker.clone(),
                strip_invisibles(&text),
            ));
        }
    }

    /// The paragraph's text; a `<cite>` or `<time>` inside it updates the
    /// walk instead of joining the text.
    fn collect_text(&mut self, element: ElementRef<'_>, out: &mut String) {
        for child in element.children() {
            match child.value() {
                Node::Text(text) => out.push_str(text),
                Node::Element(_) => {
                    let Some(child) = ElementRef::wrap(child) else {
                        continue;
                    };
                    match child.value().name() {
                        "cite" => self.speaker = cite_label(child),
                        "time" => self.start = time_of(child),
                        "br" => out.push(' '),
                        "script" | "style" => {}
                        _ => self.collect_text(child, out),
                    }
                }
                _ => {}
            }
        }
    }
}

fn cite_label(element: ElementRef<'_>) -> Option<String> {
    let text = clean_line(&element.text().collect::<String>())?;
    clean_line(text.trim_end_matches(':'))
}

fn time_of(element: ElementRef<'_>) -> Option<f64> {
    parse_clock(&element.text().collect::<String>())
        .or_else(|| element.value().attr("datetime").and_then(parse_clock))
}

// ---------------------------------------------------------------------
// Plain text.
// ---------------------------------------------------------------------

/// Each blank-line paragraph is one untimed cue that breaks the
/// paragraph before it.
fn parse_text(text: &str) -> Result<Vec<Cue>, TranscriptDrop> {
    let head: String = text.trim_start().chars().take(15).collect();
    let head = head.to_ascii_lowercase();
    if head.starts_with("<!doctype html") || head.starts_with("<html") {
        return Err(TranscriptDrop::Malformed {
            format: TranscriptFormat::Text,
            detail: "is an HTML page",
        });
    }
    Ok(blocks(text)
        .into_iter()
        .map(|lines| Cue {
            breaks_before: true,
            ..Cue::new(None, None, strip_invisibles(&lines.join(" ")))
        })
        .collect())
}

// ---------------------------------------------------------------------
// Speakers.
// ---------------------------------------------------------------------

/// How each raw speaker label reads in the transcript.
struct Speakers {
    display: HashMap<String, String>,
    roles_inferred: bool,
    names_from_file: bool,
}

impl Speakers {
    fn label(cues: &[Cue]) -> Self {
        let mut order: Vec<&str> = Vec::new();
        for speaker in cues.iter().filter_map(|cue| cue.speaker.as_deref()) {
            if !order.contains(&speaker) {
                order.push(speaker);
            }
        }
        let mut display = HashMap::new();
        let mut tokens = 0;
        for raw in &order {
            let shown = if is_diarization_token(raw) {
                tokens += 1;
                format!("Speaker {tokens}")
            } else {
                (*raw).to_string()
            };
            display.insert((*raw).to_string(), shown);
        }
        let names_from_file = tokens < order.len();
        let mut roles_inferred = false;
        if let [first, second] = order.as_slice()
            && tokens == 2
            && let Some(host) =
                host_of([*first, *second].map(|raw| question_stats(cues, raw)))
        {
            let (host, guest) = if host == 0 {
                (first, second)
            } else {
                (second, first)
            };
            display.insert((*host).to_string(), "Host".to_string());
            display.insert((*guest).to_string(), "Guest".to_string());
            roles_inferred = true;
        }
        Self {
            display,
            roles_inferred,
            names_from_file,
        }
    }

    fn display(&self, raw: Option<&str>) -> Option<String> {
        raw.map(|raw| {
            self.display
                .get(raw)
                .cloned()
                .unwrap_or_else(|| raw.to_string())
        })
    }
}

/// `SPEAKER_00`, `Speaker 1`, `speaker-a`, `spk_0`, `S1`, … : a
/// diarizer's placeholder, never a name.
fn is_diarization_token(label: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:speaker[ _-]?(?:\d+|[a-z])|spk[ _-]?\d+|s\d+)$")
            .expect("diarization token regex")
    })
    .is_match(label.trim())
}

/// A speaker's `(questions, words)`.
fn question_stats(cues: &[Cue], raw: &str) -> (usize, usize) {
    cues.iter()
        .filter(|cue| cue.speaker.as_deref() == Some(raw))
        .fold((0, 0), |(questions, words), cue| {
            (
                questions + count_questions(&cue.text),
                words + count_words(&cue.text),
            )
        })
}

/// Which of two speakers is the host: at least [`MIN_HOST_QUESTIONS`]
/// questions and a question rate at least [`HOST_RATE_FACTOR`] times
/// the other's.
fn host_of(stats: [(usize, usize); 2]) -> Option<usize> {
    [(0, 1), (1, 0)].into_iter().find_map(|(a, b)| {
        let ((qa, wa), (qb, wb)) = (stats[a], stats[b]);
        // qa / wa >= 3 * qb / wb, without dividing.
        (wa > 0
            && wb > 0
            && qa >= MIN_HOST_QUESTIONS
            && qa * wb >= HOST_RATE_FACTOR * qb * wa)
            .then_some(a)
    })
}

/// Runs of `?` (a `??` or `?!?` asks once).
fn count_questions(text: &str) -> usize {
    let mut count = 0;
    let mut in_run = false;
    for c in text.chars() {
        let question = matches!(c, '?' | '？');
        if question && !in_run {
            count += 1;
        }
        in_run = question || (in_run && c == '!');
    }
    count
}

// ---------------------------------------------------------------------
// Paragraphs.
// ---------------------------------------------------------------------

/// A whitespace token with at least one letter or digit.
fn is_word(token: &str) -> bool {
    token.chars().any(char::is_alphanumeric)
}

fn count_words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|token| is_word(token))
        .count()
}

/// The token ends a sentence: a final `.`, `?`, `!`, or `…` (closing
/// quotes and brackets aside) that is not an abbreviation's dot.
fn ends_sentence(token: &str) -> bool {
    let trimmed = token.trim_end_matches(['"', '\'', '”', '’', ')', ']', '»']);
    match trimmed.chars().last() {
        Some('?' | '!' | '…' | '？' | '！' | '。') => true,
        Some('.') => !is_abbreviation(trimmed),
        _ => false,
    }
}

fn is_abbreviation(token: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:mr|mrs|ms|dr|prof|st|jr|sr|vs|(?:[a-z]\.)+[a-z])\.$",
        )
        .expect("abbreviation regex")
    });
    re.is_match(token.trim_start_matches(['"', '\'', '“', '‘', '(', '[']))
}

/// The paragraph being filled.
struct OpenParagraph<'a> {
    start: Option<f64>,
    speaker: Option<String>,
    tokens: Vec<&'a str>,
    words: usize,
}

impl OpenParagraph<'_> {
    fn finish(self) -> Option<Paragraph> {
        (self.words > 0).then(|| Paragraph {
            start_secs: self.start.map(whole_secs),
            speaker: self.speaker,
            text: self.tokens.join(" "),
        })
    }
}

/// Merge cues into paragraphs. A paragraph that opens inside the cue
/// that opened the one before it (a long cue split at a sentence end)
/// has no start of its own, so one cue never stamps two paragraphs.
fn build_paragraphs(cues: &[Cue], speakers: &Speakers) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    let mut open: Option<OpenParagraph<'_>> = None;
    let mut last_opening_cue = None;
    for (index, cue) in cues.iter().enumerate() {
        let speaker = speakers.display(cue.speaker.as_deref());
        if open
            .as_ref()
            .is_some_and(|open| cue.breaks_before || open.speaker != speaker)
        {
            paragraphs.extend(open.take().and_then(OpenParagraph::finish));
        }
        for token in cue.text.split_whitespace() {
            let paragraph = open.get_or_insert_with(|| {
                let reopened = last_opening_cue.replace(index) == Some(index);
                OpenParagraph {
                    start: if reopened { None } else { cue.start },
                    speaker: speaker.clone(),
                    tokens: Vec::new(),
                    words: 0,
                }
            });
            paragraph.tokens.push(token);
            if is_word(token) {
                paragraph.words += 1;
            }
            if paragraph.words >= PARAGRAPH_MAX_WORDS
                || (paragraph.words >= PARAGRAPH_SOFT_WORDS
                    && ends_sentence(token))
            {
                paragraphs.extend(open.take().and_then(OpenParagraph::finish));
            }
        }
    }
    paragraphs.extend(open.and_then(OpenParagraph::finish));
    paragraphs
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_VTT: &str = include_str!(
        "../../../../tests/fixtures/podcast/pragmatic_engineer_transcript.vtt"
    );
    const SRT: &str =
        include_str!("../../../../tests/fixtures/podcast/transcript.srt");
    const JSON: &str =
        include_str!("../../../../tests/fixtures/podcast/transcript.json");
    const HTML: &str =
        include_str!("../../../../tests/fixtures/podcast/transcript.html");
    const TEXT: &str =
        include_str!("../../../../tests/fixtures/podcast/transcript.txt");

    /// The full episode the trimmed fixtures come from (1:26:37).
    const EPISODE_MS: u64 = 5_197_032;

    /// A duration the trimmed fixtures fit: their last cue starts at
    /// 2:58.
    const TRIMMED_MS: u64 = 190_000;

    fn parse(
        body: &str,
        mime: &str,
        duration_ms: Option<u64>,
    ) -> Result<Transcript, TranscriptDrop> {
        parse_transcript(body.as_bytes(), mime, "", duration_ms)
    }

    fn speakers(transcript: &Transcript) -> Vec<Option<&str>> {
        transcript
            .paragraphs
            .iter()
            .map(|p| p.speaker.as_deref())
            .collect()
    }

    /// Every paragraph holds 1–160 words; one that ends while its
    /// speaker keeps talking closes at a sentence end after 95 words or
    /// at exactly 160.
    fn assert_paragraph_bounds(transcript: &Transcript) {
        let paragraphs = &transcript.paragraphs;
        for (index, paragraph) in paragraphs.iter().enumerate() {
            let words = count_words(&paragraph.text);
            assert!(
                (1..=PARAGRAPH_MAX_WORDS).contains(&words),
                "{words} words: {}",
                paragraph.text
            );
            let continues = paragraphs.get(index + 1).is_some_and(|next| {
                next.speaker == paragraph.speaker
                    && transcript.format != TranscriptFormat::Text
            });
            if continues {
                let last = paragraph.text.split_whitespace().last().unwrap();
                assert!(
                    words == PARAGRAPH_MAX_WORDS
                        || (words >= PARAGRAPH_SOFT_WORDS
                            && ends_sentence(last)),
                    "{words} words: {}",
                    paragraph.text
                );
            }
        }
        let total: usize =
            paragraphs.iter().map(|p| count_words(&p.text)).sum();
        assert_eq!(total, transcript.words);
    }

    /// A VTT file from `(start_secs, voice, text)` cues.
    fn vtt_of(cues: &[(u32, Option<&str>, &str)]) -> String {
        let mut out = String::from("WEBVTT\n");
        for (start, voice, text) in cues {
            let voice = voice.map(|v| format!("<v {v}>")).unwrap_or_default();
            out.push_str(&format!(
                "\n{} --> {}\n{voice}{text}\n",
                vtt_clock(*start),
                vtt_clock(start + 1)
            ));
        }
        out
    }

    fn vtt_clock(secs: u32) -> String {
        format!(
            "{:02}:{:02}:{:02}.000",
            secs / 3600,
            secs / 60 % 60,
            secs % 60
        )
    }

    /// `count` words with no sentence end.
    fn filler(count: usize) -> String {
        vec!["word"; count].join(" ")
    }

    // -----------------------------------------------------------------
    // The real Pragmatic Engineer VTT.
    // -----------------------------------------------------------------

    #[test]
    fn the_real_vtt_becomes_diarized_speaker_paragraphs() {
        let transcript =
            parse(REAL_VTT, "text/vtt", Some(TRIMMED_MS)).expect("parses");
        assert_eq!(transcript.format, TranscriptFormat::Vtt);
        assert_eq!(transcript.words, 603);
        assert!(!transcript.names_from_file);
        // Speaker 1 asks 7 questions here: too few to call a host.
        assert!(!transcript.roles_inferred);
        assert_paragraph_bounds(&transcript);
        assert_eq!(transcript.paragraphs.len(), 11);

        let first = &transcript.paragraphs[0];
        assert_eq!(first.start_secs, Some(0));
        assert_eq!(first.speaker.as_deref(), Some("Speaker 1"));
        assert!(first.text.starts_with(
            "What does it look like when a startup CEO merges 5,000 pull \
             requests in a single month thanks to going all-in"
        ));
        assert!(first.text.ends_with("and many more."));
        assert_eq!(count_words(&first.text), 133);

        // SPEAKER_00 speaks second, so it is Speaker 2.
        let guest = &transcript.paragraphs[5];
        assert_eq!(guest.start_secs, Some(143));
        assert_eq!(guest.speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(
            guest.text,
            "Harjit welcome to the podcast. Hey thanks for having me here."
        );

        // Cue text is whitespace-collapsed.
        assert!(transcript.paragraphs[2].text.contains(
            "The result is a much better search than what Substack has."
        ));
        assert!(transcript
            .paragraphs
            .iter()
            .all(|p| !p.text.contains("SPEAKER_") && !p.text.contains('<')));
        let last = transcript.paragraphs.last().unwrap();
        assert_eq!(last.start_secs, Some(168));
        assert!(last.text.ends_with("the usual stuff, right?"));
    }

    #[test]
    fn the_trimmed_vtt_is_dropped_against_the_full_episode() {
        let drop = parse(REAL_VTT, "text/vtt", Some(EPISODE_MS)).unwrap_err();
        assert_eq!(
            drop,
            TranscriptDrop::EndsEarly {
                last_start_secs: 178,
                duration_secs: 5197,
            }
        );
        assert_eq!(
            drop.to_string(),
            "the last cue starts at 2:58, before half of the 1:26:37 episode"
        );
        // Without a known duration there is nothing to check.
        assert!(parse(REAL_VTT, "text/vtt", None).is_ok());
    }

    // -----------------------------------------------------------------
    // The other formats.
    // -----------------------------------------------------------------

    #[test]
    fn srt_with_crlf_and_no_speakers_parses() {
        assert!(SRT.contains("\r\n"));
        // The SRT holds the first 45 cues; its last starts at 2:16.
        let transcript =
            parse(SRT, "application/x-subrip", Some(150_000)).expect("parses");
        assert_eq!(transcript.format, TranscriptFormat::Srt);
        assert_eq!(transcript.words, 488);
        assert!(!transcript.names_from_file && !transcript.roles_inferred);
        assert!(transcript.paragraphs.iter().all(|p| p.speaker.is_none()));
        assert_paragraph_bounds(&transcript);
        let starts: Vec<_> =
            transcript.paragraphs.iter().map(|p| p.start_secs).collect();
        assert_eq!(starts, [Some(0), Some(36), Some(71), Some(102), Some(132)]);
    }

    #[test]
    fn json_segments_with_diarized_speakers_parse() {
        let transcript =
            parse(JSON, "application/json", Some(TRIMMED_MS)).expect("parses");
        assert_eq!(transcript.format, TranscriptFormat::Json);
        assert_eq!(transcript.words, 603);
        assert!(!transcript.names_from_file && !transcript.roles_inferred);
        assert_paragraph_bounds(&transcript);
        // spk_1 speaks first.
        assert_eq!(
            speakers(&transcript)[4..8],
            [
                Some("Speaker 1"),
                Some("Speaker 2"),
                Some("Speaker 1"),
                Some("Speaker 2")
            ]
        );
        assert_eq!(transcript.paragraphs[5].start_secs, Some(143));
    }

    #[test]
    fn json_speakers_with_real_names_and_string_times_parse() {
        let segments: Vec<_> = (0..30)
            .map(|i| {
                serde_json::json!({
                    "speaker": if i % 2 == 0 { "Alban Brooke" } else { "Dave Jones" },
                    "startTime": format!("{}.5", i * 10),
                    "endTime": i * 10 + 9,
                    "body": format!("{} sentence {i}.", filler(8)),
                })
            })
            .collect();
        let body =
            serde_json::json!({ "version": "1.0.0", "segments": segments })
                .to_string();
        let transcript =
            parse(&body, "application/json", None).expect("parses");
        assert!(transcript.names_from_file);
        assert!(!transcript.roles_inferred);
        assert_eq!(transcript.paragraphs.len(), 30);
        assert_eq!(transcript.paragraphs[1].start_secs, Some(10));
        assert_eq!(
            transcript.paragraphs[1].speaker.as_deref(),
            Some("Dave Jones")
        );
    }

    #[test]
    fn html_cites_name_speakers_and_times_stamp_paragraphs() {
        let transcript =
            parse(HTML, "text/html", Some(TRIMMED_MS)).expect("parses");
        assert_eq!(transcript.format, TranscriptFormat::Html);
        assert_eq!(transcript.words, 603);
        assert!(transcript.names_from_file);
        assert!(!transcript.roles_inferred);
        assert_paragraph_bounds(&transcript);
        assert_eq!(
            transcript.paragraphs[0].speaker.as_deref(),
            Some("Gergely Orosz")
        );
        assert_eq!(
            transcript.paragraphs[5].speaker.as_deref(),
            Some("Harjot Gill")
        );
        // The opening turn is one long <p>: only its first paragraph
        // carries the turn's time.
        let starts: Vec<_> = transcript.paragraphs[..7]
            .iter()
            .map(|p| p.start_secs)
            .collect();
        assert_eq!(
            starts,
            [Some(0), None, None, None, None, Some(143), Some(146)]
        );
    }

    #[test]
    fn html_cites_and_times_inside_paragraphs_and_breaks_parse() {
        let cues = parse_html(
            "<html><head><title>Ignored words</title></head><body>\
             <div><p><cite>Kevin:</cite> <time>1:02:03</time>Hello<br>there \
             &amp; welcome.</p></div>\
             <p>Still Kevin.</p>\
             <cite>SPEAKER_00</cite><time datetime=\"0:15\">soon</time>\
             <p>A token.</p></body></html>",
        );
        assert_eq!(
            cues,
            [
                Cue::new(
                    Some(3723.0),
                    Some("Kevin".into()),
                    " Hello there & welcome.".into()
                ),
                Cue::new(None, Some("Kevin".into()), "Still Kevin.".into()),
                Cue::new(
                    Some(15.0),
                    Some("SPEAKER_00".into()),
                    "A token.".into()
                ),
            ]
        );
    }

    #[test]
    fn text_paragraphs_never_merge_and_carry_no_times() {
        let transcript =
            parse(TEXT, "text/plain", Some(EPISODE_MS)).expect("untimed");
        assert_eq!(transcript.format, TranscriptFormat::Text);
        assert_eq!(transcript.words, 603);
        assert!(transcript
            .paragraphs
            .iter()
            .all(|p| p.start_secs.is_none() && p.speaker.is_none()));
        assert_paragraph_bounds(&transcript);
        // Seven blank-line paragraphs; the first (494 words) splits into
        // five.
        assert_eq!(transcript.paragraphs.len(), 11);
        assert_eq!(
            transcript.paragraphs[6].text,
            "It's great to have you here. This is your pull requests per \
             month. The last month, it was more than 5,000 pull requests."
        );
        assert_eq!(transcript.paragraphs[8].text, "How? How?");
    }

    // -----------------------------------------------------------------
    // VTT and SRT syntax.
    // -----------------------------------------------------------------

    #[test]
    fn vtt_skips_header_notes_styles_and_regions() {
        let cues = parse_cue_blocks(
            "WEBVTT - Episode 12\nKind: captions\nLanguage: en\n\n\
             STYLE\n::cue { color: lime }\n\n\
             REGION\nid:fred\n\n\
             NOTE This is a comment\nspanning lines\n\n\
             NOTE\nanother --> looking note\n\n\
             intro\n00:01.500 --> 00:04.000 align:start position:10% line:0\n\
             First cue\nsecond line\n\n\
             2\n01:02:03.250 --> 01:02:05.000\n\
             Second cue\n\n\
             stray text without timing\n\n\
             3\nnot a time --> 00:09.000\nunparseable timing\n",
        );
        assert_eq!(
            cues,
            [
                Cue::new(Some(1.5), None, "First cue\nsecond line".into()),
                Cue::new(Some(3723.25), None, "Second cue".into()),
            ]
        );
    }

    #[test]
    fn vtt_voices_tags_and_entities_resolve() {
        let cues = parse_cue_blocks(
            "\u{FEFF}WEBVTT\n\n\
             00:00:01.000 --> 00:00:03.000\n\
             <v.loud Gergely Orosz>Hi <c.yellow>there</c>, <i>you</i>\
             <00:00:02.000> &amp; me &lt;3 &nbsp;&#8212; &#x2019;s &lrm;ok &bogus; a<b\n\n\
             00:00:04.000 --> 00:00:06.000\n\
             <v Esme>Hello</v> <v SPEAKER_00>World <ruby>漢<rt>kan</rt></ruby>字\n\n\
             00:00:07.000 --> 00:00:08.000\n\
             <V Upper>Case {\\an8}tags</v> after\n",
        );
        let shown: Vec<_> = cues
            .iter()
            .map(|cue| {
                (
                    cue.start,
                    cue.speaker.as_deref(),
                    cue.text.split_whitespace().collect::<Vec<_>>().join(" "),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    Some(1.0),
                    Some("Gergely Orosz"),
                    "Hi there, you & me <3 — ’s ok &bogus; a<b".to_string()
                ),
                (Some(4.0), Some("Esme"), "Hello".to_string()),
                (Some(4.0), Some("SPEAKER_00"), "World 漢字".to_string()),
                (Some(7.0), Some("Upper"), "Case tags".to_string()),
                (Some(7.0), None, "after".to_string()),
            ]
        );
    }

    #[test]
    fn srt_tags_and_overrides_are_dropped() {
        let cues = parse_cue_blocks(
            "1\n00:00:00,000 --> 00:00:02,000 X1:10 X2:20 Y1:30 Y2:40\n\
             {\\an8}<font color=\"#ffffff\"><i>Hello</i></font>\n\n\
             2\n00:00:02,500 --> 00:00:04,000\n<b>World</b>\n",
        );
        assert_eq!(
            cues,
            [
                Cue::new(Some(0.0), None, "Hello".into()),
                Cue::new(Some(2.5), None, "World".into()),
            ]
        );
    }

    #[test]
    fn clocks_parse_with_dots_commas_and_hours() {
        assert_eq!(parse_clock("00:01.5"), Some(1.5));
        assert_eq!(parse_clock("1:02:03,250"), Some(3723.25));
        assert_eq!(parse_clock(" 12:34 "), Some(754.0));
        assert_eq!(parse_clock("0:60"), None);
        assert_eq!(parse_clock("12"), None);
        assert_eq!(parse_clock("soon"), None);
    }

    #[test]
    fn clocks_format_with_hours_only_when_needed() {
        assert_eq!(format_clock(0), "0:00");
        assert_eq!(format_clock(178), "2:58");
        assert_eq!(format_clock(5197), "1:26:37");
    }

    // -----------------------------------------------------------------
    // Garbage and sanity checks.
    // -----------------------------------------------------------------

    #[test]
    fn garbage_is_dropped_with_a_reason() {
        let cases: Vec<(Vec<u8>, &str, &str, TranscriptDrop, &str)> = vec![
            (
                b"ID3\x04\x00\x00\x00\x00\xff\xfb\x90\x64".to_vec(),
                "text/vtt",
                "",
                TranscriptDrop::NotText,
                "the file is not text",
            ),
            (
                vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46],
                "text/plain",
                "",
                TranscriptDrop::NotText,
                "the file is not text",
            ),
            (
                b"ID3".to_vec(),
                "audio/mpeg",
                "https://cdn.example/ep.mp3",
                TranscriptDrop::Unsupported {
                    mime: "audio/mpeg".into(),
                },
                "unsupported transcript type audio/mpeg",
            ),
            (
                b"hello".to_vec(),
                "",
                "https://cdn.example/transcript",
                TranscriptDrop::Unsupported {
                    mime: String::new(),
                },
                "the file names no transcript format",
            ),
            (
                b"{\"segments\": [".to_vec(),
                "application/json",
                "",
                TranscriptDrop::Malformed {
                    format: TranscriptFormat::Json,
                    detail: "is not valid JSON",
                },
                "the JSON transcript is not valid JSON",
            ),
            (
                b"{\"version\": \"1.0.0\"}".to_vec(),
                "application/json",
                "",
                TranscriptDrop::Malformed {
                    format: TranscriptFormat::Json,
                    detail: "has no segments list",
                },
                "the JSON transcript has no segments list",
            ),
            (
                b"<!DOCTYPE html><html><body>Not found</body></html>".to_vec(),
                "text/vtt",
                "",
                TranscriptDrop::Empty {
                    format: TranscriptFormat::Vtt,
                },
                "the VTT transcript holds no text",
            ),
            (
                b"  <!doctype HTML>\n<html><body>Error page</body></html>"
                    .to_vec(),
                "text/plain",
                "",
                TranscriptDrop::Malformed {
                    format: TranscriptFormat::Text,
                    detail: "is an HTML page",
                },
                "the text transcript is an HTML page",
            ),
            (
                b"WEBVTT\n\n00:00.000 --> 00:01.000\n- - -\n".to_vec(),
                "text/vtt",
                "",
                TranscriptDrop::Empty {
                    format: TranscriptFormat::Vtt,
                },
                "the VTT transcript holds no text",
            ),
            (
                vtt_of(&[(0, None, &filler(199))]).into_bytes(),
                "text/vtt",
                "",
                TranscriptDrop::TooShort { words: 199 },
                "only 199 words (fewer than 200)",
            ),
        ];
        for (bytes, mime, url, drop, message) in cases {
            let got = parse_transcript(&bytes, mime, url, None).unwrap_err();
            assert_eq!(got, drop, "{mime} {url}");
            assert_eq!(got.to_string(), message);
        }
    }

    #[test]
    fn word_count_bounds_are_inclusive() {
        let at_floor = vtt_of(&[(0, None, &filler(200))]);
        assert_eq!(parse(&at_floor, "text/vtt", None).unwrap().words, 200);

        let at_cap = filler(MAX_WORDS);
        assert_eq!(
            parse(&at_cap, "text/plain", None).unwrap().words,
            MAX_WORDS
        );

        let over = filler(MAX_WORDS + 1);
        let drop = parse(&over, "text/plain", None).unwrap_err();
        assert_eq!(drop, TranscriptDrop::TooLong { words: 120_001 });
        assert_eq!(drop.to_string(), "120,001 words (more than 120,000)");
    }

    #[test]
    fn the_last_cue_must_start_within_half_to_110_percent() {
        let body = vtt_of(&[(0, None, &filler(150)), (500, None, &filler(60))]);
        let at = |duration_secs: u64| {
            parse(&body, "text/vtt", Some(duration_secs * 1000))
        };
        assert!(at(1000).is_ok(), "exactly half");
        assert!(at(455).is_ok(), "500 s is within 110 % of 455 s");
        assert_eq!(
            at(1001).unwrap_err(),
            TranscriptDrop::EndsEarly {
                last_start_secs: 500,
                duration_secs: 1001,
            }
        );
        let late = at(450).unwrap_err();
        assert_eq!(
            late,
            TranscriptDrop::EndsLate {
                last_start_secs: 500,
                duration_secs: 450,
            }
        );
        assert_eq!(
            late.to_string(),
            "the last cue starts at 8:20, past 110% of the 7:30 episode"
        );
        assert!(
            parse(&body, "text/vtt", Some(0)).is_ok(),
            "a zero duration is unknown"
        );
    }

    #[test]
    fn decoding_follows_the_bom_then_the_charset() {
        let text = format!("caf\u{e9} {}", filler(205));
        let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
        utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let transcript = parse_transcript(&utf16, "text/plain", "", None)
            .expect("UTF-16 with a BOM");
        assert!(transcript.paragraphs[0].text.starts_with("café word"));

        let mut latin1 = b"caf\xe9 ".to_vec();
        latin1.extend(filler(205).into_bytes());
        let transcript = parse_transcript(
            &latin1,
            "text/plain; charset=\"ISO-8859-1\"",
            "",
            None,
        )
        .expect("a declared charset");
        assert!(transcript.paragraphs[0].text.starts_with("café word"));
    }

    // -----------------------------------------------------------------
    // Speakers.
    // -----------------------------------------------------------------

    /// An interview: the questioner's turns are one-question lines, the
    /// answerer's are 30-word answers that ask `answer_questions` of
    /// their questions in total.
    fn interview(
        questioner: &str,
        answerer: &str,
        questions: usize,
        answer_questions: usize,
        answerer_first: bool,
    ) -> Transcript {
        let mut cues = Vec::new();
        let mut at = 0;
        if answerer_first {
            cues.push((at, Some(answerer), format!("{} hello.", filler(5))));
            at += 2;
        }
        for turn in 0..questions {
            cues.push((at, Some(questioner), "So what happened next?".into()));
            let ask = if turn < answer_questions {
                "Right?"
            } else {
                "Yes."
            };
            cues.push((
                at + 2,
                Some(answerer),
                format!("{} done. {ask}", filler(28)),
            ));
            at += 20;
        }
        let refs: Vec<_> = cues
            .iter()
            .map(|(at, voice, text)| (*at, *voice, text.as_str()))
            .collect();
        parse(&vtt_of(&refs), "text/vtt", None).expect("interview parses")
    }

    #[test]
    fn a_frequent_questioner_is_the_host_either_way_round() {
        let transcript = interview("SPEAKER_00", "SPEAKER_01", 12, 0, false);
        assert!(transcript.roles_inferred);
        assert!(!transcript.names_from_file);
        assert_eq!(speakers(&transcript)[..2], [Some("Host"), Some("Guest")]);

        // The guest speaks first; the questioner is still the host.
        let transcript = interview("spk_1", "spk_0", 12, 0, true);
        assert!(transcript.roles_inferred);
        assert_eq!(
            speakers(&transcript)[..3],
            [Some("Guest"), Some("Host"), Some("Guest")]
        );
    }

    #[test]
    fn roles_need_ten_questions_and_a_threefold_rate() {
        // Nine questions are too few.
        let transcript = interview("SPEAKER_00", "SPEAKER_01", 9, 0, false);
        assert!(!transcript.roles_inferred);
        assert_eq!(
            speakers(&transcript)[..2],
            [Some("Speaker 1"), Some("Speaker 2")]
        );

        // Host: 12 questions in 48 words; guest: 12 in 360 words — the
        // host's rate is 7.5×.
        assert!(interview("S1", "S2", 12, 12, false).roles_inferred);

        // A guest who asks back often enough keeps the rate under 3×.
        let host = (12, 48);
        assert_eq!(host_of([host, (12, 360)]), Some(0));
        assert_eq!(host_of([host, (12, 144)]), Some(0), "exactly 3×");
        assert_eq!(host_of([host, (12, 143)]), None, "just under 3×");
        assert_eq!(host_of([(12, 143), host]), None);
        assert_eq!(host_of([(9, 36), (0, 400)]), None, "nine questions");
        assert_eq!(host_of([(10, 40), (0, 400)]), Some(0), "ten questions");
        assert_eq!(host_of([(0, 400), (10, 40)]), Some(1));
        assert_eq!(host_of([(10, 40), (0, 0)]), None, "a silent speaker");
    }

    #[test]
    fn real_names_and_crowds_never_get_roles() {
        let named = interview("Gergely Orosz", "Harjot Gill", 12, 0, false);
        assert!(named.names_from_file);
        assert!(!named.roles_inferred);
        assert_eq!(
            speakers(&named)[..2],
            [Some("Gergely Orosz"), Some("Harjot Gill")]
        );

        // A named speaker beside one token: no roles, the token numbered.
        let mixed = interview("SPEAKER_07", "Harjot Gill", 12, 0, false);
        assert!(mixed.names_from_file && !mixed.roles_inferred);
        assert_eq!(
            speakers(&mixed)[..2],
            [Some("Speaker 1"), Some("Harjot Gill")]
        );

        // Three diarized speakers: no roles.
        let mut cues: Vec<(u32, Option<&str>, String)> = Vec::new();
        for turn in 0..12u32 {
            cues.push((turn * 30, Some("SPEAKER_02"), "Why though?".into()));
            cues.push((turn * 30 + 5, Some("SPEAKER_00"), filler(10)));
            cues.push((turn * 30 + 10, Some("SPEAKER_01"), filler(10)));
        }
        let refs: Vec<_> =
            cues.iter().map(|(a, v, t)| (*a, *v, t.as_str())).collect();
        let crowd = parse(&vtt_of(&refs), "text/vtt", None).unwrap();
        assert!(!crowd.roles_inferred && !crowd.names_from_file);
        assert_eq!(
            speakers(&crowd)[..3],
            [Some("Speaker 1"), Some("Speaker 2"), Some("Speaker 3")]
        );
    }

    #[test]
    fn diarization_tokens_are_recognized() {
        for token in [
            "SPEAKER_00",
            "SPEAKER_12",
            "Speaker 1",
            "speaker-a",
            "Speaker B",
            "spk_0",
            "SPK-3",
            "S1",
            "s12",
        ] {
            assert!(is_diarization_token(token), "{token}");
        }
        for name in [
            "Sam",
            "S",
            "Speaker",
            "Speaker of the House",
            "Spike",
            "Gergely Orosz",
            "Host",
        ] {
            assert!(!is_diarization_token(name), "{name}");
        }
    }

    #[test]
    fn questions_count_once_per_run() {
        assert_eq!(count_questions("Why? Why?! Really?!? Sure."), 3);
        assert_eq!(count_questions("¿Qué？ Fine"), 1);
        assert_eq!(count_questions("No questions."), 0);
    }

    // -----------------------------------------------------------------
    // Paragraphs.
    // -----------------------------------------------------------------

    #[test]
    fn paragraphs_close_at_a_sentence_end_after_95_words() {
        // Thirty 10-word sentences, one cue each, every 3 seconds.
        let sentence = format!("{} end.", filler(9));
        let cues: Vec<_> = (0..30)
            .map(|i| (i * 3, Some("Speaker 1"), sentence.as_str()))
            .collect();
        let transcript = parse(&vtt_of(&cues), "text/vtt", None).unwrap();
        let shape: Vec<_> = transcript
            .paragraphs
            .iter()
            .map(|p| (p.start_secs, count_words(&p.text)))
            .collect();
        assert_eq!(shape, [(Some(0), 100), (Some(30), 100), (Some(60), 100)]);
        assert_paragraph_bounds(&transcript);
    }

    #[test]
    fn paragraphs_are_forced_closed_at_160_words() {
        let cues: Vec<_> = (0..40)
            .map(|i| {
                (
                    i * 2,
                    None,
                    "word word word word word word word word word word",
                )
            })
            .collect();
        let transcript = parse(&vtt_of(&cues), "text/vtt", None).unwrap();
        let shape: Vec<_> = transcript
            .paragraphs
            .iter()
            .map(|p| (p.start_secs, count_words(&p.text)))
            .collect();
        assert_eq!(shape, [(Some(0), 160), (Some(32), 160), (Some(64), 80)]);
    }

    #[test]
    fn a_speaker_change_always_closes_a_paragraph() {
        let short = filler(70);
        let transcript = parse(
            &vtt_of(&[
                (0, Some("Ann"), short.as_str()),
                (30, Some("Bo"), "Yes."),
                (31, Some("Ann"), short.as_str()),
                (60, None, short.as_str()),
            ]),
            "text/vtt",
            None,
        )
        .unwrap();
        let shape: Vec<_> = transcript
            .paragraphs
            .iter()
            .map(|p| (p.start_secs, p.speaker.as_deref(), count_words(&p.text)))
            .collect();
        assert_eq!(
            shape,
            [
                (Some(0), Some("Ann"), 70),
                (Some(30), Some("Bo"), 1),
                (Some(31), Some("Ann"), 70),
                (Some(60), None, 70),
            ]
        );
    }

    #[test]
    fn a_split_cue_stamps_only_its_first_paragraph() {
        // The sentence ends mid-cue at word 96; the rest of that cue
        // opens a paragraph stamped with the cue's start, because the
        // paragraph before opened in an earlier cue. The long last cue
        // closes two paragraphs: the first one it opens takes its start,
        // the second has none.
        let opening = filler(90);
        let middle = format!("{} stop. {}", filler(5), filler(20));
        let long =
            format!("{} stop. {} stop. {}", filler(99), filler(99), filler(10));
        let transcript = parse(
            &vtt_of(&[
                (0, None, opening.as_str()),
                (40, None, middle.as_str()),
                (50, None, long.as_str()),
            ]),
            "text/vtt",
            None,
        )
        .unwrap();
        let shape: Vec<_> = transcript
            .paragraphs
            .iter()
            .map(|p| (p.start_secs, count_words(&p.text)))
            .collect();
        assert_eq!(
            shape,
            [(Some(0), 96), (Some(40), 120), (Some(50), 100), (None, 10)]
        );
    }

    #[test]
    fn abbreviations_do_not_end_sentences() {
        for token in ["Dr.", "e.g.", "U.S.", "vs.", "(i.e."] {
            assert!(!ends_sentence(token), "{token}");
        }
        for token in [
            "done.", "why?", "wow!", "so…", "said.\"", "(this).", "A.", "I.",
        ] {
            assert!(ends_sentence(token), "{token}");
        }
        // "Dr." is word 96: the paragraph runs on to "so." at 99.
        let body = format!("{} Dr. Smith said so. {}", filler(95), filler(110));
        let transcript =
            parse(&vtt_of(&[(0, None, body.as_str())]), "text/vtt", None)
                .unwrap_or_else(|drop| panic!("{drop}"));
        let words: Vec<_> = transcript
            .paragraphs
            .iter()
            .map(|p| count_words(&p.text))
            .collect();
        assert_eq!(words, [99, 110]);
    }

    // -----------------------------------------------------------------
    // Choosing a format.
    // -----------------------------------------------------------------

    #[test]
    fn offers_are_chosen_json_then_vtt_srt_html_text() {
        let offers = [
            ("text/plain", "https://cdn.example/t.txt"),
            ("text/html", "https://cdn.example/t.html"),
            ("application/x-subrip", "https://cdn.example/t.srt"),
            ("text/vtt", "https://cdn.example/t.vtt"),
            ("text/vtt", "https://cdn.example/second.vtt"),
            ("application/json", "https://cdn.example/t.json"),
            ("audio/mpeg", "https://cdn.example/t.mp3"),
        ];
        let pick = |offers: &[(&'static str, &'static str)]| {
            preferred_offer(offers, |(mime, url)| (*mime, *url))
                .map(|(offer, format)| (offer.1, format))
        };
        assert_eq!(
            pick(&offers),
            Some(("https://cdn.example/t.json", TranscriptFormat::Json))
        );
        assert_eq!(
            pick(&offers[..5]),
            Some(("https://cdn.example/t.vtt", TranscriptFormat::Vtt)),
            "the first offer of a format wins"
        );
        assert_eq!(pick(&offers[..3]).unwrap().1, TranscriptFormat::Srt);
        assert_eq!(pick(&offers[..2]).unwrap().1, TranscriptFormat::Html);
        assert_eq!(pick(&offers[..1]).unwrap().1, TranscriptFormat::Text);
        assert_eq!(pick(&offers[6..]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn feed_transcript_offers_choose_directly() {
        use super::super::feed::FeedTranscript;
        let offer = |url: &str, mime: &str| FeedTranscript {
            url: url.to_string(),
            mime: mime.to_string(),
            language: Some("en".to_string()),
            rel: None,
        };
        let offers = [
            offer("https://cdn.example/t.srt", "application/x-subrip"),
            offer("https://cdn.example/t.vtt", "text/vtt"),
            offer("https://cdn.example/t.pdf", "application/pdf"),
        ];
        let (chosen, format) =
            preferred_offer(&offers, |t| (t.mime.as_str(), t.url.as_str()))
                .expect("a known format");
        assert_eq!(chosen.url, "https://cdn.example/t.vtt");
        assert_eq!(format, TranscriptFormat::Vtt);
    }

    #[test]
    fn generic_mime_types_defer_to_the_url() {
        let cases = [
            (
                "text/vtt; charset=utf-8",
                "https://x/t",
                Some(TranscriptFormat::Vtt),
            ),
            ("TEXT/VTT", "https://x/t.json", Some(TranscriptFormat::Vtt)),
            (
                "application/octet-stream",
                "https://x/t.VTT?sig=1#x",
                Some(TranscriptFormat::Vtt),
            ),
            ("text/plain", "https://x/t.srt", Some(TranscriptFormat::Srt)),
            (
                "text/plain; charset=utf-8",
                "https://x/transcript",
                Some(TranscriptFormat::Text),
            ),
            ("", "https://x/t.htm", Some(TranscriptFormat::Html)),
            ("", "https://x.json/transcript", None),
            ("application/srt", "", Some(TranscriptFormat::Srt)),
            ("audio/mpeg", "https://x/t.mp3", None),
        ];
        for (mime, url, expected) in cases {
            assert_eq!(
                TranscriptFormat::detect(mime, url),
                expected,
                "{mime} {url}"
            );
        }
    }

    #[test]
    fn formats_have_payload_and_reader_names() {
        let names: Vec<_> = TranscriptFormat::PREFERENCE
            .iter()
            .map(|format| (format.as_str(), format.label()))
            .collect();
        assert_eq!(
            names,
            [
                ("json", "JSON"),
                ("vtt", "VTT"),
                ("srt", "SRT"),
                ("html", "HTML"),
                ("text", "text"),
            ]
        );
    }

    #[test]
    fn paragraphs_serialize_as_the_payload_shape() {
        let paragraph = Paragraph {
            start_secs: Some(197),
            speaker: Some("Speaker 1".into()),
            text: "Hello.".into(),
        };
        assert_eq!(
            serde_json::to_value(&paragraph).unwrap(),
            serde_json::json!({
                "start_secs": 197,
                "speaker": "Speaker 1",
                "text": "Hello.",
            })
        );
    }
}

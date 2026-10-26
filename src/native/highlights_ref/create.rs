use crate::native::env as bob_env;
use std::{
    cmp::Ordering,
    env,
    ffi::{OsStr, OsString},
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// TTY-gated `fetching …` reporter preserving the pre-ingest output:
/// prints only on a terminal, exactly as `fetch` used to.
fn fetch_progress(line: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("{line}");
    }
}

/// Unconditional warning reporter preserving the pre-ingest PDF warning.
fn pdf_progress(line: &str) {
    eprintln!("{line}");
}

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use sha2::Digest;

use super::force::{is_overwrite, nothing_to_overwrite, triage_overwrite_url};
use super::note_snapshot;
use super::recapture::RereadPreflight;
use super::FIELD_SOURCE_NOTE;
use super::{
    attach as attach_mod, companion as companion_mod,
    pdf_target as pdf_target_mod,
};
use super::{
    bob_dir_arg, compose_marker, dry_run_arg, lib_dir_arg, plan_default_target,
    plan_exact_output, print_next_step, ref_dir_arg, stamp_and_install,
    validate_ref_type, xlib_dir_arg, AudioCopyPlan, CommandError, Config,
    DefaultTargetIdentity, MarkerValue, PdfInfo, RenamedFrom, Result,
    TargetPlan, TargetWorkflow, COMMAND_NAME,
};
use super::{sources as sources_mod, target as target_mod};
use crate::native::style::Styler;

const DEFAULT_MARKDOWN_REF_TYPE: &str = "chat";
const DEFAULT_NOTE_REF_TYPE: &str = "notes";
const DEFAULT_PDF_REF_TYPE: &str = "papers";
const DEFAULT_ARTICLE_REF_TYPE: &str = "blogs";
const DEFAULT_STATUS: &str = "ready";
const ENV_PANDOC_COMMAND: &str = "BOB_PANDOC_COMMAND";
const ENV_AUDIO_LINK_TEMPLATE: &str = "BOB_HIGHLIGHTS_AUDIO_LINK_TEMPLATE";
const DEFAULT_AUDIO_LINK_TEMPLATE: &str =
    "obsidian://open?vault={vault}&file={path}";
/// LaTeX preamble that wraps long code blocks instead of overflowing the page.
///
/// Pandoc emits every fenced block as a `Highlighting` environment, which does
/// not wrap by default, so a single long log or command line silently runs off
/// the right margin.
pub(super) const PANDOC_HEADER_INCLUDES: &str = concat!(
    r"\usepackage{fvextra}",
    r"\DefineVerbatimEnvironment{Highlighting}{Verbatim}",
    r"{breaklines,breakanywhere,commandchars=\\\{\}}",
    r"\definecolor{BobListenRule}{HTML}{3B6EA8}",
    r"\definecolor{BobListenFill}{HTML}{EEF3FA}",
    r"\newcommand{\BobListenCard}[1]{\par\medskip\noindent\hbox{{\color{BobListenRule}\vrule width 2.5pt}\setlength{\fboxsep}{6pt}\colorbox{BobListenFill}{\parbox{\dimexpr\linewidth-2.5pt-12pt\relax}{\sffamily\small\raggedright #1}}}\par\medskip}",
    r"\newcommand{\BobListenPlay}{\colorbox{BobListenRule}{\textcolor{white}{\textbf{▶\,Play}}}}",
    r"\AtBeginDocument{\raggedright\hyphenpenalty=10000\exhyphenpenalty=10000}",
);
/// Pandoc Lua filter that gives long inline code somewhere to break, renders
/// listen-card Divs as a compact LaTeX callout, and numbers sections from the
/// file's top section level (a lone H1 becomes an unnumbered, unlisted title
/// heading and every other heading moves up) whenever pandoc numbers sections
/// for LaTeX output.
pub(super) const PANDOC_CODE_BREAK_FILTER: &str = r#"local SEPARATORS = "[/_%-%.:,]"
local PLAY_URI = nil

function Meta(meta)
  local value = meta["bob-listen-uri"]
  if value ~= nil then
    PLAY_URI = pandoc.utils.stringify(value)
  end
end

function Code(code)
  local pieces = {}
  local buffer = ""
  for index = 1, #code.text do
    local char = code.text:sub(index, index)
    buffer = buffer .. char
    if char:match(SEPARATORS) and index < #code.text then
      table.insert(pieces, pandoc.Code(buffer, code.attr))
      table.insert(pieces, pandoc.RawInline("latex", "\\allowbreak{}"))
      buffer = ""
    end
  end
  if #pieces == 0 then
    return nil
  end
  if buffer ~= "" then
    table.insert(pieces, pandoc.Code(buffer, code.attr))
  end
  return pieces
end

local function is_relative_target(target)
  return not target:match("^[%w][%w+%.%-]*:") and not target:match("^#")
end

local function trim_dead_link_separator(inlines, link_index)
  while link_index > 1 and inlines[link_index - 1].t == "Space" do
    table.remove(inlines, link_index - 1)
    link_index = link_index - 1
  end
  local previous = inlines[link_index - 1]
  if previous and previous.t == "Str" then
    previous.text = previous.text:gsub("%s*·$", "")
    if previous.text == "" then
      table.remove(inlines, link_index - 1)
      link_index = link_index - 1
    end
  end
  return link_index
end

function Div(div)
  if not (FORMAT:match("latex") and div.classes:includes("listen")) then
    return nil
  end

  local content = nil
  for _, block in ipairs(div.content) do
    if block.t == "Para" or block.t == "Plain" then
      content = block.content
      break
    end
  end
  if content == nil then
    return pandoc.Null()
  end

  local index = 1
  while index <= #content do
    local inline = content[index]
    if inline.t == "Link" and is_relative_target(inline.target) then
      index = trim_dead_link_separator(content, index)
      table.remove(content, index)
    else
      index = index + 1
    end
  end

  if PLAY_URI and PLAY_URI ~= "" then
    local first = content[1]
    if first and first.t == "Str" then
      local remainder = first.text:gsub("^♫", "", 1)
      if remainder ~= first.text then
        table.remove(content, 1)
        while content[1] and content[1].t == "Space" do
          table.remove(content, 1)
        end
        table.insert(content, 1, pandoc.RawInline("latex", "\\hspace{0.7em}"))
        table.insert(content, 1, pandoc.Link(
          {pandoc.RawInline("latex", "\\BobListenPlay{}")}, PLAY_URI))
        if remainder ~= "" then
          table.insert(content, 2, pandoc.Str(remainder))
        end
      end
    end
  end

  return pandoc.RawBlock("latex", "\\BobListenCard{" ..
    pandoc.write(pandoc.Pandoc({pandoc.Plain(content)}), "latex") .. "}")
end

local function number_from_top_level(doc)
  if not (FORMAT:match("latex") and PANDOC_WRITER_OPTIONS
      and PANDOC_WRITER_OPTIONS.number_sections) then
    return nil
  end
  local h1 = 0
  doc:walk({ Header = function(h) if h.level == 1 then h1 = h1 + 1 end end })
  local lone = h1 == 1
  local shallowest = nil
  doc:walk({ Header = function(h)
    if not (lone and h.level == 1) and (shallowest == nil or h.level < shallowest) then
      shallowest = h.level
    end
  end })
  local shift = (shallowest or 1) - 1
  local listed = 0
  doc = doc:walk({ Header = function(h)
    if lone and h.level == 1 then
      for _, class in ipairs({ "unnumbered", "unlisted" }) do
        if not h.classes:includes(class) then h.classes:insert(class) end
      end
    elseif shift > 0 then
      h.level = h.level - shift
    end
    if not h.classes:includes("unlisted") then listed = listed + 1 end
    return h
  end })
  if listed == 0 then doc.meta.toc = false end
  return doc
end

return {{Meta = Meta}, {Code = Code, Div = Div}, {Pandoc = number_from_top_level}}
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CreateOptions {
    pub(super) audio: Option<PathBuf>,
    pub(super) author: Option<String>,
    pub(super) closed: bool,
    pub(super) dry_run: bool,
    pub(super) force: Option<super::force::ForceMode>,
    pub(super) html: Option<OsString>,
    pub(super) include_id: bool,
    pub(super) keep_intake_reason: Option<String>,
    pub(super) listen: bool,
    pub(super) name: Option<String>,
    pub(super) no_audio: bool,
    pub(super) output: Option<PathBuf>,
    pub(super) plain: bool,
    pub(super) parent: String,
    pub(super) parent_default_reason: Option<String>,
    pub(super) resolved_parent:
        Option<crate::native::parent_notes::ResolvedParent>,
    pub(super) published: Option<String>,
    pub(super) sections: Vec<String>,
    pub(super) ref_type: Option<String>,
    pub(super) status: String,
    pub(super) title: Option<String>,
    pub(super) reread: Option<RereadPreflight>,
}

enum PreparedTarget {
    Url,
    Markdown(PathBuf),
    Pdf(PathBuf),
    VaultNote {
        store: super::note_snapshot::SourceStore,
        note: super::note_snapshot::ClassifiedNote,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreatePlan {
    source: String,
    source_kind: &'static str,
    target: PathBuf,
    sidecar: PathBuf,
    workflow: TargetWorkflow,
    stem: String,
    renamed_from: Option<RenamedFrom>,
    title: String,
    title_source: &'static str,
    author: Option<String>,
    published: Option<String>,
    source_url: Option<String>,
    source_note: Option<String>,
    source_markdown: Option<String>,
    captured: Option<String>,
    id: Option<String>,
    marker: String,
    audio: Option<AudioCopyPlan>,
    page_count_hint: Option<usize>,
}

/// Pandoc inputs for the Markdown render: the code-break/listen filter, the
/// return-link filter that runs after it, the JSON report path the
/// return-link filter writes for bob, and an optional first-position
/// diagrams swap filter.
///
/// Shared with the note-snapshot print profile, which reuses the same
/// filters plus its own contract filter. [`super::diagrams::prepare`]
/// fills `diagrams` when the pass finds Mermaid blocks.
pub(crate) struct RenderFilters {
    pub(crate) code_break: PathBuf,
    pub(crate) return_links: PathBuf,
    pub(crate) report: PathBuf,
    pub(crate) diagrams: Option<super::diagrams::DiagramFilter>,
}

impl CreatePlan {
    fn target_plan(&self) -> TargetPlan {
        TargetPlan {
            target: self.target.clone(),
            sidecar: self.sidecar.clone(),
            workflow: self.workflow.clone(),
            stem: self.stem.clone(),
            renamed_from: self.renamed_from.clone(),
        }
    }
}

pub(super) fn print_create_intake_preview(
    config: &Config,
    plan: &TargetPlan,
    keep_reason: Option<&str>,
) {
    let unit = match &plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => super::intake::IntakeUnit::Pdf {
            source: plan.target.clone(),
            destination: library_destination.clone(),
        },
        TargetWorkflow::Library => super::intake::IntakeUnit::LibraryPdf {
            pdf: plan.target.clone(),
        },
        TargetWorkflow::External => {
            print_next_step(plan);
            return;
        }
    };
    super::intake::print_preview(config, &unit, keep_reason);
}

pub(super) fn finish_create_intake(
    config: &Config,
    plan: &TargetPlan,
    keep_reason: Option<&str>,
) -> Result<()> {
    let unit = match &plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => super::intake::IntakeUnit::Pdf {
            source: plan.target.clone(),
            destination: library_destination.clone(),
        },
        TargetWorkflow::Library => super::intake::IntakeUnit::LibraryPdf {
            pdf: plan.target.clone(),
        },
        TargetWorkflow::External => {
            print_next_step(plan);
            return Ok(());
        }
    };
    let progress = |line: &str| eprintln!("{line}");
    let outcome = super::intake::finish_intake(
        config,
        unit,
        super::intake::IntakeRequest {
            producer: "bob ref create",
            keep_reason,
            dry_run: false,
            progress: Some(&progress),
        },
    );
    super::intake::print_outcome(config, plan, outcome)
}

pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("create")
        .about("Create a Highlights-ready PDF from a vault note, Markdown, a PDF, or a URL")
        .alias("clip")
        .arg(
            Arg::new("target")
                .value_name("TARGET")
                .required(true)
                .num_args(1..)
                .action(ArgAction::Append)
                .value_parser(clap::builder::OsStringValueParser::new())
                .help("Vault note, Markdown file, PDF file, PDF URL, arXiv paper URL, or web article URL; several web article URLs become one reference"),
        )
        .arg(
            Arg::new("author")
                .long("author")
                .short('A')
                .value_name("NAME")
                .help("Override the derived author"),
        )
        .arg(
            Arg::new("closed")
                .long("closed")
                .short('C')
                .action(ArgAction::SetTrue)
                .conflicts_with("plain")
                .help_heading("Vault notes")
                .help("Also print closed tasks (a Closed tasks appendix) and full task logs"),
        )
        .arg(
            Arg::new("audio")
                .long("audio")
                .short('a')
                .value_name("PATH")
                .value_parser(clap::builder::OsStringValueParser::new())
                .value_hint(clap::ValueHint::FilePath)
                .conflicts_with("no-audio")
                .help("Use this companion audio file instead of discovering one"),
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(super::force::force_arg())
        .arg(
            Arg::new("html")
                .long("html")
                .short('H')
                .value_name("FILE")
                .value_parser(clap::builder::OsStringValueParser::new())
                .help("Replay a page saved from a real browser (Save Page As, SingleFile); `-` reads stdin; forces the web-article route"),
        )
        .arg(
            Arg::new("include-id")
                .long("include-id")
                .short('i')
                .action(ArgAction::SetTrue)
                .help("Embed the output filename stem as the marker id (URL targets always embed it)"),
        )
        .arg(
            Arg::new("keep-intake")
                .long("keep-intake")
                .short('k')
                .action(ArgAction::SetTrue)
                .help("Leave the new PDF queued in xlib/ for a later bob ref scan instead of filing it into the library now"),
        )
        .arg(
            Arg::new("listen")
                .long("listen")
                .short('L')
                .action(ArgAction::SetTrue)
                .conflicts_with("audio")
                .conflicts_with("no-audio")
                .help("Narrate TARGET with highlights.listen_command and bind the episode as companion audio"),
        )
        .arg(lib_dir_arg())
        .arg(
            Arg::new("name")
                .long("name")
                .short('N')
                .value_name("NAME")
                .help("Reference name: the PDF filename and the reference note written during intake [default: dated note stem for vault notes; otherwise derived from TARGET; for several URLs, the words their slugs share]"),
        )
        .arg(
            Arg::new("no-audio")
                .long("no-audio")
                .short('n')
                .action(ArgAction::SetTrue)
                .conflicts_with("audio")
                .help("Skip companion audio discovery and copy"),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .short('o')
                .value_name("PDF")
                .value_parser(clap::builder::OsStringValueParser::new())
                .conflicts_with("ref-type")
                .conflicts_with("name")
                .help("Complete path for the generated PDF, including the .pdf filename; with several URLs, its filename is the reference name"),
        )
        .arg(
            Arg::new("parent")
                .long("parent")
                .short('P')
                .value_name("NOTE")
                .help("Area or project note that owns the reading task [vault notes default to the nearest eligible note or ancestor]"),
        )
        .arg(
            Arg::new("plain")
                .long("plain")
                .short('M')
                .action(ArgAction::SetTrue)
                .conflicts_with("closed")
                .help_heading("Vault notes")
                .help("Render a vault note as plain Markdown, without the snapshot view"),
        )
        .arg(
            Arg::new("section")
                .long("section")
                .short('S')
                .value_name("HEADING")
                .action(ArgAction::Append)
                .conflicts_with("plain")
                .help_heading("Vault notes")
                .help("Keep only this projected section; repeat to select several"),
        )
        .arg(
            Arg::new("published")
                .long("published")
                .short('p')
                .value_name("DATE")
                .help("Override the derived publish date (YYYY-MM-DD)"),
        )
        .arg(ref_dir_arg())
        .arg(
            Arg::new("status")
                .long("status")
                .short('s')
                .value_name("STATUS")
                .default_value(DEFAULT_STATUS)
                .value_parser([
                    "ready",
                    "next",
                    "wip",
                    "read",
                    "abandoned",
                    "legacy",
                ])
                .help("Lifecycle status embedded in the marker"),
        )
        .arg(
            Arg::new("title")
                .long("title")
                .short('T')
                .value_name("TITLE")
                .help("Override the derived title; for vault notes this is the exact PDF identity title"),
        )
        .arg(
            Arg::new("ref-type")
                .long("ref-type")
                .short('t')
                .value_name("DIR")
                .conflicts_with("output")
                .help("Single library subdirectory [default: notes for vault notes, chat for Markdown, papers for PDFs, blogs for web articles]"),
        )
        .arg(xlib_dir_arg())
        .after_help(
            "Targets:\n  Vault note route, wikilink, or in-vault Markdown path -> projected reading snapshot (default ref type notes)\n  Other Markdown file (.md) -> rendered with pandoc (default ref type chat)\n  Local PDF file (.pdf or %PDF- magic) -> stamped as-is (default ref type papers)\n  PDF URL (Content-Type PDF or sniffed %PDF-) -> downloaded, stamped as-is (default ref type papers)\n  arXiv paper URL (abs/html/pdf) -> PDF fetched from arxiv.org/pdf/<id>, metadata from the API (default ref type papers)\n  Web article URL (HTML 2xx or 403/429/503) -> captured with the web-article engine into a Highlights-ready PDF (default ref type blogs)\n\nVault notes:\n  `TARGET` resolves like an Obsidian note link: exact vault-relative path, then a unique case-insensitive basename; `[[route|alias]]` is accepted. Existing filesystem paths keep precedence. Dot-directories, `_conflicts/`, `xlib/`, `lib/`, `lit_review/`, and shared excluded note directories are not snapshot notes. The default parent is the note itself when it is an eligible area/project, otherwise the nearest eligible area/project in its `parent:` chain (up to eight links); if none exists, pass `-P`. Names default to `<note-stem>_<YYMMDD>` using `BOB_NOW` when set. Titles use `-T`, frontmatter `title`, the first H1 outside fences/comments, then a humanized stem; the first H1 is removed only when it supplies the title. Closed tasks are counted and omitted, and schedule/work logs are condensed; `-C` restores closed tasks and full logs. Date-only `[?]` ticklers become a compact Scheduled lane (This week / This month / Later); other blocked tasks stay under Waiting on. `-M` keeps today's plain Markdown renderer while retaining the vault-note parent, dated name, and `source_note` provenance. `-C` and `-M` require one vault-note TARGET and conflict with each other. `-S` keeps only named projected sections (repeatable; an unknown name exits 2). Dry run lists earlier snapshots of this note newest-first as `earlier: <stem> (<status>, N highlights)`; both dry run and a real create print `hint: earlier snapshot <stem> is still unread (0 highlights)` for each earlier snapshot that is still `ready` with 0 highlights, and never change those notes. Dry run (`-d`) shows the selected identity, parent, target, projection summary, and warnings without running pandoc, xelatex, or listen; it ends with `writes: none`. Set BOB_HIGHLIGHTS_KEEP_WORKDIR=1 to retain projected Markdown, TeX, logs, fonts, and staged assets for debugging.\n  Examples: `bob ref create harbor_cranes`; `bob ref create '[[harbor_cranes]]' -d`; `bob ref create zk/some_idea -N idea_261016`; `bob ref create harbor_cranes -S Requirements`; `bob ref create harbor_cranes -C`; `bob ref create harbor_cranes -M`; `bob ref create harbor_cranes -L`; `bob ref create harbor_cranes -o ~/Desktop/harbor_cranes.pdf`\n\nSeveral URLs:\n  With several URLs and no -N, bob names the reference after the words every URL's slug starts with, or failing that the folder they all share, ignoring dates, ids, part numbers, and filler. It needs two meaningful words (or one when the pages differ only by a part or page number); otherwise pass -N or -T. Several URLs must all be web article URLs. Collection capture is all-or-nothing and follows argument order; one reference takes at most 20 URLs. Merge material you'll read as one unit; capture an ongoing series part by part under the same -P.\n\nWeb articles:\n  Captures the article in reader mode and re-typesets it with a Bob-owned print template; it never prints the live page. A successful create files the PDF, reference note, and reading task now. If intake is kept or cannot run, `bob ref scan` can file the queued PDF later. A site that blocks headless browsers is retried headed automatically: on Linux under a private Xvfb display or in an off-screen window on macOS; hosts with no browser fail closed with a hint. `-H, --html FILE` replays a page saved from a real browser instead of fetching (`-` reads stdin) and forces the web-article route. `-A, --author` and `-p, --published` override the derived author/publish date on every route. Environment: BOB_WEB_CLIP_ADAPTER replaces the adapter invocation; BOB_CHROME selects the browser executable; BOB_WEB_CLIP_TIMEOUT_SECS sets the adapter timeout in seconds (default 300); BOB_WEB_CLIP_KEEP_WORKDIR=1 keeps the scratch directory for debugging.\n\nListen:\n  `-L, --listen` narrates TARGET with `highlights.listen_command` (`BOB_HIGHLIGHTS_LISTEN_COMMAND` overrides), which must be `sase listen render ...` and write MP3 audio to `{audio}`; bob passes `{target}`, `{pdf}`, `{audio}`, and `{title}` as single arguments to `sase` itself — do not quote them — and streams the command output unchanged. A direct `sase-listen render ...` setting migrates automatically; wrappers, shell programs, chains, pipelines, redirections, and expansions need conversion to plain SASE render arguments. Preflights run first, the PDF is produced in private scratch, then the episode is bound beside the PDF; if the listen command fails nothing is written, and a failure after the episode exists keeps the scratch audio with a `kept:` line. If TARGET is already captured, `--listen` attaches the new episode and files it into the library with the existing PDF, or leaves it queued in `xlib/` when `-k` is set or intake cannot run. Vault-note `-L` passes the rendered `projected.md` as `{target}` and the rendered snapshot PDF as `{pdf}`; `-M` narrates the original Markdown. If this or an earlier episode is still queued for feed publishing, bob ends with a podcast-feed warning and recovery hint.\n\nAudio:\n  Create discovers audio as `--audio PATH`, then frontmatter `audio.episode_id` in the sase-listen library (Markdown only), then a sibling `<stem>_narration.md` hash matched against library manifests (`BOB_HIGHLIGHTS_AUDIO_LIBRARY`, then `highlights.audio_library`, then `$XDG_DATA_HOME/sase-listen/library`, then `~/.local/share/sase-listen/library`); `--no-audio` skips discovery. The copy lands beside the PDF with the source extension lowercased before the PDF is installed, reuses identical bytes, refuses different bytes without `--force`, and refuses when the mirrored library audio already exists. Only a companion already at the target's `<stem>.<ext>` counts as reused; audio beside the source is copied through the same rules.\n\nOutput:\n  Default target `<xlib-dir>/<ref-type>/<stem>.pdf`, becoming `<stem>_2`, `<stem>_3`, … when the name is taken by a different reference; the same reference refuses (`already captured` / `already queued`), and `-o, --output` keeps the exact path. `-o, --output` selects the complete path instead, including the filename (`.pdf` required, `~` expanded, cwd-relative). `--output` cannot be combined with `--ref-type` or `--name` because those only participate in default target derivation. For vault notes `-N` overrides the dated stem and `-i` follows the final collision-resolved stem; `-T` sets the exact identity title. Pandoc renders TOC/bookmarks for the legacy Markdown route and embeds the page-1 scan marker. Create files managed intake and library PDFs inline; `bob ref scan` drains queued PDFs and repairs notes or annotations. Other paths need `bob ref sync`. A `<div class=\"listen\">` card becomes a callout with a Play link when audio is bound. Same-document `#` links get a raised letter tag and a matching `↩ p. N` return pill under their target, dead ones render as plain text with a warning, and frontmatter `bob-return-links: false` turns this off.\n\nRecapture:\n  `-f [MODE]`, `--force[=MODE]` changes what a repeated capture does. Bare `-f` means `--force=reread`; an explicit mode needs `=`: `--force=overwrite` or `-f=overwrite` (a glued `-foverwrite` is not a mode; it parses as `-f -o verwrite`). A `reread` or `overwrite` word directly after a bare `-f` is adopted as the mode (`-f overwrite URL` captures URL with overwrite); an explicit `=` value never adopts a following word. A bare TARGET that equals a mode, or is within edit distance 2 of one, refuses with exit 2: write `./overwrite` for a file and `[[overwrite]]` for a vault note. Bare `-f` on an already-captured source reads it again as `<stem>__vN` beside the earlier readings: the newest reading's parent and type become the defaults (explicit `-P`/`-t` win), `id` is stamped to the new stem, earlier audio is never inherited, and a `reread:` line prints where `renamed:` would. An exact-URL-set collection rereads the same way in any URL order (several readings sharing one set count as one family); any partial overlap refuses under every mode. Bare `-f` on a fresh source captures it as usual. Without force the refusal names the next stem (`hint: -f read it again as <stem>__vN`). `-N` must name a reading of the same family (a free `-N` stays an ordinary capture) and selects the reading a later overwrite recaptures in place; `-o` on a captured reread exits 2. A library PDF as TARGET refuses even with `-f`: reread from its recorded `source_url` or the original file instead. `-L` without force still attaches to the captured set, while `-f -L` narrates the new reading. `-f overwrite` on a filed reference exits 2 naming the reread it would take (not available yet); without a filed match it exits 1. No force is needed for a URL recorded only by legacy notes without a Highlights PDF (it warns and captures a fresh copy) or for a new-day vault-note snapshot (an ordinary dated create). Queued-PDF, exact-output (`-o`), and companion-audio replacement keep working under either mode.\n\nExamples:\n  bob ref create report.md -P sase\n  bob ref create harbor_cranes\n  bob ref create harbor_cranes -d\n  bob ref create zk/some_idea -M\n  bob ref create paper.pdf -t papers -P sase\n  bob ref create https://example.com/paper.pdf -N my_paper -P sase\n  bob ref create https://arxiv.org/abs/1706.03762 -d -P bob\n  bob ref create https://arxiv.org/abs/1706.03762 -L -P bob\n  bob ref create https://example.com/essay -H saved.html -P sase\n  bob ref create https://example.com/essay -A \"Jane Doe\" -p 2026-01-02 -d -P sase\n  bob ref create https://example.com/2024/01/tokio-internals-part-1 https://example.com/2024/02/tokio-internals-part-2 -L -P sase\n  bob ref create URL1 URL2 URL3 -N spotify_bg_agents -P sase",
        )
}

pub(super) fn run(matches: &ArgMatches, raw: &[OsString]) -> i32 {
    let code = run_create(matches, raw);
    super::listen_feed::print_deferred_notices();
    code
}

fn run_create(matches: &ArgMatches, raw: &[OsString]) -> i32 {
    let config = Config::from_matches(matches);
    let (ingest_enabled, disabled_reason) =
        match super::intake::ingest_on_create() {
            Ok(setting) => setting,
            Err(error) => return report_create_error(error),
        };
    let keep_intake_reason = if matches.get_flag("keep-intake") {
        Some("requested with -k".to_string())
    } else if !ingest_enabled {
        Some(disabled_reason.to_string())
    } else {
        None
    };
    // Decode `--force[=MODE]` before dispatch: an adopted mode word
    // leaves TARGET, so single-target versus collection dispatch sees
    // the adopted list.
    let decoded = match super::force::decode_force(matches, raw) {
        Ok(decoded) => decoded,
        Err(super::force::ForceDecodeError::StrayMode { word }) => {
            return report_create_error(
                CommandError::new(super::force::stray_mode_message(&word))
                    .with_exit_code(2),
            );
        }
        Err(super::force::ForceDecodeError::MissingTarget) => {
            return report_missing_target();
        }
    };
    let force = decoded.mode;
    let targets = decoded.targets;
    if targets.len() > 1 {
        if matches.get_flag("closed") || matches.get_flag("plain") {
            return report_create_error(
                CommandError::new(
                    "--closed and --plain require a single vault-note TARGET\nhint: pass one note route, such as `bob ref create harbor_cranes -d`",
                )
                .with_exit_code(2),
            );
        }
        return super::collection::run(
            matches,
            &config,
            &targets,
            keep_intake_reason,
            force,
        );
    }
    let target_os = targets.into_iter().next().expect("required by clap");
    // Validate -N early for a clean error even though the planner
    // validates again.
    if let Some(name) = matches.get_one::<String>("name") {
        if let Err(error) = super::clip_url::validate_name(name) {
            let styler = Styler::detect();
            eprintln!("{COMMAND_NAME}: {}: {error}", styler.red("error"));
            return 1;
        }
    }
    let ref_type = matches.get_one::<String>("ref-type").cloned();
    if let Some(ref_type) = &ref_type {
        if let Err(error) = validate_ref_type(ref_type) {
            let styler = Styler::detect();
            eprintln!("{COMMAND_NAME}: {}: {error}", styler.red("error"));
            return 1;
        }
    }
    if let Some(published) = matches.get_one::<String>("published") {
        if let Err(error) = super::clip_url::validate_published(published) {
            let styler = Styler::detect();
            eprintln!("{COMMAND_NAME}: {}: {error}", styler.red("error"));
            return 1;
        }
    }
    if matches.contains_id("html")
        && matches.get_one::<OsString>("html").is_some()
        && {
            let raw = target_os.to_string_lossy();
            !super::target::looks_like_url(&raw)
        }
    {
        let styler = Styler::detect();
        eprintln!(
            "{COMMAND_NAME}: {}: --html requires an http(s) URL TARGET",
            styler.red("error")
        );
        return 1;
    }
    let mut prepared = match prepare_target(&config, &target_os) {
        Ok(target) => target,
        Err(error) => return report_create_error(error),
    };
    if !matches!(prepared, PreparedTarget::VaultNote { .. })
        && (matches.get_flag("closed")
            || matches.get_flag("plain")
            || matches.get_many::<String>("section").is_some())
    {
        return report_create_error(
            CommandError::new(
                "--closed, --plain, and --section require a vault-note TARGET\nhint: pass a note route, such as `bob ref create harbor_cranes -d`",
            )
            .with_exit_code(2),
        );
    }
    // Shared reread preflight: resolve duplicates and family defaults
    // before requiring -P, doing network work, or invoking adapters. A
    // filed family refuses here (no force carries the `-f` remedy with
    // the computed next stem; overwrite names it as unavailable), while
    // a selected family under reread force proceeds with its allocated
    // `<root>__vN` stem and newest-reading defaults. `-L` without reread
    // force keeps the legacy attach path, so it skips preflight.
    let reread = match preflight_before_parent(
        &config,
        &target_os,
        &prepared,
        matches,
        force,
        matches.get_flag("listen")
            && force != Some(super::force::ForceMode::Reread),
    ) {
        Ok(reread) => reread,
        Err(error) => return report_create_error(error),
    };
    let parent_input = matches.get_one::<String>("parent").cloned();
    // Duplicate before parent: on non-vault-note routes a duplicate
    // refuses (exit 1) before a missing -P errors (exit 2). `-L` attach
    // never needs -P. A resolved reread already passed its preflight,
    // so it skips this legacy check (reread narrates a new reading
    // instead of attaching, and carries its own parent default).
    if !matches!(prepared, PreparedTarget::VaultNote { .. })
        && parent_input.is_none()
        && reread.is_none()
    {
        if let Some(code) = check_duplicate_before_parent(
            &config, &target_os, &prepared, matches, force,
        ) {
            return code;
        }
    }
    let parent_input = matches.get_one::<String>("parent").cloned();
    let (parent, resolved_parent, parent_default_reason) =
        match resolve_parent_for_target(
            &config,
            &mut prepared,
            parent_input,
            reread.as_ref(),
        ) {
            Ok(parent) => parent,
            Err(error) => return report_create_error(error),
        };
    // Family defaults lose to explicit flags: the newest reading's type
    // fills in `-t` only when it is absent (explicit `-P` already won
    // inside parent resolution).
    let mut ref_type = matches.get_one::<String>("ref-type").cloned();
    if ref_type.is_none()
        && let Some(preflight) = &reread
        && let Some(default) = &preflight.defaults.ref_type
    {
        ref_type = Some(default.clone());
    }
    let options = CreateOptions {
        audio: matches.get_one::<OsString>("audio").map(PathBuf::from),
        author: matches.get_one::<String>("author").cloned(),
        closed: matches.get_flag("closed"),
        dry_run: matches.get_flag("dry-run"),
        force,
        html: matches.get_one::<OsString>("html").cloned(),
        include_id: matches.get_flag("include-id"),
        keep_intake_reason,
        listen: matches.get_flag("listen"),
        name: matches.get_one::<String>("name").cloned(),
        no_audio: matches.get_flag("no-audio"),
        output: matches.get_one::<OsString>("output").map(PathBuf::from),
        plain: matches.get_flag("plain"),
        parent,
        parent_default_reason,
        resolved_parent,
        published: matches.get_one::<String>("published").cloned(),
        sections: matches
            .get_many::<String>("section")
            .map(|values| values.cloned().collect())
            .unwrap_or_default(),
        ref_type,
        status: matches
            .get_one::<String>("status")
            .expect("defaulted by clap")
            .clone(),
        title: matches.get_one::<String>("title").cloned(),
        reread,
    };

    match create_pdf(&config, &target_os, &mut prepared, &options) {
        Ok(()) => 0,
        Err(error) => report_create_error(error),
    }
}

fn report_create_error(error: CommandError) -> i32 {
    let styler = Styler::detect();
    super::duplicate::report_error(COMMAND_NAME, error, &styler)
}

/// Report the normal missing-TARGET usage error after an adopted mode
/// word emptied TARGET. Re-parsing the bare subcommand keeps the
/// usage, message, and exit code identical to clap's own error.
fn report_missing_target() -> i32 {
    match command().try_get_matches_from(["bob ref create"]) {
        Ok(_) => {
            let styler = Styler::detect();
            eprintln!(
                "{COMMAND_NAME}: {}: internal: expected a missing TARGET",
                styler.red("error")
            );
            1
        }
        Err(error) => {
            let code = error.exit_code();
            if let Err(print_error) = error.print()
                && print_error.kind() != std::io::ErrorKind::BrokenPipe
            {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: {print_error}"
                );
            }
            code
        }
    }
}

fn prepare_target(
    config: &Config,
    target_os: &OsStr,
) -> Result<PreparedTarget> {
    if target_os.to_str().is_some_and(target_mod::looks_like_url) {
        return Ok(PreparedTarget::Url);
    }
    let path = Path::new(target_os);
    if path.exists() {
        return match target_mod::resolve_local_target(path)? {
            target_mod::CreateSource::Markdown(path) => {
                let canonical = fs::canonicalize(&path).map_err(|error| {
                    CommandError::new(format!(
                        "resolve Markdown file {}: {error}",
                        path.display()
                    ))
                })?;
                let member = note_snapshot::vault_member_relative(
                    &config.bob_dir,
                    &canonical,
                );
                let utf8_identity = member.as_ref().is_some_and(|relative| {
                    relative.components().all(|component| {
                        component.as_os_str().to_str().is_some()
                    })
                });
                if !utf8_identity {
                    return Ok(PreparedTarget::Markdown(path));
                }
                let mut store = open_snapshot_store(config)?;
                let raw = path.to_str().ok_or_else(|| {
                    CommandError::new(format!(
                        "Markdown file path is not UTF-8: {}",
                        path.display()
                    ))
                })?;
                match note_snapshot::classify_target(&mut store, raw)
                    .map_err(|error| CommandError::new(error.to_string()))?
                {
                    note_snapshot::SnapshotTarget::VaultNote(note) => {
                        Ok(PreparedTarget::VaultNote { store, note })
                    }
                    note_snapshot::SnapshotTarget::PlainMarkdown(path) => {
                        Ok(PreparedTarget::Markdown(path))
                    }
                    note_snapshot::SnapshotTarget::Pdf(path) => {
                        Ok(PreparedTarget::Pdf(path))
                    }
                    note_snapshot::SnapshotTarget::OtherExistingFile(path) => {
                        Err(CommandError::new(format!(
                            "TARGET must be a Markdown file (.md), a PDF file, or an http(s) URL: {}",
                            path.display()
                        )))
                    }
                }
            }
            target_mod::CreateSource::LocalPdf(path) => {
                Ok(PreparedTarget::Pdf(path))
            }
            _ => Err(CommandError::new("unexpected target classification")),
        };
    }
    if target_os.to_str().is_none() {
        return Err(target_mod::resolve_local_target(path).unwrap_err());
    }
    let mut store = open_snapshot_store(config)?;
    match note_snapshot::classify_target(
        &mut store,
        target_os.to_str().expect("checked UTF-8 target"),
    )
    .map_err(|error| CommandError::new(error.to_string()))?
    {
        note_snapshot::SnapshotTarget::VaultNote(note) => {
            Ok(PreparedTarget::VaultNote { store, note })
        }
        note_snapshot::SnapshotTarget::PlainMarkdown(path) => {
            Ok(PreparedTarget::Markdown(path))
        }
        note_snapshot::SnapshotTarget::Pdf(path) => {
            Ok(PreparedTarget::Pdf(path))
        }
        note_snapshot::SnapshotTarget::OtherExistingFile(path) => {
            Err(CommandError::new(format!(
                "TARGET must be a Markdown file (.md), a PDF file, or an http(s) URL: {}",
                path.display()
            )))
        }
    }
}

fn open_snapshot_store(config: &Config) -> Result<note_snapshot::SourceStore> {
    note_snapshot::SourceStore::open(&config.bob_dir).map_err(|error| {
        CommandError::new(format!(
            "open vault source cache {}: {error}",
            config.bob_dir.display()
        ))
    })
}

fn resolve_parent_for_target(
    config: &Config,
    target: &mut PreparedTarget,
    parent_input: Option<String>,
    reread: Option<&RereadPreflight>,
) -> Result<(
    String,
    Option<crate::native::parent_notes::ResolvedParent>,
    Option<String>,
)> {
    match target {
        PreparedTarget::VaultNote { store, note } => {
            if let Some(input) = parent_input {
                let resolved = crate::native::parent_notes::resolve_parent(
                    &config.bob_dir,
                    &input,
                )
                .map_err(|error| CommandError::new(error.message()))?;
                return Ok((resolved.route.clone(), Some(resolved), None));
            }
            // A same-day reread carries the newest reading's located task
            // residence as its parent default; explicit `-P` already won
            // above. The shared preflight runs before parent resolution
            // so the refusal wins over the missing-parent error.
            if let Some(preflight) = reread
                && let Some(task_path) = &preflight.defaults.parent
                && let Ok(resolved) =
                    crate::native::parent_notes::resolve_parent(
                        &config.bob_dir,
                        task_path,
                    )
            {
                let reason = format!("newest reading's task in {task_path}");
                return Ok((
                    resolved.route.clone(),
                    Some(resolved),
                    Some(reason),
                ));
            }
            let Some(default) = note_snapshot::default_parent(store, note)
            else {
                return Err(missing_parent_error());
            };
            Ok((
                default.resolved.route.clone(),
                Some(default.resolved),
                Some(default.reason),
            ))
        }
        _ => {
            if let Some(input) = parent_input {
                let resolved = crate::native::parent_notes::resolve_parent(
                    &config.bob_dir,
                    &input,
                )
                .map_err(|error| CommandError::new(error.message()))?;
                return Ok((resolved.route.clone(), Some(resolved), None));
            }
            // A reread carries the newest reading's located task residence
            // as its parent default; explicit `-P` already won above.
            if let Some(preflight) = reread
                && let Some(task_path) = &preflight.defaults.parent
                && let Ok(resolved) =
                    crate::native::parent_notes::resolve_parent(
                        &config.bob_dir,
                        task_path,
                    )
            {
                let reason = format!("newest reading's task in {task_path}");
                return Ok((
                    resolved.route.clone(),
                    Some(resolved),
                    Some(reason),
                ));
            }
            Err(missing_parent_error())
        }
    }
}

pub(super) fn missing_parent_error() -> CommandError {
    CommandError::new(
        "--parent is required\nhint: pass -P <note>, for example `-P sase`; list area and project notes with `bob capture-targets`",
    )
    .with_exit_code(2)
}

/// Shared reread preflight for single-source routes, resolved before `-P`
/// is required and before any fetch or adapter work. Vault-note targets
/// compute their dated stem (honoring `BOB_NOW` and `-N`) here so the
/// refusal wins over the missing-parent error and rereads carry the
/// newest reading's parent default; new-day snapshots stay ordinary
/// dated creates.
fn preflight_before_parent(
    config: &Config,
    target_os: &OsString,
    prepared: &PreparedTarget,
    matches: &ArgMatches,
    force: Option<super::force::ForceMode>,
    skip_for_attach: bool,
) -> Result<Option<RereadPreflight>> {
    if skip_for_attach {
        return Ok(None);
    }
    let name = matches.get_one::<String>("name").map(String::as_str);
    let output = matches.get_one::<OsString>("output").map(Path::new);
    match prepared {
        PreparedTarget::Url => {
            let raw = target_os.to_string_lossy().into_owned();
            let Ok((url, arxiv)) = super::target::resolve_url_syntactic(&raw)
            else {
                return Ok(None);
            };
            let recorded = sources_mod::collect_recorded_source_urls(config)
                .map_err(|error| CommandError::new(error.message.clone()))?;
            let rows = super::recapture::index_rows(config);
            let mut wanted = std::collections::BTreeSet::new();
            wanted.insert(url.dedupe_key.clone());
            if let Some(paper) = &arxiv {
                wanted.insert(paper.dedupe_key());
            }
            super::recapture::preflight_for_url(
                config,
                &rows,
                &recorded,
                &wanted,
                name,
                output,
                force,
                &url.cleaned,
            )
        }
        PreparedTarget::Markdown(path) => {
            let canonical = match fs::canonicalize(path) {
                Ok(canonical) => canonical,
                Err(_) => return Ok(None),
            };
            let markdown = match fs::read_to_string(&canonical) {
                Ok(markdown) => markdown,
                Err(_) => return Ok(None),
            };
            let stem = match name {
                Some(name) => name.to_string(),
                None => {
                    match canonical.file_stem().and_then(|stem| stem.to_str()) {
                        Some(stem) => stem.to_string(),
                        None => return Ok(None),
                    }
                }
            };
            let title = match matches.get_one::<String>("title") {
                Some(override_title) if !override_title.is_empty() => {
                    override_title.clone()
                }
                _ => match extract_title(&markdown, &canonical) {
                    Ok(title) => title,
                    Err(_) => return Ok(None),
                },
            };
            let rows = super::recapture::index_rows(config);
            super::recapture::preflight_for_stem(
                config,
                &rows,
                &stem,
                name,
                output,
                force,
                &title,
                super::recapture::StemIdentity::Title(&title),
            )
        }
        PreparedTarget::Pdf(path) => {
            // A library/intake PDF as TARGET is never a reread source:
            // copying highlights into the next reading is forbidden. The
            // identity check below keeps refusing (with `-L` attach), so
            // preflight stays out of its way.
            if let Ok(canonical) = fs::canonicalize(path) {
                let inside = super::relative_inside_canonical(
                    &canonical,
                    &config.lib_dir,
                )
                .is_some()
                    || super::relative_inside_canonical(
                        &canonical,
                        &config.xlib_dir,
                    )
                    .is_some();
                if inside {
                    return Ok(None);
                }
            }
            let canonical = match fs::canonicalize(path) {
                Ok(canonical) => canonical,
                Err(_) => return Ok(None),
            };
            let pdf_plan = match super::pdf_target::plan_local_pdf(
                &canonical,
                name,
                matches.get_one::<String>("title").map(String::as_str),
                matches.get_one::<String>("author").map(String::as_str),
                matches.get_one::<String>("published").map(String::as_str),
                None,
            ) {
                Ok(plan) => plan,
                Err(_) => return Ok(None),
            };
            let rows = super::recapture::index_rows(config);
            super::recapture::preflight_for_stem(
                config,
                &rows,
                &pdf_plan.stem,
                name,
                output,
                force,
                &pdf_plan.title,
                super::recapture::StemIdentity::Title(&pdf_plan.title),
            )
        }
        PreparedTarget::VaultNote { note, .. } => {
            // The dated stem is known without snapshot planning:
            // `<note-stem>_<YYMMDD>` from `BOB_NOW`, overridden by `-N`.
            // Same-day duplicates reread as `<dated-root>__vN`; a new day
            // keeps its ordinary dated create.
            let source_route = note.identity.route.clone();
            let source_stem = source_route
                .rsplit('/')
                .next()
                .filter(|stem| !stem.is_empty())
                .unwrap_or(source_route.as_str())
                .to_string();
            let snapshot_date = bob_env::current_datetime().date();
            let dated_stem =
                format!("{source_stem}_{}", snapshot_date.format("%y%m%d"));
            let stem = name.unwrap_or(dated_stem.as_str()).to_string();
            let title_fallback = matches
                .get_one::<String>("title")
                .filter(|title| !title.is_empty())
                .map(String::as_str)
                .unwrap_or(stem.as_str())
                .to_string();
            let rows = super::recapture::index_rows(config);
            super::recapture::preflight_for_stem(
                config,
                &rows,
                &stem,
                name,
                output,
                force,
                &title_fallback,
                super::recapture::StemIdentity::SourceNote(&source_route),
            )
        }
    }
}

/// Duplicate-before-parent pre-check for non-vault-note routes when `-P`
/// is missing: report an already-captured refusal (exit 1) instead of the
/// missing-parent error (exit 2), and run `-L` attach without a parent.
/// Returns `Some(exit code)` when the pre-check decided, `None` when the
/// caller should proceed to parent resolution.
fn check_duplicate_before_parent(
    config: &Config,
    target_os: &OsString,
    prepared: &PreparedTarget,
    matches: &ArgMatches,
    force: Option<super::force::ForceMode>,
) -> Option<i32> {
    let source_display = target_os.to_string_lossy().into_owned();
    let force_flag = force.is_some();
    let listen = matches.get_flag("listen");
    match prepared {
        PreparedTarget::Url => {
            let raw = target_os.to_string_lossy().into_owned();
            let Ok((url, arxiv)) = super::target::resolve_url_syntactic(&raw)
            else {
                return None;
            };
            let recorded =
                match sources_mod::collect_recorded_source_urls(config) {
                    Ok(recorded) => recorded,
                    Err(error) => {
                        return Some(report_create_error(CommandError::new(
                            error.message.clone(),
                        )));
                    }
                };
            let dedupe_key = match &arxiv {
                Some(paper) => paper.dedupe_key(),
                None => url.dedupe_key.clone(),
            };
            if listen {
                if let Some(hit) = sources_mod::find_refusing_hit(
                    &recorded,
                    &dedupe_key,
                    None,
                    force_flag,
                ) {
                    let hit = super::recapture::newest_reading_hit(
                        config, &recorded, &hit,
                    );
                    let command = match super::listen::require_validated() {
                        Ok(command) => command,
                        Err(error) => {
                            return Some(report_create_error(
                                error.into_command_error(),
                            ));
                        }
                    };
                    let attach =
                        match attach_mod::attach_for_dedupe_hit(config, &hit) {
                            Ok(attach) => attach,
                            Err(error) => {
                                return Some(report_create_error(error))
                            }
                        };
                    let (source_line, target_value) =
                        url_attach_lines(&url, &arxiv);
                    let mut scratch =
                        match super::ScratchDir::create("create-attach") {
                            Ok(scratch) => scratch,
                            Err(error) => {
                                return Some(report_create_error(error))
                            }
                        };
                    let keep = if matches.get_flag("keep-intake") {
                        Some("requested with -k".to_string())
                    } else {
                        None
                    };
                    let dry_run = matches.get_flag("dry-run");
                    match attach_mod::run_attach(
                        config,
                        &command,
                        &attach,
                        &source_line,
                        &target_value,
                        dry_run,
                        keep.as_deref(),
                        &mut scratch,
                    ) {
                        Ok(()) => return Some(0),
                        Err(error) => {
                            return Some(report_create_error(error));
                        }
                    }
                }
            }
            // Overwrite triage runs after `-L` attach (attach is not a
            // fresh capture) and before the plain refusal. An existing
            // exact-output file keeps the old replacement permission.
            if is_overwrite(force)
                && let Err(error) = triage_overwrite_url(
                    &recorded,
                    &dedupe_key,
                    &source_display,
                    matches
                        .get_one::<OsString>("output")
                        .map(std::path::Path::new),
                )
            {
                return Some(report_create_error(error));
            }
            match sources_mod::check_dedupe(
                config,
                &recorded,
                &dedupe_key,
                None,
                force_flag,
            ) {
                Ok(()) => None,
                Err(error) => Some(report_create_error(CommandError::new(
                    error.message.clone(),
                ))),
            }
        }
        PreparedTarget::Markdown(path) => check_markdown_stamp_before_parent(
            config,
            path,
            matches,
            force,
            &source_display,
        ),
        PreparedTarget::Pdf(path) => check_pdf_stamp_before_parent(
            config,
            path,
            matches,
            force,
            &source_display,
        ),
        PreparedTarget::VaultNote { .. } => None,
    }
}

fn check_markdown_stamp_before_parent(
    config: &Config,
    source: &Path,
    matches: &ArgMatches,
    force: Option<super::force::ForceMode>,
    source_display: &str,
) -> Option<i32> {
    // An existing exact-output file keeps the old replacement permission
    // under overwrite: skip the default-target duplicate probe and let
    // the exact-output planner decide.
    if is_overwrite(force)
        && matches
            .get_one::<OsString>("output")
            .is_some_and(|output| Path::new(output).exists())
    {
        return None;
    }
    let canonical = fs::canonicalize(source).ok()?;
    let markdown = fs::read_to_string(&canonical).ok()?;
    let stem = match matches.get_one::<String>("name") {
        Some(name) => super::clip_url::validate_name(name).ok()?,
        None => canonical
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)?,
    };
    let title = match matches.get_one::<String>("title") {
        Some(override_title) if !override_title.is_empty() => {
            override_title.clone()
        }
        _ => extract_title(&markdown, &canonical).ok()?,
    };
    let ref_type = matches
        .get_one::<String>("ref-type")
        .cloned()
        .unwrap_or_else(|| DEFAULT_MARKDOWN_REF_TYPE.to_string());
    if validate_ref_type(&ref_type).is_err() {
        return None;
    }
    let recorded = sources_mod::collect_recorded_source_urls(config).ok()?;
    let identity = DefaultTargetIdentity::Title(title.clone());
    // Keep a clone for the refusal title; the identity borrows it.
    let _ = &recorded;
    match plan_default_target(
        config,
        std::ffi::OsStr::new(&stem),
        &ref_type,
        force.is_some(),
        &identity,
    ) {
        Ok(plan) => {
            if is_overwrite(force) && !plan.target.exists() {
                return Some(report_create_error(nothing_to_overwrite(
                    source_display,
                )));
            }
            None
        }
        // The shared preflight already refused filed families with the
        // typed reread remedy (or the typed overwrite interim message),
        // so no string-matched overwrite fallback remains here.
        Err(error) => Some(report_create_error(error)),
    }
}

fn check_pdf_stamp_before_parent(
    config: &Config,
    source: &Path,
    matches: &ArgMatches,
    force: Option<super::force::ForceMode>,
    source_display: &str,
) -> Option<i32> {
    // An existing exact-output file keeps the old replacement permission
    // under overwrite: skip the default-target duplicate probe and let
    // the exact-output planner decide.
    if is_overwrite(force)
        && matches
            .get_one::<OsString>("output")
            .is_some_and(|output| Path::new(output).exists())
    {
        return None;
    }
    if let Err(error) = pdf_target_mod::check_local_pdf_identity(config, source)
    {
        // Only pre-empt the parent error for duplicates; other
        // library/intake placement errors still surface after `-P`. The
        // shared preflight already refused filed families with the typed
        // remedy, so no string-matched overwrite fallback remains here.
        if error.message.contains("already captured")
            || error.message.contains("already queued")
        {
            return Some(report_create_error(error));
        }
        return None;
    }
    let canonical = fs::canonicalize(source).ok()?;
    let pdf_plan = pdf_target_mod::plan_local_pdf(
        &canonical,
        matches.get_one::<String>("name").map(String::as_str),
        matches.get_one::<String>("title").map(String::as_str),
        matches.get_one::<String>("author").map(String::as_str),
        matches.get_one::<String>("published").map(String::as_str),
        None,
    )
    .ok()?;
    let ref_type = matches
        .get_one::<String>("ref-type")
        .cloned()
        .unwrap_or_else(|| DEFAULT_PDF_REF_TYPE.to_string());
    if validate_ref_type(&ref_type).is_err() {
        return None;
    }
    let recorded = sources_mod::collect_recorded_source_urls(config).ok()?;
    let identity = DefaultTargetIdentity::Title(pdf_plan.title.clone());
    let _ = &recorded;
    match plan_default_target(
        config,
        std::ffi::OsStr::new(&pdf_plan.stem),
        &ref_type,
        force.is_some(),
        &identity,
    ) {
        Ok(plan) => {
            if is_overwrite(force) && !plan.target.exists() {
                return Some(report_create_error(nothing_to_overwrite(
                    source_display,
                )));
            }
            None
        }
        // The shared preflight already refused filed families with the
        // typed reread remedy (or the typed overwrite interim message),
        // so no string-matched overwrite fallback remains here.
        Err(error) => Some(report_create_error(error)),
    }
}

pub(super) fn pandoc_command() -> Option<OsString> {
    if let Some(command) =
        bob_env::var_os(ENV_PANDOC_COMMAND).filter(|value| !value.is_empty())
    {
        return Some(command);
    }

    let path = bob_env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join("pandoc"))
        .find(|candidate| candidate.is_file())
        .map(PathBuf::into_os_string)
}

fn create_pdf(
    config: &Config,
    target_os: &OsString,
    prepared: &mut PreparedTarget,
    options: &CreateOptions,
) -> Result<()> {
    // With --listen the command is resolved and validated before any
    // fetch or render, so an unconfigured or invalid command fails
    // before vault work starts.
    let listen = match options.listen {
        true => Some(
            super::listen::require_validated()
                .map_err(|error| error.into_command_error())?,
        ),
        false => None,
    };
    let mut scratch = super::ScratchDir::create("create")?;
    let target_lossy = target_os.to_string_lossy().into_owned();
    let target_raw = target_lossy.as_str();
    // Syntactic classification first; URL dedupe runs before any fetch.
    if matches!(prepared, PreparedTarget::Url) {
        let (url, arxiv) = target_mod::resolve_url_syntactic(target_raw)?;
        let recorded = sources_mod::collect_recorded_source_urls(config)
            .map_err(|error| CommandError::new(error.message.clone()))?;
        let dedupe_key = match &arxiv {
            Some(paper) => paper.dedupe_key(),
            None => url.dedupe_key.clone(),
        };
        // A shared reread preflight already selected the family: it skips
        // the attach path (a reread narrates a new reading) and both
        // dedupe probes. Legacy-only matches never produce a preflight.
        let rereading = options.reread.is_some();
        if !rereading
            && let Some(command) = &listen
            && let Some(hit) = sources_mod::find_refusing_hit(
                &recorded,
                &dedupe_key,
                None,
                options.force.is_some(),
            )
        {
            let hit =
                super::recapture::newest_reading_hit(config, &recorded, &hit);
            let attach = attach_mod::attach_for_dedupe_hit(config, &hit)?;
            let (source_line, target_value) = url_attach_lines(&url, &arxiv);
            return attach_mod::run_attach(
                config,
                command,
                &attach,
                &source_line,
                &target_value,
                options.dry_run,
                options.keep_intake_reason.as_deref(),
                &mut scratch,
            );
        }
        // Overwrite triage runs after `-L` attach (attach is not a
        // fresh capture) and before any fetch or adapter work. A reread
        // already passed triage at preflight. An existing exact-output
        // file keeps the old replacement permission.
        if !rereading && is_overwrite(options.force) {
            triage_overwrite_url(
                &recorded,
                &dedupe_key,
                target_raw,
                options.output.as_deref().map(std::path::Path::new),
            )?;
        }
        if !rereading {
            check_dedupe_with_listen_hint(
                config,
                &recorded,
                &dedupe_key,
                None,
                options.force.is_some(),
            )?;
            // A URL recorded only by legacy notes without a Highlights PDF
            // warns here, before any fetch, and captures a fresh copy. The
            // arXiv and PDF routes share this key (`dedupe_key_for` is
            // arXiv-aware), so they must not warn again; the article route
            // is told the warning already fired.
            sources_mod::warn_for_legacy_hits(&sources_mod::legacy_hits(
                &recorded,
                &dedupe_key,
            ));
        }
        if options.html.is_some() {
            return create_article_route(config, url, options, listen.as_ref());
        }
        if let Some(paper) = arxiv {
            return create_arxiv_route(
                config,
                paper,
                url,
                &mut scratch,
                options,
                &recorded,
                listen.as_ref(),
            );
        }
        match target_mod::fetch_and_route(
            url.clone(),
            &scratch,
            Some(&fetch_progress),
        )? {
            target_mod::CreateSource::PdfUrl {
                url,
                downloaded,
                final_url,
            } => {
                return create_pdf_url_route(
                    config,
                    url,
                    downloaded,
                    final_url,
                    &mut scratch,
                    options,
                    &recorded,
                    listen.as_ref(),
                );
            }
            target_mod::CreateSource::WebArticle { url } => {
                return create_article_route(
                    config,
                    url,
                    options,
                    listen.as_ref(),
                );
            }
            _ => {
                return Err(CommandError::new(
                    "unexpected target classification",
                ));
            }
        }
    }
    match prepared {
        PreparedTarget::VaultNote { store, note } => create_vault_note_route(
            config,
            store,
            note.clone(),
            &mut scratch,
            options,
            listen.as_ref(),
            target_raw,
        ),
        PreparedTarget::Markdown(path) => create_markdown_route(
            config,
            path,
            &mut scratch,
            options,
            listen.as_ref(),
            target_raw,
        ),
        PreparedTarget::Pdf(path) => create_local_pdf_route(
            config,
            path,
            &mut scratch,
            options,
            listen.as_ref(),
            target_raw,
        ),
        PreparedTarget::Url => {
            Err(CommandError::new("unexpected target classification"))
        }
    }
}

/// The attach report's source line plus the `{target}` listen value for
/// a URL that hit dedupe before any fetch.
fn url_attach_lines(
    url: &super::clip_url::WebUrl,
    arxiv: &Option<super::arxiv::ArxivPaper>,
) -> (String, String) {
    match arxiv {
        Some(paper) => (
            format!("source: {} (arXiv {})", paper.abs_url(), paper.full_id()),
            url.cleaned.clone(),
        ),
        None => (format!("source: {}", url.cleaned), url.cleaned.clone()),
    }
}

/// A display-only [`AudioCopyPlan`] for a planned listen episode, so
/// dry runs and reports share the `audio: … (from --listen)` line.
fn listen_audio_plan(flow: &super::listen::ListenFlow) -> AudioCopyPlan {
    AudioCopyPlan {
        source: flow.scratch_audio.clone(),
        dest: flow.dest.clone(),
        library_dest: flow.library_dest.clone(),
        origin: "--listen".to_string(),
        reused: false,
    }
}

/// Recovery hint for a post-listen install failure: rebind the kept
/// episode explicitly.
fn bind_create_hint(target_display: &str, scratch_audio: &Path) -> String {
    format!(
        "bind it with bob ref create {target_display} --audio {}",
        scratch_audio.display()
    )
}

/// Stamp the PDF into scratch *before* the listen, then run the listen,
/// re-check collisions, install the audio, and install the stamped PDF.
/// Stamping first fails fast before the paid render and leaves no orphan
/// audio when it fails. `{pdf}` stays the unstamped scratch copy.
#[allow(clippy::too_many_arguments)]
fn stamp_listen_and_install_pdf(
    scratch: &mut super::ScratchDir,
    target_plan: &TargetPlan,
    command: &super::listen::ListenCommand,
    flow: &super::listen::ListenFlow,
    listen_target: &str,
    listen_pdf: &std::path::Path,
    listen_title: &str,
    hint_source: &str,
    stamp_source: &Path,
    marker: &str,
    info: &PdfInfo,
    force: bool,
) -> Result<usize> {
    // Fail fast before the paid render: no audio exists yet, so a stamp
    // failure is a plain error with nothing to clean up.
    let (stamped, page_count) = pdf_target_mod::stamp_pdf_to_scratch(
        stamp_source,
        scratch,
        marker,
        info,
    )?;
    let values = super::listen::ListenValues {
        target: listen_target.to_string(),
        pdf: listen_pdf.to_path_buf(),
        audio: flow.scratch_audio.clone(),
        title: listen_title.to_string(),
    };
    super::listen::run_listen(command, &values)
        .map_err(|error| error.into_command_error())?;
    // The vault may have changed during a long listen.
    if let Err(error) = super::refuse_target_collisions(
        &target_plan.target,
        &target_plan.sidecar,
        &target_plan.workflow,
        force,
    ) {
        return Err(super::listen::post_listen_error(
            scratch,
            &flow.scratch_audio,
            error.to_string(),
            bind_create_hint(hint_source, &flow.scratch_audio),
        ));
    }
    let created =
        super::listen::install_listen_audio(flow, force).map_err(|error| {
            super::listen::post_listen_error(
                scratch,
                &flow.scratch_audio,
                error.to_string(),
                bind_create_hint(hint_source, &flow.scratch_audio),
            )
        })?;
    if let Err(error) = super::atomic_copy(&stamped, &target_plan.target) {
        companion_mod::cleanup_audio_on_failure(created.as_ref());
        return Err(super::listen::post_listen_error(
            scratch,
            &flow.scratch_audio,
            error.to_string(),
            bind_create_hint(hint_source, &flow.scratch_audio),
        ));
    }
    Ok(page_count)
}

fn create_article_route(
    config: &Config,
    url: super::clip_url::WebUrl,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let ref_type =
        default_ref_type_for_kind("article", options.ref_type.as_deref())?;
    let validated_name = options
        .name
        .clone()
        .map(|name| super::clip_url::validate_name(&name))
        .transpose()?;
    validate_ref_type(&ref_type)?;
    let clip_options = super::clip::ClipOptions {
        dry_run: options.dry_run,
        force: options.force,
        author: options.author.clone(),
        html: options.html.clone(),
        keep_intake_reason: options.keep_intake_reason.clone(),
        name: validated_name,
        output: options.output.clone(),
        parent: options.parent.clone(),
        published: options.published.clone(),
        ref_type,
        status: options.status.clone(),
        title: options.title.clone(),
        reread: options.reread.clone(),
    };
    // `-i` is accepted and is a no-op: the clip engine always stamps `id`.
    // (`-L` conflicts with `-a` at the CLI, so both can never be set.)
    let companion = match (&options.audio, listen) {
        (Some(path), None) => super::clip::Companion::Explicit(path.clone()),
        (None, Some(command)) => {
            super::clip::Companion::Listen(command.clone())
        }
        (None, None) => super::clip::Companion::None,
        (Some(_), Some(_)) => {
            return Err(CommandError::new(
                "--listen cannot be used with --audio",
            ));
        }
    };
    // The top-level dispatch already warned about legacy-only hits
    // before its fetch.
    super::clip::capture_article(
        config,
        &url.cleaned,
        &clip_options,
        companion,
        true,
    )?;
    Ok(())
}

fn check_dedupe_with_listen_hint(
    config: &Config,
    recorded: &[sources_mod::RecordedSource],
    dedupe_key: &str,
    planned_target: Option<&Path>,
    force: bool,
) -> Result<()> {
    sources_mod::check_dedupe(
        config,
        recorded,
        dedupe_key,
        planned_target,
        force,
    )
    .map_err(|error| CommandError::new(error.message.clone()))
}

/// Print the reread report line for an enabled reread, else the
/// existing `renamed:` line. Rereads occupy the `renamed:` position in
/// both real runs and dry runs. A narrated reread (`-f` + `-L`) also
/// names the earlier episode it leaves behind, while a plain reread
/// without `-L` or `-a` names the newest narrated earlier reading (if
/// any), since earlier audio is never inherited.
fn print_reread_or_renamed(plan: &TargetPlan, options: &CreateOptions) {
    if let Some(preflight) = &options.reread {
        println!("{}", preflight.plan.report_line());
        if options.listen {
            super::recapture::print_reread_audio_hint(preflight, true);
        } else if options.audio.is_none() {
            super::recapture::print_reread_audio_hint(preflight, false);
        }
    } else if let Some(line) = plan.renamed_line() {
        println!("{line}");
    }
}

fn default_ref_type_for_kind(
    kind: &str,
    explicit: Option<&str>,
) -> Result<String> {
    if let Some(ref_type) = explicit {
        validate_ref_type(ref_type)?;
        return Ok(ref_type.to_string());
    }
    Ok(match kind {
        "markdown" => DEFAULT_MARKDOWN_REF_TYPE.to_string(),
        "pdf" | "arxiv" => DEFAULT_PDF_REF_TYPE.to_string(),
        "article" => DEFAULT_ARTICLE_REF_TYPE.to_string(),
        "note" => DEFAULT_NOTE_REF_TYPE.to_string(),
        _ => DEFAULT_MARKDOWN_REF_TYPE.to_string(),
    })
}

/// Plan the intake target for the vault-note, Markdown, local-PDF,
/// PDF-URL, and arXiv routes. Under `--force=overwrite`, a filed
/// refusal stays refused with the interim exit-2 message, and a free
/// target refuses as fresh (exit 1) instead of capturing: only an
/// existing exact-output or queued target keeps the old replacement
/// permission.
fn plan_target_for(
    config: &Config,
    stem: &str,
    ref_type: &str,
    output: Option<&PathBuf>,
    force: Option<super::force::ForceMode>,
    include_id: bool,
    identity: &DefaultTargetIdentity<'_>,
    source_display: &str,
) -> Result<TargetPlan> {
    // The shared preflight already refused filed families with the typed
    // reread remedy (or the typed overwrite interim message): exact-output
    // planning never reports `already captured`, and a default-target
    // `already captured` would mean the preflight missed a family, so no
    // string-matched overwrite fallback remains here.
    let plan = match output {
        Some(output) => {
            let plan = plan_exact_output(config, output, force.is_some())?;
            if include_id {
                super::stamp::ensure_reference_id_available(
                    config, &plan.stem,
                )?;
            }
            plan
        }
        None => plan_default_target(
            config,
            std::ffi::OsStr::new(stem),
            ref_type,
            force.is_some(),
            identity,
        )?,
    };
    if is_overwrite(force) && !plan.target.exists() {
        return Err(nothing_to_overwrite(source_display));
    }
    Ok(plan)
}

fn create_vault_note_route(
    config: &Config,
    store: &mut note_snapshot::SourceStore,
    note: note_snapshot::ClassifiedNote,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
    source_display: &str,
) -> Result<()> {
    let snapshot_date = bob_env::current_datetime().date();
    let settings =
        crate::native::task_status_hooks::read_tasks_settings(&config.bob_dir);
    let parent = options
        .resolved_parent
        .clone()
        .ok_or_else(missing_parent_error)?;
    let snapshot = note_snapshot::plan_snapshot(
        store,
        note,
        &settings,
        snapshot_date,
        options.title.as_deref(),
        options.closed,
        &options.sections,
        parent,
        options.parent_default_reason.clone(),
    )
    .map_err(CommandError::new)?;

    let stem = match &options.name {
        Some(name) => super::clip_url::validate_name(name)?,
        None => snapshot.dated_stem.clone(),
    };
    let source_note = snapshot.note.identity.route.clone();
    // Same-day duplicates reread as `<dated-root>__vN`; a new day keeps
    // its ordinary dated create. The dated stem is known only after
    // snapshot planning, so the vault route preflights here.
    let rows = super::recapture::index_rows(config);
    let reread = super::recapture::preflight_for_stem(
        config,
        &rows,
        &stem,
        options.name.as_deref(),
        options.output.as_deref(),
        options.force,
        &snapshot.identity_title,
        super::recapture::StemIdentity::SourceNote(&source_note),
    )?;
    // Family defaults lose to explicit flags: the newest reading's type
    // fills in `-t` only when it is absent.
    let ref_type = match (&options.ref_type, &reread) {
        (Some(explicit), _) => {
            validate_ref_type(explicit)?;
            explicit.clone()
        }
        (None, Some(preflight)) if preflight.defaults.ref_type.is_some() => {
            preflight
                .defaults
                .ref_type
                .clone()
                .expect("checked ref-type default")
        }
        (None, _) => default_ref_type_for_kind("note", None)?,
    };
    let identity = DefaultTargetIdentity::SourceNote {
        source_note: source_note.clone(),
        title: snapshot.identity_title.clone(),
    };
    // A reread plans its allocated `<root>__vN` stem with no suffix
    // fallback and always stamps `id`; `-o` on a captured reread was
    // refused at preflight.
    let target_plan = match &reread {
        Some(preflight) => super::recapture::plan_reread_target(
            config,
            &preflight.plan.stem,
            &ref_type,
            &identity,
        )?,
        None => plan_target_for(
            config,
            &stem,
            &ref_type,
            options.output.as_ref(),
            options.force,
            options.include_id,
            &identity,
            source_display,
        )?,
    };
    let final_stem = target_plan.stem.clone();
    let id = if options.include_id || reread.is_some() {
        Some(final_stem)
    } else {
        None
    };
    // Carry the vault preflight into the shared reports below.
    let owned = CreateOptions {
        reread,
        ..options.clone()
    };
    let options = &owned;
    let author = options.author.clone().filter(|value| !value.is_empty());
    let published = options.published.clone().filter(|value| !value.is_empty());
    let marker = compose_markdown_marker(
        &options.status,
        &options.parent,
        &snapshot.identity_title,
        id.as_deref(),
        author.as_deref(),
        published.as_deref(),
        Some(&source_note),
        false,
    )?;
    let audio = plan_audio_copy(
        config,
        &snapshot.note.canonical_path,
        &snapshot.source_markdown,
        &target_plan,
        options,
    )?;
    let plan = CreatePlan {
        source: snapshot.note.canonical_path.display().to_string(),
        source_kind: "vault note",
        target: target_plan.target.clone(),
        sidecar: target_plan.sidecar.clone(),
        workflow: target_plan.workflow.clone(),
        stem: target_plan.stem.clone(),
        renamed_from: target_plan.renamed_from.clone(),
        title: snapshot.identity_title.clone(),
        title_source: snapshot.title_source.as_str(),
        author,
        published,
        source_url: None,
        source_note: Some(source_note),
        source_markdown: Some(snapshot.source_markdown.clone()),
        captured: None,
        id,
        marker,
        audio,
        page_count_hint: None,
    };
    if options.plain {
        return create_markdown_plan(config, plan, scratch, options, listen);
    }
    create_snapshot_plan(config, plan, snapshot, scratch, options, listen)
}

fn create_snapshot_plan(
    config: &Config,
    mut plan: CreatePlan,
    mut snapshot: note_snapshot::SnapshotPlan,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let asset_preparation = note_snapshot::assets::prepare(&mut snapshot)
        .map_err(CommandError::new)?;
    let stem = plan
        .target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            CommandError::new(format!(
                "target has no file stem: {}",
                plan.target.display()
            ))
        })?
        .to_string();
    let listen_flow = match listen {
        Some(_) => {
            let flow = super::listen::plan_listen_flow(
                &plan.target_plan(),
                scratch,
                &stem,
            )?;
            plan.audio = Some(listen_audio_plan(&flow));
            Some(flow)
        }
        None => None,
    };
    let audio_uri = play_uri(config, &plan)?;
    if options.dry_run {
        if let (Some(command), Some(flow)) = (listen, listen_flow.as_ref()) {
            println!(
                "{}",
                super::listen::would_run_line(
                    command,
                    &super::listen::ListenValues {
                        target: scratch
                            .path()
                            .join("projected.md")
                            .display()
                            .to_string(),
                        pdf: scratch.path().join("note.pdf"),
                        audio: flow.scratch_audio.clone(),
                        title: plan.title.clone(),
                    },
                )
            );
        }
        print_snapshot_plan(
            config,
            &plan,
            &snapshot,
            &asset_preparation,
            options,
            &Styler::detect(),
        );
        if let Some(line) =
            super::diagrams::dry_run_line(&snapshot.projected_markdown)
        {
            println!("{line}");
        }
        emit_snapshot_warnings(&snapshot, &snapshot.projected_markdown, &[]);
        println!("writes: none");
        return Ok(());
    }

    if plan.target.parent().is_none() {
        return Err(CommandError::new(format!(
            "target has no parent directory: {}",
            plan.target.display()
        )));
    }
    stage_snapshot_assets(&asset_preparation, scratch.path())?;
    let mut filters = write_render_filters(scratch.path())?;
    let diagram_pass = super::diagrams::prepare(
        pandoc_command().as_deref(),
        super::diagrams::DiagramSource::Text(&snapshot.projected_markdown),
        super::diagrams::DiagramRoute {
            reader: super::diagrams::DiagramReader::Snapshot,
            text_width_pt:
                note_snapshot::profile::SNAPSHOT_DIAGRAM_TEXT_WIDTH_PT,
            text_height_pt:
                note_snapshot::profile::SNAPSHOT_DIAGRAM_TEXT_HEIGHT_PT,
        },
        scratch.path(),
    );
    filters.diagrams = diagram_pass.filter.clone();
    let outcome = note_snapshot::profile::render_note_pdf(
        &snapshot.projected_markdown,
        scratch.path(),
        &filters,
        &note_snapshot::profile::NoteMeta {
            identity_title: snapshot.identity_title.clone(),
            source_path: snapshot.note.identity.vault_relative_path.clone(),
            kind: snapshot.source_kind.to_string(),
            listen_uri: audio_uri,
        },
    )?;
    let marker = stamp_marker_for_outcome(&plan, options, &outcome.report)?;
    let info = PdfInfo {
        title: Some(snapshot.identity_title.clone()),
        author: plan.author.clone(),
    };
    let (page_count, link_outcome) = match (listen, listen_flow.as_ref()) {
        (Some(command), Some(flow)) => {
            let projected_path = scratch.path().join("projected.md");
            let projected_display = projected_path.display().to_string();
            let pages = stamp_listen_and_install_pdf(
                scratch,
                &plan.target_plan(),
                command,
                flow,
                &projected_display,
                &outcome.pdf,
                &plan.title,
                &plan.source,
                &outcome.pdf,
                &marker,
                &info,
                options.force.is_some(),
            )?;
            (pages, outcome.report)
        }
        _ => {
            let (stamped, pages) = pdf_target_mod::stamp_pdf_to_scratch(
                &outcome.pdf,
                scratch,
                &marker,
                &info,
            )?;
            super::refuse_target_collisions(
                &plan.target,
                &plan.sidecar,
                &plan.workflow,
                options.force.is_some(),
            )?;
            pdf_target_mod::install_stamped_pdf(
                &stamped,
                &plan.target,
                plan.audio.as_ref(),
                pages,
            )?;
            (pages, outcome.report)
        }
    };

    let styler = Styler::detect();
    println!("{} created note snapshot", styler.success_prefix(false));
    println!(
        "source: {} ({} · sha256 {})",
        plan.source,
        snapshot.source_kind,
        &snapshot.source_digest[..8]
    );
    print_reread_or_renamed(&plan.target_plan(), options);
    println!("pdf: {}", plan.target.display());
    if let Some(audio) = &plan.audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("title: {}", plan.title);
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    if let Some(id) = &plan.id {
        println!("id: {id}");
    }
    println!("pages: {page_count}");
    print_snapshot_shown_omitted(&snapshot.summary);
    emit_snapshot_warnings(
        &snapshot,
        &snapshot.projected_markdown,
        &outcome.warnings,
    );
    super::return_links::emit_outcome(&link_outcome, &styler);
    diagram_pass.emit(&styler);
    finish_create_intake(
        config,
        &plan.target_plan(),
        options.keep_intake_reason.as_deref(),
    )?;
    print_earlier_snapshots(config, &snapshot, &plan.stem, false);
    Ok(())
}

fn stage_snapshot_assets(
    preparation: &note_snapshot::assets::AssetPreparation,
    scratch: &Path,
) -> Result<()> {
    for asset in &preparation.assets {
        let destination = scratch.join(&asset.relative_path);
        let parent = destination.parent().ok_or_else(|| {
            CommandError::new(format!(
                "snapshot asset has no destination directory: {}",
                destination.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create snapshot asset directory {}: {error}",
                parent.display()
            ))
        })?;
        fs::write(&destination, &asset.bytes).map_err(|error| {
            CommandError::new(format!(
                "write snapshot asset {}: {error}",
                destination.display()
            ))
        })?;
    }
    Ok(())
}

fn write_render_filters(scratch: &Path) -> Result<RenderFilters> {
    let code_break = scratch.join("filter.lua");
    fs::write(&code_break, PANDOC_CODE_BREAK_FILTER).map_err(|error| {
        CommandError::new(format!(
            "write pandoc filter {}: {error}",
            code_break.display()
        ))
    })?;
    let return_links = scratch.join("return-links.lua");
    fs::write(&return_links, super::return_links::FILTER).map_err(|error| {
        CommandError::new(format!(
            "write pandoc filter {}: {error}",
            return_links.display()
        ))
    })?;
    Ok(RenderFilters {
        code_break,
        return_links,
        report: scratch.join("return-links.json"),
        diagrams: None,
    })
}

fn print_snapshot_plan(
    config: &Config,
    plan: &CreatePlan,
    snapshot: &note_snapshot::SnapshotPlan,
    assets: &note_snapshot::assets::AssetPreparation,
    options: &CreateOptions,
    styler: &Styler,
) {
    println!(
        "dry-run: {} would create note snapshot",
        styler.success_prefix(true)
    );
    println!(
        "source: {} ({} · sha256 {})",
        plan.source,
        snapshot.source_kind,
        &snapshot.source_digest[..8]
    );
    print_reread_or_renamed(&plan.target_plan(), options);
    println!("pdf: {}", plan.target.display());
    println!("sidecar_guard: {}", plan.sidecar.display());
    if let TargetWorkflow::Intake {
        library_destination,
    } = &plan.workflow
    {
        println!("library_destination: {}", library_destination.display());
    }
    if let Some(audio) = &plan.audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("title: {}", plan.title);
    println!("status: {}", options.status);
    let parent_line = snapshot
        .parent
        .as_ref()
        .map(|parent| parent.dry_run_line())
        .unwrap_or_else(|| format!("parent: {}", options.parent));
    if let Some(reason) = &snapshot.parent_default_reason {
        println!("{parent_line} · default: {reason}");
    } else {
        println!("{parent_line}");
    }
    if let Some(id) = &plan.id {
        println!("id: {id}");
    }
    println!("marker:");
    print!("{}", plan.marker);
    print_snapshot_projection_summary(snapshot, assets);
    print_earlier_snapshots(config, snapshot, &plan.stem, true);
    print_create_intake_preview(
        config,
        &plan.target_plan(),
        options.keep_intake_reason.as_deref(),
    );
}

/// One earlier Highlights snapshot of the same source note.
struct EarlierSnapshot {
    stem: String,
    status: String,
    highlights: u64,
    created: Option<String>,
}

impl EarlierSnapshot {
    fn listing(&self) -> String {
        format!(
            "{} ({}, {} highlights)",
            self.stem, self.status, self.highlights
        )
    }

    fn is_unread_ready(&self) -> bool {
        self.status.eq_ignore_ascii_case("ready") && self.highlights == 0
    }

    fn hint_line(&self) -> String {
        format!(
            "hint: earlier snapshot {} is still unread (0 highlights)",
            self.stem
        )
    }
}

fn print_earlier_snapshots(
    config: &Config,
    snapshot: &note_snapshot::SnapshotPlan,
    exclude_stem: &str,
    list: bool,
) {
    let earlier = collect_earlier_snapshots(
        &config.ref_dir,
        &snapshot.note.identity.route,
        exclude_stem,
    );
    if list && !earlier.is_empty() {
        println!(
            "earlier: {}",
            earlier
                .iter()
                .map(EarlierSnapshot::listing)
                .collect::<Vec<_>>()
                .join(" · ")
        );
    }
    for item in earlier {
        if item.is_unread_ready() {
            println!("{}", item.hint_line());
        }
    }
}

fn collect_earlier_snapshots(
    ref_dir: &Path,
    source_route: &str,
    exclude_stem: &str,
) -> Vec<EarlierSnapshot> {
    let mut refs = Vec::new();
    collect_markdown_files(ref_dir, &mut refs);
    let wanted = source_route.to_lowercase();
    let mut earlier = Vec::new();
    for path in refs {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem == exclude_stem {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(frontmatter) =
            crate::native::projects::parse_frontmatter(&contents)
        else {
            continue;
        };
        let Some(source_note) = crate::native::projects::frontmatter_value(
            &frontmatter,
            "source_note",
        ) else {
            continue;
        };
        if normalize_source_note(source_note) != wanted {
            continue;
        }
        let status = ref_frontmatter_status(&frontmatter);
        if status.is_empty() {
            continue;
        }
        let highlights = crate::native::projects::frontmatter_value(
            &frontmatter,
            "highlights_count",
        )
        .map(crate::native::projects::trim_yaml_scalar)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
        let created =
            crate::native::projects::frontmatter_value(&frontmatter, "created")
                .map(crate::native::projects::trim_yaml_scalar)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
        earlier.push(EarlierSnapshot {
            stem: stem.to_string(),
            status,
            highlights,
            created,
        });
    }
    earlier.sort_by(|left, right| {
        match (left.created.as_deref(), right.created.as_deref()) {
            (Some(left_created), Some(right_created)) => right_created
                .cmp(left_created)
                .then_with(|| left.stem.cmp(&right.stem)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.stem.cmp(&right.stem),
        }
    });
    earlier
}

fn ref_frontmatter_status(
    frontmatter: &crate::native::projects::Frontmatter<'_>,
) -> String {
    crate::native::projects::frontmatter_value(frontmatter, "status")
        .map(crate::native::projects::trim_yaml_scalar)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            crate::native::projects::frontmatter_value(
                frontmatter,
                "legacy_status",
            )
            .map(crate::native::projects::trim_yaml_scalar)
            .filter(|value| !value.is_empty())
        })
        .unwrap_or("")
        .to_string()
}

fn collect_markdown_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn normalize_source_note(value: &str) -> String {
    let value = crate::native::projects::trim_yaml_scalar(value)
        .trim()
        .trim_start_matches("[[")
        .trim_end_matches("]]");
    value
        .split_once('|')
        .map(|(target, _)| target)
        .unwrap_or(value)
        .to_lowercase()
}

fn print_snapshot_projection_summary(
    snapshot: &note_snapshot::SnapshotPlan,
    assets: &note_snapshot::assets::AssetPreparation,
) {
    let sections = snapshot
        .projection
        .view
        .sections
        .iter()
        .map(|section| {
            section
                .heading
                .content
                .iter()
                .filter_map(|inline| match inline {
                    note_snapshot::Inline::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    println!(
        "sections: {}",
        if sections.is_empty() {
            "none".to_string()
        } else {
            sections.join(" · ")
        }
    );
    println!("tasks: {}", task_summary(&snapshot.summary));
    println!(
        "omitted: {} closed · {} self-snapshot · {} sections",
        snapshot.summary.omitted_closed_tasks,
        snapshot.summary.omitted_self_snapshot_tasks,
        snapshot.summary.omitted_sections
    );
    println!(
        "logs: {} schedule logs · {} work logs · {} rotten logs",
        snapshot.summary.schedule_logs,
        snapshot.summary.work_logs,
        snapshot.summary.rotten_logs
    );
    println!(
        "embeds: {} images ({} missing) · {} transclusions · {} guarded omissions",
        snapshot.summary.assets,
        snapshot.summary.missing_images,
        snapshot.summary.transclusions,
        snapshot.summary.guarded_omissions
    );
    println!(
        "image_processing: {} total · {} downscaled · {} missing",
        snapshot.summary.assets + snapshot.summary.missing_images
            - assets.missing.len() as u64,
        assets.downscaled.len(),
        snapshot.summary.missing_images
    );
    for path in &assets.downscaled {
        println!("  downscaled: {path}");
    }
    for path in &assets.missing {
        println!("  missing: {path}");
    }
    println!(
        "links: {} resolved · {} unresolved",
        snapshot.summary.links.resolved, snapshot.summary.links.unresolved
    );
    match glyph_coverage(&snapshot.projected_markdown) {
        Ok(uncovered) if uncovered.is_empty() => {
            println!("glyphs: all covered")
        }
        Ok(uncovered) => println!(
            "glyphs: uncovered {}",
            uncovered
                .iter()
                .map(|ch| format!("U+{:04X}", u32::from(*ch)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Err(error) => println!("glyphs: unavailable ({error})"),
    }
}

fn print_snapshot_shown_omitted(summary: &note_snapshot::SnapshotSummary) {
    println!("shown: {}", task_summary(summary));
    println!(
        "omitted: {} closed · {} self-snapshot · {} sections",
        summary.omitted_closed_tasks,
        summary.omitted_self_snapshot_tasks,
        summary.omitted_sections
    );
}

fn task_summary(summary: &note_snapshot::SnapshotSummary) -> String {
    use note_snapshot::SnapshotTaskStatus as Status;
    let statuses = [
        (Status::Open, "open"),
        (Status::Next, "next"),
        (Status::Wip, "in progress"),
        (Status::Blocked, "blocked"),
        (Status::Scheduled, "scheduled"),
        (Status::Done, "done"),
        (Status::Canceled, "canceled"),
    ];
    let shown = statuses
        .iter()
        .filter_map(|(status, label)| {
            let count = summary.task_counts.get(status).copied().unwrap_or(0);
            (count > 0).then(|| format!("{count} {label}"))
        })
        .collect::<Vec<_>>();
    let total = summary.task_counts.values().sum::<u64>();
    format!(
        "{total} shown ({})",
        if shown.is_empty() {
            "none".to_string()
        } else {
            shown.join(" · ")
        }
    )
}

fn glyph_coverage(markdown: &str) -> std::io::Result<Vec<char>> {
    note_snapshot::fonts::GlyphCoverage::new()
        .map(|coverage| coverage.uncovered_everywhere(markdown))
}

fn emit_snapshot_warnings(
    snapshot: &note_snapshot::SnapshotPlan,
    projected_markdown: &str,
    render_warnings: &[note_snapshot::profile::NoteWarning],
) {
    for warning in &snapshot.warnings {
        println!("warning: {}", warning.message);
    }
    match glyph_coverage(projected_markdown) {
        Ok(uncovered) if !uncovered.is_empty() => println!(
            "warning: uncovered glyphs: {}",
            uncovered
                .iter()
                .map(|ch| format!("U+{:04X} {ch}", u32::from(*ch)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Err(error) => println!("warning: glyph coverage unavailable: {error}"),
        _ => {}
    }
    for warning in render_warnings {
        println!("warning: {}", warning.message());
    }
}

fn create_markdown_route(
    config: &Config,
    source: &Path,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
    source_display: &str,
) -> Result<()> {
    let plan = plan_markdown(config, source, options, source_display)?;
    create_markdown_plan(config, plan, scratch, options, listen)
}

fn create_markdown_plan(
    config: &Config,
    mut plan: CreatePlan,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let styler = Styler::detect();
    // With --listen the staged render doubles as the narration source:
    // `{target}` and `{pdf}` are `<scratch>/<stem>.pdf`, and the listen
    // card's Play URI targets `<stem>.mp3`.
    let listen_setup = match listen {
        Some(command) => {
            let stem = plan
                .target
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| {
                    CommandError::new(format!(
                        "target has no file stem: {}",
                        plan.target.display()
                    ))
                })?
                .to_string();
            let flow = super::listen::plan_listen_flow(
                &plan.target_plan(),
                scratch,
                &stem,
            )?;
            plan.audio = Some(listen_audio_plan(&flow));
            Some((command, flow, scratch.path().join(format!("{stem}.pdf"))))
        }
        None => None,
    };
    let audio_uri = play_uri(config, &plan)?;
    if options.dry_run {
        if let Some((command, flow, render_path)) = &listen_setup {
            println!(
                "{}",
                super::listen::would_run_line(
                    command,
                    &super::listen::ListenValues {
                        target: render_path.display().to_string(),
                        pdf: render_path.clone(),
                        audio: flow.scratch_audio.clone(),
                        title: plan.title.clone(),
                    },
                )
            );
        }
        print_plan(config, &plan, options, &styler);
        if let Some(markdown) = plan.source_markdown.as_deref()
            && let Some(line) = super::diagrams::dry_run_line(markdown)
        {
            println!("{line}");
        }
        println!("writes: none");
        return Ok(());
    }
    let pandoc = pandoc_command().ok_or_else(|| {
        CommandError::new(format!(
            "pandoc command not found; install pandoc or set {ENV_PANDOC_COMMAND}"
        ))
    })?;
    // Do not create the target parent before the listen: a failed listen
    // must leave `xlib/` untouched. `atomic_copy` creates parents at
    // install time.
    if plan.target.parent().is_none() {
        return Err(CommandError::new(format!(
            "target has no parent directory: {}",
            plan.target.display()
        )));
    }
    let render_path = match &listen_setup {
        Some((.., path)) => path.clone(),
        None => scratch.path().join("render.pdf"),
    };
    let mut filters = write_render_filters(scratch.path())?;
    let diagram_pass = super::diagrams::prepare(
        Some(pandoc.as_os_str()),
        super::diagrams::DiagramSource::Path(Path::new(&plan.source)),
        super::diagrams::DiagramRoute {
            reader: super::diagrams::DiagramReader::Inferred,
            text_width_pt: GENERIC_DIAGRAM_TEXT_WIDTH_PT,
            text_height_pt: GENERIC_DIAGRAM_TEXT_HEIGHT_PT,
        },
        scratch.path(),
    );
    filters.diagrams = diagram_pass.filter.clone();
    let (_audio_created, page_count, link_outcome) = match &listen_setup {
        Some((command, flow, _)) => {
            let link_outcome = render_temp_pdf(
                &pandoc,
                &plan,
                &render_path,
                &filters,
                audio_uri.as_deref(),
            )?;
            let values = super::listen::ListenValues {
                target: render_path.display().to_string(),
                pdf: render_path.clone(),
                audio: flow.scratch_audio.clone(),
                title: plan.title.clone(),
            };
            super::listen::run_listen(command, &values)
                .map_err(|error| error.into_command_error())?;
            // The vault may have changed during a long listen.
            if let Err(error) = super::refuse_target_collisions(
                &plan.target,
                &plan.sidecar,
                &plan.workflow,
                options.force.is_some(),
            ) {
                return Err(super::listen::post_listen_error(
                    scratch,
                    &flow.scratch_audio,
                    error.to_string(),
                    bind_create_hint(&plan.source, &flow.scratch_audio),
                ));
            }
            let created = super::listen::install_listen_audio(
                flow,
                options.force.is_some(),
            )
            .map_err(|error| {
                super::listen::post_listen_error(
                    scratch,
                    &flow.scratch_audio,
                    error.to_string(),
                    bind_create_hint(&plan.source, &flow.scratch_audio),
                )
            })?;
            let marker =
                stamp_marker_for_outcome(&plan, options, &link_outcome)?;
            let page_count = match stamp_and_install(
                &render_path,
                &plan.target,
                &marker,
                &PdfInfo::default(),
            ) {
                Ok(page_count) => page_count,
                Err(error) => {
                    companion_mod::cleanup_audio_on_failure(created.as_ref());
                    return Err(super::listen::post_listen_error(
                        scratch,
                        &flow.scratch_audio,
                        error.to_string(),
                        bind_create_hint(&plan.source, &flow.scratch_audio),
                    ));
                }
            };
            (created, page_count, link_outcome)
        }
        None => {
            let render_result = render_temp_pdf(
                &pandoc,
                &plan,
                &render_path,
                &filters,
                audio_uri.as_deref(),
            );
            let audio_created = if render_result.is_ok() {
                match companion_mod::copy_audio_for_install(plan.audio.as_ref())
                {
                    Ok(created) => created,
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            let install_result = render_result.and_then(|link_outcome| {
                stamp_marker_for_outcome(&plan, options, &link_outcome)
                    .and_then(|marker| {
                        stamp_and_install(
                            &render_path,
                            &plan.target,
                            &marker,
                            &PdfInfo::default(),
                        )
                        .map(|page_count| (link_outcome, page_count))
                    })
            });
            let (link_outcome, page_count) = match install_result {
                Ok((link_outcome, page_count)) => (link_outcome, page_count),
                Err(error) => {
                    companion_mod::cleanup_audio_on_failure(
                        audio_created.as_ref(),
                    );
                    return Err(error);
                }
            };
            (audio_created, page_count, link_outcome)
        }
    };
    println!(
        "{} created Highlights-ready PDF",
        styler.success_prefix(false)
    );
    println!("source: {}", plan.source);
    print_reread_or_renamed(&plan.target_plan(), options);
    println!("pdf: {}", plan.target.display());
    if let Some(audio) = &plan.audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    if let Some(uri) = audio_uri {
        println!("audio_link: {uri}");
    }
    println!("title: {}", plan.title);
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    if let Some(id) = &plan.id {
        println!("id: {id}");
    }
    println!("pages: {page_count}");
    super::return_links::emit_outcome(&link_outcome, &styler);
    diagram_pass.emit(&styler);
    finish_create_intake(
        config,
        &plan.target_plan(),
        options.keep_intake_reason.as_deref(),
    )?;
    Ok(())
}

fn create_local_pdf_route(
    config: &Config,
    source: &Path,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
    source_display: &str,
) -> Result<()> {
    let canonical = fs::canonicalize(source).map_err(|error| {
        CommandError::new(format!(
            "resolve PDF file {}: {error}",
            source.display()
        ))
    })?;
    // Attach when the target itself is an existing capture. A reread
    // narrates a new reading instead, and a library PDF as TARGET never
    // produces a preflight: under reread force the library-PDF refusal
    // (with its source_url hint) wins even with `-L`, while `-L`
    // without force still attaches here.
    if options.reread.is_none()
        && options.force != Some(super::force::ForceMode::Reread)
        && let Some(command) = listen
        && let Some(attach) =
            attach_mod::attach_for_local_pdf(config, &canonical)?
    {
        let source_line = format!("source: {} (PDF)", canonical.display());
        let target_value = canonical.display().to_string();
        return attach_mod::run_attach(
            config,
            command,
            &attach,
            &source_line,
            &target_value,
            options.dry_run,
            options.keep_intake_reason.as_deref(),
            scratch,
        );
    }
    pdf_target_mod::check_local_pdf_identity(config, source)?;
    let pdf_plan = pdf_target_mod::plan_local_pdf(
        &canonical,
        options.name.as_deref(),
        options.title.as_deref(),
        options.author.as_deref(),
        options.published.as_deref(),
        Some(&pdf_progress),
    )?;
    let ref_type =
        default_ref_type_for_kind("pdf", options.ref_type.as_deref())?;
    let identity = DefaultTargetIdentity::Title(pdf_plan.title.clone());
    // A reread plans its allocated `<root>__vN` stem with no suffix
    // fallback and always stamps `id`; `-o` on a captured reread was
    // refused at preflight.
    let target_plan = match &options.reread {
        Some(preflight) => super::recapture::plan_reread_target(
            config,
            &preflight.plan.stem,
            &ref_type,
            &identity,
        )?,
        None => plan_target_for(
            config,
            &pdf_plan.stem,
            &ref_type,
            options.output.as_ref(),
            options.force,
            options.include_id,
            &identity,
            source_display,
        )?,
    };
    let final_stem = target_plan.stem.clone();
    // An out-of-vault marked PDF is already captured: refuse before any
    // write (in-vault cases were handled by attach/identity above).
    {
        let library_hint: Option<&Path> = match &target_plan.workflow {
            TargetWorkflow::Intake {
                library_destination,
            } => Some(library_destination),
            _ => None,
        };
        pdf_target_mod::refuse_marked_pdf(&canonical, library_hint)?;
    }
    let audio =
        plan_pdf_audio(config, &target_plan, options, Some(&canonical))?;
    // With --listen `{target}` is the source path and `{pdf}` is the
    // unstaged scratch copy; the library check runs now, the intake
    // byte check after the run.
    let (listen_flow, audio) = match listen {
        Some(command) => {
            let flow = super::listen::plan_listen_flow(
                &target_plan,
                scratch,
                &final_stem,
            )?;
            let display = listen_audio_plan(&flow);
            (Some((command, flow)), Some(display))
        }
        None => (None, audio),
    };
    // Rereads always stamp `id` equal to the final stem so the birth
    // lineage can key on it.
    let id = if options.include_id || options.reread.is_some() {
        Some(final_stem.clone())
    } else {
        None
    };
    let marker = pdf_target_mod::compose_pdf_marker(
        &options.status,
        &options.parent,
        &pdf_plan,
        id.as_deref(),
    )?;
    let info = PdfInfo {
        title: Some(pdf_plan.title.clone()),
        author: pdf_plan.author.clone(),
    };
    let styler = Styler::detect();
    if options.dry_run {
        if let Some((command, flow)) = &listen_flow {
            let unstamped = scratch.path().join(format!("{final_stem}.pdf"));
            println!(
                "{}",
                super::listen::would_run_line(
                    command,
                    &super::listen::ListenValues {
                        target: canonical.display().to_string(),
                        pdf: unstamped,
                        audio: flow.scratch_audio.clone(),
                        title: pdf_plan.title.clone(),
                    },
                )
            );
        }
        print_pdf_dry_run(
            config,
            &styler,
            &canonical.display().to_string(),
            "PDF",
            &target_plan,
            &pdf_plan,
            id.as_deref(),
            &marker,
            audio.as_ref(),
            options,
            &[],
        );
        return Ok(());
    }
    let installed = match &listen_flow {
        Some((command, flow)) => {
            let unstamped = scratch.path().join(format!("{final_stem}.pdf"));
            fs::copy(&canonical, &unstamped).map_err(|error| {
                CommandError::new(format!(
                    "stage PDF {}: {error}",
                    canonical.display()
                ))
            })?;
            stamp_listen_and_install_pdf(
                scratch,
                &target_plan,
                command,
                flow,
                &canonical.display().to_string(),
                &unstamped,
                &pdf_plan.title,
                &canonical.display().to_string(),
                &unstamped,
                &marker,
                &info,
                options.force.is_some(),
            )?
        }
        None => {
            let (stamped, page_count) = pdf_target_mod::stamp_pdf_to_scratch(
                &canonical, scratch, &marker, &info,
            )?;
            pdf_target_mod::install_stamped_pdf(
                &stamped,
                &target_plan.target,
                audio.as_ref(),
                page_count,
            )?
        }
    };
    let bytes = fs::metadata(&target_plan.target)
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "{} created Highlights-ready PDF",
        styler.success_prefix(false)
    );
    println!("source: {} (PDF)", canonical.display());
    print_reread_or_renamed(&target_plan, options);
    println!("pdf: {}", target_plan.target.display());
    if let Some(audio) = &audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("title: {}", pdf_plan.title);
    if let Some(author) = &pdf_plan.author {
        println!("author: {author}");
    }
    if let Some(published) = &pdf_plan.published {
        println!("published: {published}");
    }
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    if let Some(id) = &id {
        println!("id: {id}");
    }
    println!(
        "pages: {installed} · size: {}",
        super::clip::format_bytes(bytes)
    );
    finish_create_intake(
        config,
        &target_plan,
        options.keep_intake_reason.as_deref(),
    )?;
    Ok(())
}

fn create_pdf_url_route(
    config: &Config,
    url: super::clip_url::WebUrl,
    downloaded: PathBuf,
    final_url: String,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    recorded: &[sources_mod::RecordedSource],
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let captured = super::current_local_date();
    let pdf_plan = pdf_target_mod::plan_pdf_url(
        &url,
        &downloaded,
        options.name.as_deref(),
        options.title.as_deref(),
        options.author.as_deref(),
        options.published.as_deref(),
        &captured,
        Some(&pdf_progress),
    )?;
    let ref_type =
        default_ref_type_for_kind("pdf", options.ref_type.as_deref())?;
    let identity = DefaultTargetIdentity::Url {
        keys: vec![url.dedupe_key.clone()],
        recorded,
    };
    // A reread plans its allocated `<root>__vN` stem with no suffix
    // fallback; ordinary captures plan from the fetched slug.
    let target_plan = match &options.reread {
        Some(preflight) => super::recapture::plan_reread_target(
            config,
            &preflight.plan.stem,
            &ref_type,
            &identity,
        )?,
        None => plan_target_for(
            config,
            &pdf_plan.stem,
            &ref_type,
            options.output.as_ref(),
            options.force,
            true,
            &identity,
            &url.cleaned,
        )?,
    };
    let final_stem = target_plan.stem.clone();
    // The stem is only known after the fetch, so a race that captured
    // the URL mid-fetch attaches here instead of refusing. A reread
    // narrates its new reading instead, and re-resolves its keys against
    // the live index (a redirected fetch that now claims another family
    // refuses).
    if options.reread.is_none()
        && let Some(command) = listen
        && let Some(hit) = sources_mod::find_refusing_hit(
            recorded,
            &url.dedupe_key,
            Some(&target_plan.target),
            options.force.is_some(),
        )
    {
        let hit = super::recapture::newest_reading_hit(config, recorded, &hit);
        let attach = attach_mod::attach_for_dedupe_hit(config, &hit)?;
        return attach_mod::run_attach(
            config,
            command,
            &attach,
            &format!("source: {} (PDF)", url.cleaned),
            &url.cleaned,
            options.dry_run,
            options.keep_intake_reason.as_deref(),
            scratch,
        );
    }
    if let Some(preflight) = &options.reread {
        let fresh_recorded = sources_mod::collect_recorded_source_urls(config)
            .map_err(|error| CommandError::new(error.message.clone()))?;
        let rows = super::recapture::index_rows(config);
        let mut wanted = std::collections::BTreeSet::new();
        wanted.insert(url.dedupe_key.clone());
        let fresh = super::recapture::preflight_for_url(
            config,
            &rows,
            &fresh_recorded,
            &wanted,
            options.name.as_deref(),
            None,
            options.force,
            &url.cleaned,
        )?;
        if fresh.is_some_and(|next| next.plan.stem != target_plan.stem) {
            return Err(CommandError::new(format!(
                "reread {} is already filed; nothing written",
                target_plan.stem,
            )));
        }
        // A redirect that now claims another family refuses before
        // writing, even when the pre-fetch key still selects our family.
        super::recapture::refuse_redirected_reread(
            config,
            &url.dedupe_key,
            &final_url,
            preflight,
        )?;
        let _ = preflight;
    } else {
        check_dedupe_with_listen_hint(
            config,
            recorded,
            &url.dedupe_key,
            Some(&target_plan.target),
            options.force.is_some(),
        )?;
    }
    // A downloaded PDF that already carries a marker is already captured.
    {
        let library_hint: Option<&Path> = match &target_plan.workflow {
            TargetWorkflow::Intake {
                library_destination,
            } => Some(library_destination),
            _ => None,
        };
        pdf_target_mod::refuse_marked_pdf(&downloaded, library_hint)?;
    }
    let audio = plan_pdf_audio(config, &target_plan, options, None)?;
    // With --listen `{target}` is the cleaned URL and `{pdf}` is the
    // downloaded scratch copy.
    let (listen_flow, audio) = match listen {
        Some(command) => {
            let flow = super::listen::plan_listen_flow(
                &target_plan,
                scratch,
                &final_stem,
            )?;
            let display = listen_audio_plan(&flow);
            (Some((command, flow)), Some(display))
        }
        None => (None, audio),
    };
    let id = Some(final_stem.clone());
    let marker = pdf_target_mod::compose_pdf_marker(
        &options.status,
        &options.parent,
        &pdf_plan,
        id.as_deref(),
    )?;
    let info = PdfInfo {
        title: Some(pdf_plan.title.clone()),
        author: pdf_plan.author.clone(),
    };
    let styler = Styler::detect();
    if options.dry_run {
        if let Some((command, flow)) = &listen_flow {
            println!(
                "{}",
                super::listen::would_run_line(
                    command,
                    &super::listen::ListenValues {
                        target: url.cleaned.clone(),
                        pdf: downloaded.clone(),
                        audio: flow.scratch_audio.clone(),
                        title: pdf_plan.title.clone(),
                    },
                )
            );
        }
        print_pdf_dry_run(
            config,
            &styler,
            &url.cleaned,
            "PDF",
            &target_plan,
            &pdf_plan,
            id.as_deref(),
            &marker,
            audio.as_ref(),
            options,
            &sources_mod::legacy_hits(recorded, &url.dedupe_key),
        );
        return Ok(());
    }
    let installed = match &listen_flow {
        Some((command, flow)) => stamp_listen_and_install_pdf(
            scratch,
            &target_plan,
            command,
            flow,
            &url.cleaned,
            &downloaded,
            &pdf_plan.title,
            &url.cleaned,
            &downloaded,
            &marker,
            &info,
            options.force.is_some(),
        )?,
        None => {
            let (stamped, page_count) = pdf_target_mod::stamp_pdf_to_scratch(
                &downloaded,
                scratch,
                &marker,
                &info,
            )?;
            pdf_target_mod::install_stamped_pdf(
                &stamped,
                &target_plan.target,
                audio.as_ref(),
                page_count,
            )?
        }
    };
    let bytes = fs::metadata(&target_plan.target)
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "{} created Highlights-ready PDF",
        styler.success_prefix(false)
    );
    println!("source: {} (PDF)", url.cleaned);
    print_reread_or_renamed(&target_plan, options);
    println!("pdf: {}", target_plan.target.display());
    if let Some(audio) = &audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("title: {}", pdf_plan.title);
    if let Some(author) = &pdf_plan.author {
        println!("author: {author}");
    }
    if let Some(published) = &pdf_plan.published {
        println!("published: {published}");
    }
    println!("captured: {captured}");
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    println!("id: {final_stem}");
    println!(
        "pages: {installed} · size: {}",
        super::clip::format_bytes(bytes)
    );
    finish_create_intake(
        config,
        &target_plan,
        options.keep_intake_reason.as_deref(),
    )?;
    Ok(())
}

fn create_arxiv_route(
    config: &Config,
    paper: super::arxiv::ArxivPaper,
    url: super::clip_url::WebUrl,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    recorded: &[sources_mod::RecordedSource],
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let captured = super::current_local_date();
    let (metadata, warning) = super::arxiv::fetch_metadata(
        &paper,
        scratch.path(),
        Some(&fetch_progress),
    );
    if let Some(warning) = warning {
        eprintln!("warning: {warning}");
    }
    let dest = scratch.path().join("arxiv.pdf");
    let pdf_url = paper.pdf_url();
    let fetch =
        super::fetch::fetch_url(&pdf_url, &dest, 300, Some(&fetch_progress))
            .map_err(|error| match error.hint() {
                Some(hint) => CommandError::new(format!(
                    "fetch arXiv PDF {pdf_url}: {}\nhint: {hint}",
                    error.message()
                )),
                None => CommandError::new(format!(
                    "fetch arXiv PDF {pdf_url}: {}",
                    error.message()
                )),
            })?;
    if !(200..300).contains(&fetch.status) {
        return Err(CommandError::new(format!(
            "server returned HTTP {} for {pdf_url}",
            fetch.status
        )));
    }
    if !target_mod::file_starts_with_pdf(&dest) {
        return Err(CommandError::new(format!(
            "server said PDF but sent something else: {pdf_url}"
        )));
    }
    let pdf_plan = pdf_target_mod::plan_arxiv(
        &paper,
        options.name.as_deref(),
        options.title.as_deref(),
        options.author.as_deref(),
        options.published.as_deref(),
        metadata.as_ref(),
        &dest,
        &captured,
        Some(&pdf_progress),
    )?;
    let ref_type =
        default_ref_type_for_kind("pdf", options.ref_type.as_deref())?;
    let key = paper.dedupe_key();
    let identity = DefaultTargetIdentity::Url {
        keys: vec![key.clone(), url.dedupe_key.clone()],
        recorded,
    };
    // A reread plans its allocated `<root>__vN` stem with no suffix
    // fallback; ordinary captures plan from the fetched slug.
    let target_plan = match &options.reread {
        Some(preflight) => super::recapture::plan_reread_target(
            config,
            &preflight.plan.stem,
            &ref_type,
            &identity,
        )?,
        None => plan_target_for(
            config,
            &pdf_plan.stem,
            &ref_type,
            options.output.as_ref(),
            options.force,
            true,
            &identity,
            &url.cleaned,
        )?,
    };
    let final_stem = target_plan.stem.clone();
    // Post-capture dedupe with the final target (stem known only now).
    // Also check the user's URL spelling key (same arXiv key).
    // With --listen a hit attaches instead of refusing. A reread
    // narrates its new reading instead, and re-resolves both keys
    // against the live index.
    if options.reread.is_none()
        && let Some(command) = listen
    {
        let hit = sources_mod::find_refusing_hit(
            recorded,
            &key,
            Some(&target_plan.target),
            options.force.is_some(),
        )
        .or_else(|| {
            sources_mod::find_refusing_hit(
                recorded,
                &url.dedupe_key,
                Some(&target_plan.target),
                options.force.is_some(),
            )
        });
        if let Some(hit) = hit {
            let hit =
                super::recapture::newest_reading_hit(config, recorded, &hit);
            let attach = attach_mod::attach_for_dedupe_hit(config, &hit)?;
            return attach_mod::run_attach(
                config,
                command,
                &attach,
                &format!(
                    "source: {} (arXiv {})",
                    paper.abs_url(),
                    paper.full_id()
                ),
                &url.cleaned,
                options.dry_run,
                options.keep_intake_reason.as_deref(),
                scratch,
            );
        }
    }
    if options.reread.is_none() {
        check_dedupe_with_listen_hint(
            config,
            recorded,
            &key,
            Some(&target_plan.target),
            options.force.is_some(),
        )?;
        check_dedupe_with_listen_hint(
            config,
            recorded,
            &url.dedupe_key,
            Some(&target_plan.target),
            options.force.is_some(),
        )?;
    } else if let Some(preflight) = &options.reread {
        let fresh_recorded = sources_mod::collect_recorded_source_urls(config)
            .map_err(|error| CommandError::new(error.message.clone()))?;
        let rows = super::recapture::index_rows(config);
        let mut wanted = std::collections::BTreeSet::new();
        wanted.insert(key.clone());
        wanted.insert(url.dedupe_key.clone());
        let fresh = super::recapture::preflight_for_url(
            config,
            &rows,
            &fresh_recorded,
            &wanted,
            options.name.as_deref(),
            None,
            options.force,
            &url.cleaned,
        )?;
        if fresh.is_some_and(|next| next.plan.stem != target_plan.stem) {
            return Err(CommandError::new(format!(
                "reread {} is already filed; nothing written",
                target_plan.stem,
            )));
        }
        // A redirect that now claims another family refuses before
        // writing, even when the pre-fetch key still selects our family.
        super::recapture::refuse_redirected_reread(
            config,
            &key,
            &fetch.final_url,
            preflight,
        )?;
    }
    // A downloaded arXiv PDF that already carries a marker is already captured.
    {
        let library_hint: Option<&Path> = match &target_plan.workflow {
            TargetWorkflow::Intake {
                library_destination,
            } => Some(library_destination),
            _ => None,
        };
        pdf_target_mod::refuse_marked_pdf(&dest, library_hint)?;
    }
    let audio = plan_pdf_audio(config, &target_plan, options, None)?;
    // With --listen `{target}` is the cleaned URL (the listen renderer does
    // its own arXiv rewrite) and `{pdf}` is the fetched scratch copy.
    let (listen_flow, audio) = match listen {
        Some(command) => {
            let flow = super::listen::plan_listen_flow(
                &target_plan,
                scratch,
                &final_stem,
            )?;
            let display = listen_audio_plan(&flow);
            (Some((command, flow)), Some(display))
        }
        None => (None, audio),
    };
    let id = Some(final_stem.clone());
    let marker = pdf_target_mod::compose_pdf_marker(
        &options.status,
        &options.parent,
        &pdf_plan,
        id.as_deref(),
    )?;
    let info = PdfInfo {
        title: Some(pdf_plan.title.clone()),
        author: pdf_plan.author.clone(),
    };
    let styler = Styler::detect();
    if options.dry_run {
        if let Some((command, flow)) = &listen_flow {
            println!(
                "{}",
                super::listen::would_run_line(
                    command,
                    &super::listen::ListenValues {
                        target: url.cleaned.clone(),
                        pdf: dest.clone(),
                        audio: flow.scratch_audio.clone(),
                        title: pdf_plan.title.clone(),
                    },
                )
            );
        }
        let mut legacy = sources_mod::legacy_hits(recorded, &key);
        for hit in sources_mod::legacy_hits(recorded, &url.dedupe_key) {
            if !legacy.iter().any(|known| known.path == hit.path) {
                legacy.push(hit);
            }
        }
        print_pdf_dry_run(
            config,
            &styler,
            &paper.abs_url(),
            &format!("arXiv {}", paper.full_id()),
            &target_plan,
            &pdf_plan,
            id.as_deref(),
            &marker,
            audio.as_ref(),
            options,
            &legacy,
        );
        return Ok(());
    }
    let installed = match &listen_flow {
        Some((command, flow)) => stamp_listen_and_install_pdf(
            scratch,
            &target_plan,
            command,
            flow,
            &url.cleaned,
            &dest,
            &pdf_plan.title,
            &url.cleaned,
            &dest,
            &marker,
            &info,
            options.force.is_some(),
        )?,
        None => {
            let (stamped, page_count) = pdf_target_mod::stamp_pdf_to_scratch(
                &dest, scratch, &marker, &info,
            )?;
            pdf_target_mod::install_stamped_pdf(
                &stamped,
                &target_plan.target,
                audio.as_ref(),
                page_count,
            )?
        }
    };
    let bytes = fs::metadata(&target_plan.target)
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "{} created Highlights-ready PDF",
        styler.success_prefix(false)
    );
    println!("source: {} (arXiv {})", paper.abs_url(), paper.full_id());
    print_reread_or_renamed(&target_plan, options);
    println!("pdf: {}", target_plan.target.display());
    if let Some(audio) = &audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("title: {}", pdf_plan.title);
    if let Some(author) = &pdf_plan.author {
        println!("author: {author}");
    }
    if let Some(published) = &pdf_plan.published {
        println!("published: {published}");
    }
    println!("captured: {captured}");
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    println!("id: {final_stem}");
    println!(
        "pages: {installed} · size: {}",
        super::clip::format_bytes(bytes)
    );
    finish_create_intake(
        config,
        &target_plan,
        options.keep_intake_reason.as_deref(),
    )?;
    Ok(())
}

fn plan_pdf_audio(
    config: &Config,
    target_plan: &TargetPlan,
    options: &CreateOptions,
    extra_beside: Option<&Path>,
) -> Result<Option<AudioCopyPlan>> {
    // With --listen the episode is narrated fresh; discovery is skipped
    // and the callers plan the `<target>.mp3` destination instead.
    if options.listen {
        return Ok(None);
    }
    if options.no_audio {
        if options.audio.is_some() {
            return Err(CommandError::new(
                "--no-audio cannot be used with --audio",
            ));
        }
        return Ok(None);
    }
    // On a reread earlier audio is never inherited: skip reused-companion
    // discovery unless an explicit `--audio` path is given.
    if options.reread.is_some() {
        if let Some(explicit) = &options.audio {
            return companion_mod::plan_explicit_audio(
                config,
                target_plan,
                options.force.is_some(),
                explicit,
            );
        }
        return Ok(None);
    }
    if let Some(explicit) = &options.audio {
        return companion_mod::plan_explicit_audio(
            config,
            target_plan,
            options.force.is_some(),
            explicit,
        );
    }
    companion_mod::plan_reused_companion(target_plan, extra_beside)
}

fn print_pdf_dry_run(
    config: &Config,
    styler: &Styler,
    source_display: &str,
    source_kind: &str,
    target_plan: &TargetPlan,
    pdf_plan: &pdf_target_mod::PdfPlan,
    id: Option<&str>,
    marker: &str,
    audio: Option<&AudioCopyPlan>,
    options: &CreateOptions,
    legacy: &[sources_mod::RecordedSource],
) {
    let _ = config;
    println!(
        "{} would create Highlights-ready PDF",
        styler.success_prefix(true)
    );
    if source_kind.starts_with("arXiv") {
        println!("source: {source_display} ({source_kind})");
    } else if source_kind == "PDF" {
        println!("source: {source_display} (PDF)");
    } else {
        println!("source: {source_display}");
    }
    print_reread_or_renamed(target_plan, options);
    println!("pdf: {}", target_plan.target.display());
    if let Some(audio) = audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("sidecar_guard: {}", target_plan.sidecar.display());
    match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => println!("library_destination: {}", library_destination.display()),
        _ => println!("library_destination: n/a (external target)"),
    }
    println!("title: {} ({})", pdf_plan.title, pdf_plan.title_source);
    if let Some(author) = &pdf_plan.author {
        println!(
            "author: {author} ({})",
            pdf_plan.author_source.unwrap_or("pdf info")
        );
    }
    if let Some(published) = &pdf_plan.published {
        println!(
            "published: {published} ({})",
            pdf_plan.published_source.unwrap_or("arxiv api")
        );
    }
    if let Some(captured) = &pdf_plan.captured {
        println!("captured: {captured}");
    }
    for hit in legacy {
        println!(
            "legacy: {} (superseded by this capture)",
            hit.path.display()
        );
    }
    println!("status: {}", options.status);
    print_parent_line(options);
    if let Some(id) = id {
        println!("id: {id}");
    }
    println!("marker:");
    print!("{marker}");
    if !marker.ends_with('\n') {
        println!();
    }
    print_create_intake_preview(
        config,
        target_plan,
        options.keep_intake_reason.as_deref(),
    );
    println!("writes: none");
}

#[cfg(test)]
fn plan_create(
    config: &Config,
    source: &Path,
    options: &CreateOptions,
) -> Result<CreatePlan> {
    plan_markdown(config, source, options, &source.display().to_string())
}

fn plan_markdown(
    config: &Config,
    source: &Path,
    options: &CreateOptions,
    source_display: &str,
) -> Result<CreatePlan> {
    validate_markdown_path(source)?;
    let canonical = fs::canonicalize(source).map_err(|error| {
        CommandError::new(format!(
            "resolve Markdown file {}: {error}",
            source.display()
        ))
    })?;
    let markdown = fs::read_to_string(&canonical).map_err(|error| {
        CommandError::new(format!(
            "read Markdown file {} as UTF-8: {error}",
            canonical.display()
        ))
    })?;
    // Validate -i first so non-UTF8 stems report the marker-id error,
    // matching the legacy planner order.
    if options.include_id && options.name.is_none() {
        let validated = derive_marker_id(&canonical)?;
        let file_stem = canonical
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        let stem = match &options.name {
            Some(name) => super::clip_url::validate_name(name)?,
            None => {
                if file_stem.is_empty() {
                    return Err(CommandError::new(format!(
                        "Markdown file has no file stem: {}",
                        canonical.display()
                    )));
                }
                file_stem.clone()
            }
        };
        let _ = (validated, stem);
    }
    // Stem: -N wins, else the file stem.
    let file_stem_os = canonical.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "Markdown file has no file stem: {}",
            canonical.display()
        ))
    })?;
    let file_stem = file_stem_os.to_str().unwrap_or_default().to_string();
    let stem = match &options.name {
        Some(name) => super::clip_url::validate_name(name)?,
        None => {
            if file_stem.is_empty() {
                // Non-UTF8 stems fail here only when -i did not already
                // fail above; report the marker-id error for consistency.
                if options.include_id {
                    derive_marker_id(&canonical)?;
                }
                return Err(CommandError::new(format!(
                    "Markdown file has no file stem: {}",
                    canonical.display()
                )));
            }
            file_stem.clone()
        }
    };
    // Title: -T wins, else frontmatter/H1/stem. The title is the local
    // identity, so it is computed before the default-target walk.
    let title = match &options.title {
        Some(override_title) if !override_title.is_empty() => {
            override_title.clone()
        }
        _ => extract_title(&markdown, &canonical)?,
    };
    let author = options.author.clone().filter(|value| !value.is_empty());
    let published = options.published.clone().filter(|value| !value.is_empty());
    let ref_type =
        default_ref_type_for_kind("markdown", options.ref_type.as_deref())?;
    let identity = DefaultTargetIdentity::Title(title.clone());
    // A reread plans its allocated `<root>__vN` stem with no suffix
    // fallback and always stamps `id` (the birth lineage keys on it);
    // `-o` on a captured reread was refused at preflight.
    let target_plan = match &options.reread {
        Some(preflight) => super::recapture::plan_reread_target(
            config,
            &preflight.plan.stem,
            &ref_type,
            &identity,
        )?,
        None => plan_target_for(
            config,
            &stem,
            &ref_type,
            options.output.as_ref(),
            options.force,
            options.include_id,
            &identity,
            source_display,
        )?,
    };
    // The marker id always follows the final output basename, including
    // an exact `--output` path. Rereads always stamp it.
    let id = if options.include_id || options.reread.is_some() {
        Some(target_plan.stem.clone())
    } else {
        None
    };
    let marker = compose_markdown_marker(
        &options.status,
        &options.parent,
        &title,
        id.as_deref(),
        author.as_deref(),
        published.as_deref(),
        None,
        false,
    )?;

    let audio =
        plan_audio_copy(config, &canonical, &markdown, &target_plan, options)?;

    Ok(CreatePlan {
        source: canonical.display().to_string(),
        source_kind: "markdown",
        target: target_plan.target.clone(),
        sidecar: target_plan.sidecar.clone(),
        workflow: target_plan.workflow.clone(),
        stem: target_plan.stem.clone(),
        renamed_from: target_plan.renamed_from.clone(),
        title,
        title_source: "markdown",
        author,
        published,
        source_url: None,
        source_note: None,
        source_markdown: Some(markdown),
        captured: None,
        id,
        marker,
        audio,
        page_count_hint: None,
    })
}

/// Compose the Markdown-route page-1 marker. `plan_markdown` previews it
/// without the key for dry runs; `create_markdown_route` recomposes with
/// `return_links: true` after a render whose report says links were paired.
fn compose_markdown_marker(
    status: &str,
    parent: &str,
    title: &str,
    id: Option<&str>,
    author: Option<&str>,
    published: Option<&str>,
    source_note: Option<&str>,
    return_links: bool,
) -> Result<String> {
    let mut extras: Vec<(&str, MarkerValue)> = Vec::new();
    if let Some(author) = author.filter(|value| !value.is_empty()) {
        extras.push(("author", MarkerValue::String(author.to_string())));
    }
    if let Some(published) = published.filter(|value| !value.is_empty()) {
        extras.push(("published", MarkerValue::String(published.to_string())));
    }
    if let Some(source_note) = source_note.filter(|value| !value.is_empty()) {
        extras.push((
            FIELD_SOURCE_NOTE,
            MarkerValue::String(source_note.to_string()),
        ));
    }
    if return_links {
        extras.push(("return_links", MarkerValue::Bool(true)));
    }
    compose_marker(
        status,
        parent,
        title,
        id,
        &extras
            .iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect::<Vec<_>>(),
    )
}

/// The marker to stamp after a Markdown render: the planned marker plus
/// `return_links: true` only when the filter report says the render actually
/// paired at least one link (`Missing` test-double renders, `Invalid`
/// reports, opt-outs, and link-free documents keep the planned marker).
fn stamp_marker_for_outcome(
    plan: &CreatePlan,
    options: &CreateOptions,
    outcome: &super::return_links::ReportOutcome,
) -> Result<String> {
    let paired = matches!(
        outcome,
        super::return_links::ReportOutcome::Report(report)
            if report.enabled && report.paired >= 1
    );
    if !paired {
        return Ok(plan.marker.clone());
    }
    compose_markdown_marker(
        &options.status,
        &options.parent,
        &plan.title,
        plan.id.as_deref(),
        plan.author.as_deref(),
        plan.published.as_deref(),
        plan.source_note.as_deref(),
        true,
    )
}

fn derive_marker_id(source: &Path) -> Result<String> {
    let stem = source.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "Markdown source {} has no filename stem for marker id",
            source.display()
        ))
    })?;
    stem.to_str()
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CommandError::new(format!(
                "Markdown source {} filename stem is not a nonempty UTF-8 marker id",
                source.display()
            ))
        })
}

fn validate_markdown_path(source: &Path) -> Result<()> {
    if !source.is_file() {
        return Err(CommandError::new(format!(
            "Markdown input does not exist or is not a file: {}",
            source.display()
        )));
    }
    if !source
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        return Err(CommandError::new(format!(
            "Markdown input must have a .md extension: {}",
            source.display()
        )));
    }
    Ok(())
}

fn extract_title(markdown: &str, source: &Path) -> Result<String> {
    if let Some(title) = frontmatter_title(markdown)? {
        return Ok(title);
    }
    if let Some(title) = markdown.lines().find_map(|line| {
        let title = line.strip_prefix("# ")?.trim();
        (!title.is_empty()).then(|| title.to_string())
    }) {
        return Ok(title);
    }
    source
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CommandError::new(format!(
                "could not derive a title from {}",
                source.display()
            ))
        })
}

fn frontmatter_title(markdown: &str) -> Result<Option<String>> {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(None);
    }

    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if matches!(line.trim(), "---" | "...") {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return Ok(None);
    }

    let value: serde_yaml::Value =
        serde_yaml::from_str(&yaml).map_err(|error| {
            CommandError::new(format!("parse Markdown frontmatter: {error}"))
        })?;
    let Some(title) = value
        .as_mapping()
        .and_then(|mapping| {
            mapping.get(serde_yaml::Value::String("title".into()))
        })
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(title.to_string()))
}

#[cfg(test)]
fn render_temp_path(target: &Path) -> Result<PathBuf> {
    let stem = target.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "target has no file stem: {}",
            target.display()
        ))
    })?;
    let mut name = OsString::from(".");
    name.push(stem);
    name.push(format!(".{}.render.pdf", std::process::id()));
    Ok(target.with_file_name(name))
}

fn play_uri(config: &Config, plan: &CreatePlan) -> Result<Option<String>> {
    let Some(audio_path) = bound_audio_path(plan) else {
        let has_listen_card = plan
            .source_markdown
            .as_deref()
            .map(|markdown| markdown.contains("class=\"listen\""))
            .unwrap_or_else(|| {
                fs::read_to_string(&plan.source)
                    .is_ok_and(|markdown| markdown.contains("class=\"listen\""))
            });
        if has_listen_card {
            eprintln!("warning: listen card has no bound companion audio");
        }
        return Ok(None);
    };

    let highlights = super::bob_config::load_highlights_config(
        &super::bob_config::config_path(),
    )
    .map_err(super::config_error)?;
    let template = super::note_snapshot::resolve_uri_template(
        bob_env::var_os(ENV_AUDIO_LINK_TEMPLATE).as_deref(),
        highlights.audio_link_template(),
        DEFAULT_AUDIO_LINK_TEMPLATE,
    );
    if template.is_empty() || matches!(plan.workflow, TargetWorkflow::External)
    {
        return Ok(None);
    }

    let vault = config
        .bob_dir
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            CommandError::new(format!(
                "bob directory has no UTF-8 basename: {}",
                config.bob_dir.display()
            ))
        })?;
    let relative = audio_path.strip_prefix(&config.bob_dir).map_err(|_| {
        CommandError::new(format!(
            "audio library path {} is outside bob directory {}",
            audio_path.display(),
            config.bob_dir.display()
        ))
    })?;
    let path = relative.to_string_lossy().replace('\\', "/");
    Ok(Some(super::note_snapshot::render_uri_template(
        &template, vault, &path,
    )))
}

fn bound_audio_path(plan: &CreatePlan) -> Option<PathBuf> {
    let audio = plan.audio.as_ref()?;
    match &plan.workflow {
        TargetWorkflow::Intake { .. } => audio.library_dest.clone(),
        TargetWorkflow::Library => Some(audio.dest.clone()),
        TargetWorkflow::External => None,
    }
}

fn plan_audio_copy(
    config: &Config,
    source_md: &Path,
    markdown: &str,
    target_plan: &TargetPlan,
    options: &CreateOptions,
) -> Result<Option<AudioCopyPlan>> {
    // With --listen the episode is narrated fresh: discovery is
    // skipped and the listen card's Play URI targets `<stem>.mp3`.
    if options.listen {
        return Ok(None);
    }
    if options.no_audio {
        if options.audio.is_some() {
            return Err(CommandError::new(
                "--no-audio cannot be used with --audio",
            ));
        }
        return Ok(None);
    }
    // On a reread earlier audio is never inherited: skip Markdown
    // auto-discovery unless an explicit `--audio` path is given.
    if options.reread.is_some() {
        if let Some(explicit) = &options.audio {
            return companion_mod::plan_explicit_audio(
                config,
                target_plan,
                options.force.is_some(),
                explicit,
            );
        }
        return Ok(None);
    }
    if let Some(explicit) = &options.audio {
        return companion_mod::plan_explicit_audio(
            config,
            target_plan,
            options.force.is_some(),
            explicit,
        );
    }
    let highlights = super::bob_config::load_highlights_config(
        &super::bob_config::config_path(),
    )
    .map_err(super::config_error)?;
    let library = super::audio::audio_library_root(highlights.audio_library());
    if let Some(episode_id) = super::audio::frontmatter_episode_id(markdown)? {
        if let Some(source) =
            super::audio::episode_audio_source(&library, &episode_id)
        {
            return companion_mod::plan_audio_copy_for_source(
                config,
                target_plan,
                options.force.is_some(),
                source,
                format!("episode {episode_id}"),
            );
        }
        eprintln!(
            "warning: highlights audio episode '{episode_id}' not found in library; trying narration script"
        );
    }
    if let Some(candidate) = super::audio::narration_script_candidate(source_md)
        && candidate.is_file()
    {
        let bytes = fs::read(&candidate).map_err(|error| {
            CommandError::new(format!(
                "read narration script {}: {error}",
                candidate.display()
            ))
        })?;
        let digest = hex::encode(sha2::Sha256::digest(bytes));
        if let Some(source) =
            super::audio::find_audio_by_script_hash(&library, &digest)
        {
            return companion_mod::plan_audio_copy_for_source(
                config,
                target_plan,
                options.force.is_some(),
                source,
                "narration sha256".to_string(),
            );
        }
    }
    companion_mod::plan_reused_companion(target_plan, Some(source_md))
}

/// Letter paper in PostScript points. Generic Markdown uses
/// `-V geometry:margin=0.85in` below; the budgeted diagram scale check uses
/// this text box (`612 - 2×0.85×72` by `792 - 2×0.85×72`) times
/// [`super::diagrams::FIT_HEIGHT_BUDGET`] for the height limit.
const LETTER_WIDTH_PT: f64 = 612.0;
const LETTER_HEIGHT_PT: f64 = 792.0;
const PT_PER_IN: f64 = 72.0;
const GENERIC_MARGIN_IN: f64 = 0.85;
pub(crate) const GENERIC_DIAGRAM_TEXT_WIDTH_PT: f64 =
    LETTER_WIDTH_PT - 2.0 * GENERIC_MARGIN_IN * PT_PER_IN;
pub(crate) const GENERIC_DIAGRAM_TEXT_HEIGHT_PT: f64 =
    LETTER_HEIGHT_PT - 2.0 * GENERIC_MARGIN_IN * PT_PER_IN;

/// Audio URIs render through the shared snapshot `obsidian://` helpers
/// (`note_snapshot::render_uri_template` in `play_uri` above), so both
/// routes encode vault and path components identically; only the template
/// source differs (audio keeps its env/config/default precedence,
/// snapshots use the default template).
fn render_temp_pdf(
    pandoc: &OsStr,
    plan: &CreatePlan,
    render_path: &Path,
    filters: &RenderFilters,
    audio_uri: Option<&str>,
) -> Result<super::return_links::ReportOutcome> {
    let source_path = Path::new(&plan.source);
    let resource_path = source_path.parent().unwrap_or_else(|| Path::new("."));
    let mut cmd = Command::new(pandoc);
    cmd.arg(&plan.source)
        .arg("-o")
        .arg(render_path)
        .arg("--standalone")
        .arg("--toc")
        .arg("--toc-depth=3")
        .arg("--number-sections")
        .arg("--pdf-engine=xelatex")
        .arg(format!("--resource-path={}", resource_path.display()))
        .arg("--highlight-style=tango");
    if let Some(diagrams) = &filters.diagrams {
        super::diagrams::apply_swap_filter_args(
            &mut cmd,
            &diagrams.filter,
            &diagrams.results,
        );
    }
    let output = cmd
        .arg("--lua-filter")
        .arg(&filters.code_break)
        .arg("--lua-filter")
        .arg(&filters.return_links)
        .arg("-V")
        .arg("colorlinks=true")
        .arg("-V")
        .arg("linkcolor=BobLinkInk")
        .arg("-V")
        .arg("urlcolor=BobLinkInk")
        .arg("-V")
        .arg("filecolor=BobLinkInk")
        .arg("-V")
        .arg("citecolor=BobLinkInk")
        .arg("-V")
        .arg("geometry:margin=0.85in")
        .arg("-V")
        .arg("fontsize=10pt")
        .arg("-V")
        .arg("linestretch=1.08")
        .arg("-V")
        .arg("mainfont=DejaVu Serif")
        .arg("-V")
        .arg("sansfont=DejaVu Sans")
        .arg("-V")
        .arg("monofont=DejaVu Sans Mono")
        .arg("-V")
        .arg("mainfontoptions=Ligatures=NoCommon")
        .arg("-V")
        .arg("sansfontoptions=Ligatures=NoCommon")
        .arg("-V")
        .arg("monofontoptions=Ligatures=NoCommon")
        .arg("-V")
        .arg(format!(
            "header-includes={PANDOC_HEADER_INCLUDES}\n{}\n{}",
            super::return_links::HEADER_INCLUDES,
            super::diagrams::HEADER_INCLUDES
        ))
        .arg("--metadata")
        .arg(format!("title={}", plan.title))
        .arg("--metadata")
        .arg(format!(
            "{}={}",
            super::return_links::REPORT_METADATA_KEY,
            filters.report.display()
        ))
        .args(audio_uri.into_iter().flat_map(|uri| {
            ["--metadata".to_string(), format!("bob-listen-uri={uri}")]
        }))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            CommandError::new(format!(
                "run pandoc command {}: {error}",
                Path::new(pandoc).display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = match (stderr.is_empty(), stdout.is_empty()) {
            (false, false) => format!("{stderr}\n{stdout}"),
            (false, true) => stderr,
            (true, false) => stdout,
            (true, true) => "pandoc produced no diagnostic output".to_string(),
        };
        let base = format!(
            "pandoc failed while rendering {} (exit {}):\n{}",
            plan.source,
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            detail
        );
        if let Some(hint) = super::render_tex::render_failure_hint(&detail) {
            return Err(CommandError::new(format!("{base}\nhint: {hint}")));
        }
        return Err(CommandError::new(base));
    }

    Ok(super::return_links::read_report(&filters.report))
}

/// The dry-run parent line: the resolved `parent    route  (kind · label
/// [· via alias])` line for an explicit `-P`, else today's `parent: …` line.
fn print_parent_line(options: &CreateOptions) {
    let line = options
        .resolved_parent
        .as_ref()
        .map(|resolved| resolved.dry_run_line())
        .unwrap_or_else(|| format!("parent: {}", options.parent));
    if let Some(reason) = &options.parent_default_reason {
        println!("{line} · default: {reason}");
    } else {
        println!("{line}");
    }
}

fn print_plan(
    config: &Config,
    plan: &CreatePlan,
    options: &CreateOptions,
    styler: &Styler,
) {
    println!(
        "{} would create Highlights-ready PDF",
        styler.success_prefix(true)
    );
    println!("source: {}", plan.source);
    print_reread_or_renamed(&plan.target_plan(), options);
    println!("pdf: {}", plan.target.display());
    if let Some(audio) = &plan.audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else {
        println!("audio: none");
    }
    println!("sidecar_guard: {}", plan.sidecar.display());
    if let TargetWorkflow::Intake {
        library_destination,
    } = &plan.workflow
    {
        println!("library_destination: {}", library_destination.display());
    }
    println!("title: {}", plan.title);
    if let Some(author) = &plan.author {
        println!("author: {author} (override)");
    }
    if let Some(published) = &plan.published {
        println!("published: {published} (override)");
    }
    println!("status: {}", options.status);
    print_parent_line(options);
    if let Some(id) = &plan.id {
        println!("id: {id}");
    }
    println!("marker:");
    print!("{}", plan.marker);
    print_create_intake_preview(
        config,
        &plan.target_plan(),
        options.keep_intake_reason.as_deref(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::{
        parse_marker_with_normalization, MarkerValue, FIELD_ID,
    };
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-highlights-create-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn options() -> CreateOptions {
        CreateOptions {
            audio: None,
            author: None,
            closed: false,
            dry_run: false,
            force: None,
            html: None,
            include_id: false,
            keep_intake_reason: None,
            listen: false,
            name: None,
            no_audio: false,
            output: None,
            plain: false,
            parent: "mac_inbox".to_string(),
            parent_default_reason: None,
            resolved_parent: None,
            published: None,
            sections: Vec::new(),
            ref_type: None,
            status: DEFAULT_STATUS.to_string(),
            title: None,
            reread: None,
        }
    }

    fn config(root: &Path) -> Config {
        Config {
            bob_dir: root.to_path_buf(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        }
    }

    fn write_ref(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create ref parent");
        }
        fs::write(path, body).expect("write ref note");
    }

    #[test]
    fn earlier_snapshots_sort_newest_first_and_hint_unread_ready() {
        let temp = TempDir::new("earlier-snapshots");
        let refs = temp.path.join("ref");
        write_ref(
            &refs.join("notes/harbor_cranes_261016.md"),
            "---\nsource_note: \"[[Harbor]]\"\nstatus: ready\ncreated: 2026-10-16T12:00:00-0400\n---\n",
        );
        write_ref(
            &refs.join("notes/harbor_cranes_261010.md"),
            "---\nsource_note: \"[[Harbor]]\"\nstatus: ready\ncreated: 2026-10-10T09:00:00-0400\n---\n",
        );
        write_ref(
            &refs.join("notes/harbor_cranes_260930.md"),
            "---\nsource_note: \"[[Harbor]]\"\nstatus: read\nhighlights_count: 7\ncreated: 2026-09-30T08:00:00-0400\n---\n",
        );
        write_ref(
            &refs.join("notes/harbor_cranes_260901.md"),
            "---\nsource_note: \"[[Harbor]]\"\nstatus: abandoned\ncreated: 2026-09-01T08:00:00-0400\n---\n",
        );
        write_ref(
            &refs.join("notes/harbor_cranes_legacy.md"),
            "---\nsource_note: \"[[Harbor]]\"\nlegacy_status: ready\ncreated: 2026-08-01T08:00:00-0400\n---\n",
        );
        write_ref(
            &refs.join("notes/other_note.md"),
            "---\nsource_note: \"[[Elsewhere]]\"\nstatus: ready\ncreated: 2026-10-20T08:00:00-0400\n---\n",
        );

        let earlier =
            collect_earlier_snapshots(&refs, "Harbor", "harbor_cranes_261016");
        let listings: Vec<String> =
            earlier.iter().map(EarlierSnapshot::listing).collect();
        assert_eq!(
            listings,
            [
                "harbor_cranes_261010 (ready, 0 highlights)",
                "harbor_cranes_260930 (read, 7 highlights)",
                "harbor_cranes_260901 (abandoned, 0 highlights)",
                "harbor_cranes_legacy (ready, 0 highlights)",
            ]
        );
        let hints: Vec<String> = earlier
            .iter()
            .filter(|item| item.is_unread_ready())
            .map(EarlierSnapshot::hint_line)
            .collect();
        assert_eq!(
            hints,
            [
                "hint: earlier snapshot harbor_cranes_261010 is still unread (0 highlights)",
                "hint: earlier snapshot harbor_cranes_legacy is still unread (0 highlights)",
            ]
        );
    }

    #[test]
    fn title_prefers_frontmatter_then_h1_then_stem() {
        let source = Path::new("/tmp/fallback-title.md");
        assert_eq!(
            extract_title("---\ntitle: Frontmatter Title\n---\n# H1\n", source)
                .expect("frontmatter title"),
            "Frontmatter Title"
        );
        assert_eq!(
            extract_title("## Intro\n# H1 Title\n", source).expect("h1 title"),
            "H1 Title"
        );
        assert_eq!(
            extract_title("No title here.\n", source).expect("stem title"),
            "fallback-title"
        );
    }

    #[test]
    fn plan_embeds_exact_output_stem_id_when_opted_in() {
        let temp = TempDir::new("include-id");
        let source = temp
            .path
            .join("202608/xprompt_role_binding/xprompt_role_binding.md");
        fs::create_dir_all(source.parent().expect("source parent"))
            .expect("create source parent");
        fs::write(&source, "# Xprompt Role Binding\n").expect("write source");
        let mut options = options();
        options.include_id = true;
        options.output = Some(temp.path.join("custom-export.pdf"));

        let plan =
            plan_create(&config(&temp.path), &source, &options).expect("plan");

        assert_eq!(plan.target, temp.path.join("custom-export.pdf"));
        assert_eq!(plan.id.as_deref(), Some("custom-export"));
        let marker =
            parse_marker_with_normalization(&plan.marker).expect("marker");
        assert_eq!(
            marker.projection.get(FIELD_ID),
            Some(&MarkerValue::String("custom-export".to_string()))
        );
    }

    #[test]
    fn markdown_marker_gains_return_links_only_after_paired_render() {
        let temp = TempDir::new("stamp-marker-outcome");
        let source = temp.path.join("report.md");
        fs::write(&source, "# Report\n").expect("write source");
        let plan = plan_create(&config(&temp.path), &source, &options())
            .expect("plan");
        assert!(
            !plan.marker.contains("return_links"),
            "dry-run preview must not carry the key: {}",
            plan.marker
        );

        fn outcome(
            enabled: bool,
            paired: u32,
        ) -> super::super::return_links::ReportOutcome {
            super::super::return_links::ReportOutcome::Report(
                super::super::return_links::Report {
                    version: 1,
                    enabled,
                    prefix: "bob:ret:".to_string(),
                    paired,
                    targets: 1,
                    untagged: 0,
                    github: 0,
                    dead: Vec::new(),
                    duplicates: Vec::new(),
                },
            )
        }

        let stamped =
            stamp_marker_for_outcome(&plan, &options(), &outcome(true, 2))
                .expect("stamp marker");
        assert!(stamped.contains("- return_links: true\n"), "{stamped}");
        let parsed = parse_marker_with_normalization(&stamped).expect("marker");
        assert_eq!(
            parsed.projection.get("return_links"),
            Some(&MarkerValue::Bool(true))
        );

        for unchanged in [
            super::super::return_links::ReportOutcome::Missing,
            super::super::return_links::ReportOutcome::Invalid(
                "bad".to_string(),
            ),
            outcome(false, 3),
            outcome(true, 0),
        ] {
            assert_eq!(
                stamp_marker_for_outcome(&plan, &options(), &unchanged)
                    .expect("stamp marker"),
                plan.marker,
                "{unchanged:?} must keep the planned marker"
            );
        }
    }

    #[test]
    fn code_break_filter_splits_long_inline_code_paths() {
        let Some(pandoc) = pandoc_command() else {
            eprintln!("skipping code break filter test: pandoc is required");
            return;
        };
        let temp = TempDir::new("code-break");
        let filter = temp.path.join("code-break.lua");
        fs::write(&filter, PANDOC_CODE_BREAK_FILTER).expect("write filter");
        let source = temp.path.join("report.md");
        fs::write(&source, "Path `src/sase/ace/tui.py` and `bead`.\n")
            .expect("write source");

        let output = Command::new(&pandoc)
            .arg(&source)
            .arg("--to=latex")
            .arg("--lua-filter")
            .arg(&filter)
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        let latex = String::from_utf8_lossy(&output.stdout);

        assert!(
            latex.contains(r"\texttt{src/}\allowbreak{}"),
            "long inline code must gain break points: {latex}"
        );
        assert!(
            latex.contains(r"\texttt{bead}"),
            "code without separators must stay a single span: {latex}"
        );
    }

    fn section_latex(source_text: &str, numbered: bool, toc: bool) -> String {
        let pandoc = pandoc_command().unwrap_or_else(|| {
            panic!("skipping section numbering test: pandoc is required")
        });
        let id = std::sync::atomic::AtomicU64::fetch_add(
            {
                static COUNTER: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                &COUNTER
            },
            1,
            std::sync::atomic::Ordering::Relaxed,
        );
        let temp = TempDir::new(&format!("section-numbering-{id}"));
        let filter = temp.path.join("filter.lua");
        fs::write(&filter, PANDOC_CODE_BREAK_FILTER).expect("write filter");
        let source = temp.path.join("report.md");
        fs::write(&source, source_text).expect("write source");
        let mut cmd = Command::new(&pandoc);
        cmd.arg(&source).arg("--to=latex");
        if numbered {
            cmd.arg("--number-sections");
        }
        if toc {
            cmd.arg("-s").arg("--toc");
        }
        cmd.arg("--lua-filter").arg(&filter);
        let output = cmd.output().expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    const LONE_H1_SOURCE: &str = "# Report\n\n## Setup\n\n### Details\n\n## Results\n\n```\n# not a heading\n```\n";

    #[test]
    fn section_numbering_lone_atx_h1_moves_sections_up() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let latex = section_latex(LONE_H1_SOURCE, true, false);
        assert!(latex.contains(r"\section*{Report}"), "{latex}");
        assert!(latex.contains(r"\label{report}"), "{latex}");
        assert!(
            !latex.contains(r"\addcontentsline{toc}{section}{Report}"),
            "lone H1 stays out of Contents: {latex}"
        );
        assert!(latex.contains(r"\section{Setup}"), "{latex}");
        assert!(latex.contains(r"\subsection{Details}"), "{latex}");
        assert!(latex.contains(r"\section{Results}"), "{latex}");
        assert!(!latex.contains(r"\subsection{Setup}"), "{latex}");
    }

    #[test]
    fn section_numbering_lone_setext_h1_moves_sections_up() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let latex = section_latex("Report\n======\n\n## Setup\n", true, false);
        assert!(latex.contains(r"\section*{Report}"), "{latex}");
        assert!(latex.contains(r"\section{Setup}"), "{latex}");
        assert!(!latex.contains(r"\subsection{Setup}"), "{latex}");
    }

    #[test]
    fn section_numbering_lone_h1_with_only_h3_children() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let latex = section_latex(
            "# Report\n\n### Deep one\n\n### Deep two\n",
            true,
            false,
        );
        assert!(latex.contains(r"\section*{Report}"), "{latex}");
        assert!(latex.contains(r"\section{Deep one}"), "{latex}");
        assert!(latex.contains(r"\section{Deep two}"), "{latex}");
    }

    #[test]
    fn section_numbering_without_h1_numbers_from_shallowest() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let latex = section_latex("## Setup\n\n### Details\n", true, false);
        assert!(latex.contains(r"\section{Setup}"), "{latex}");
        assert!(latex.contains(r"\subsection{Details}"), "{latex}");
    }

    #[test]
    fn section_numbering_two_h1s_keeps_levels() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let latex = section_latex("# One\n\n## Child\n\n# Two\n", true, false);
        assert!(latex.contains(r"\section{One}"), "{latex}");
        assert!(latex.contains(r"\subsection{Child}"), "{latex}");
        assert!(latex.contains(r"\section{Two}"), "{latex}");
        assert!(!latex.contains(r"\section*{"), "{latex}");
    }

    #[test]
    fn section_numbering_gate_off_keeps_levels() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let latex = section_latex(LONE_H1_SOURCE, false, false);
        assert!(latex.contains(r"\section{Report}"), "{latex}");
        assert!(latex.contains(r"\subsection{Setup}"), "{latex}");
        assert!(!latex.contains(r"\section*{Report}"), "{latex}");
    }

    #[test]
    fn section_numbering_contents_kept_only_when_listed() {
        let Some(_) = pandoc_command() else {
            eprintln!("skipping section numbering test: pandoc is required");
            return;
        };
        let with_sections = section_latex(LONE_H1_SOURCE, true, true);
        assert!(
            with_sections.contains(r"\tableofcontents"),
            "lone H1 with sections keeps Contents: {with_sections}"
        );
        let lone_only = section_latex("# Only\n", true, true);
        assert!(
            !lone_only.contains(r"\tableofcontents"),
            "lone-H1-only file skips Contents: {lone_only}"
        );
        let headingless =
            section_latex("Just text, no headings.\n", true, true);
        assert!(
            !headingless.contains(r"\tableofcontents"),
            "heading-less file skips Contents: {headingless}"
        );
    }

    #[test]
    fn section_numbering_xelatex_render_numbers_from_top_level() {
        if skip_pandoc_xelatex("section numbering pdf") {
            return;
        }
        let temp = TempDir::new("section-numbering-pdf");
        let source = temp.path.join("report.md");
        fs::write(&source, LONE_H1_SOURCE).expect("write source");
        let target = temp.path.join("xlib/chat/report.pdf");
        fs::create_dir_all(target.parent().expect("target parent"))
            .expect("create output directory");
        let plan = CreatePlan {
            source: source.display().to_string(),
            source_kind: "markdown",
            target: target.clone(),
            sidecar: target.with_extension("md"),
            workflow: TargetWorkflow::Intake {
                library_destination: temp.path.join("lib/chat/report.pdf"),
            },
            stem: "report".to_string(),
            renamed_from: None,
            title: "Report".to_string(),
            title_source: "markdown",
            author: None,
            published: None,
            source_url: None,
            source_note: None,
            source_markdown: Some(LONE_H1_SOURCE.to_string()),
            captured: None,
            id: None,
            marker: compose_marker(
                "ready",
                "obsidian_ref",
                "Report",
                None,
                &[],
            )
            .expect("compose marker"),
            audio: None,
            page_count_hint: None,
        };
        let render_path = render_temp_path(&target).expect("render path");
        let filters = filters_with_optional_diagrams(&temp.path, None);
        let pandoc = pandoc_command().expect("pandoc checked above");
        render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
            .expect("render section numbering PDF");
        let document =
            lopdf::Document::load(&render_path).expect("load rendered PDF");
        assert_eq!(
            outline_titles(&document),
            vec!["Setup", "Details", "Results"],
            "title bookmark stays out of the outline"
        );
        let text = pdf_haystack(&render_path);
        let normalized: Vec<String> = text
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        let has_prefix = |prefix: &str| {
            normalized.iter().any(|line| {
                line.strip_prefix(prefix).is_some_and(|rest| {
                    rest.is_empty() || rest.starts_with(' ')
                })
            })
        };
        assert!(has_prefix("1 Setup"), "Setup numbers 1: {text}");
        assert!(has_prefix("1.1 Details"), "Details numbers 1.1: {text}");
        assert!(has_prefix("2 Results"), "Results numbers 2: {text}");
        assert!(
            !normalized.iter().any(|line| line.contains("0.1")),
            "no zero-based numbers: {text}"
        );
        assert!(
            !has_prefix("1 Report"),
            "title heading prints unnumbered: {text}"
        );
    }

    #[test]
    fn listen_filter_renders_card_and_encoded_play_link() {
        let Some(pandoc) = pandoc_command() else {
            eprintln!("skipping listen filter test: pandoc is required");
            return;
        };
        let temp = TempDir::new("listen-card");
        let filter = temp.path.join("listen-card.lua");
        fs::write(&filter, PANDOC_CODE_BREAK_FILTER).expect("write filter");
        let source = temp.path.join("report.md");
        fs::write(
            &source,
            "<div class=\"listen\">\n\n♫ **Brief audio edition** · 4 min · 3 chapters · [Narration script](report_narration.md)\n\n</div>\n",
        )
        .expect("write source");

        let output = Command::new(&pandoc)
            .arg(&source)
            .arg("--to=latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg("bob-listen-uri=obsidian://open?vault=Research%20Notes&file=lib%2Fchat%2Freport.mp3")
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        let latex = String::from_utf8_lossy(&output.stdout);
        assert!(latex.contains(r"\BobListenCard{"), "{latex}");
        assert!(latex.contains(r"\BobListenPlay{}"), "{latex}");
        // Pandoc 3.1.3 escapes the `&` in the `\href` target as `\&`;
        // Pandoc 3.1.11.1 emits a bare `&`. Both compile to the same PDF
        // link target (hyperref accepts both), so accept either form here
        // while still pinning the percent-encoded URI segments below.
        assert!(
            latex.contains(
                r"obsidian://open?vault=Research\%20Notes\&file=lib\%2Fchat\%2Freport.mp3"
            ) || latex.contains(
                r"obsidian://open?vault=Research\%20Notes&file=lib\%2Fchat\%2Freport.mp3"
            ),
            "{latex}"
        );
        assert!(latex.contains(r"Research\%20Notes"), "{latex}");
        assert!(latex.contains(r"lib\%2Fchat\%2Freport.mp3"), "{latex}");
        assert!(!latex.contains("report_narration.md"), "{latex}");
        assert!(!latex.contains("♫"), "{latex}");
    }

    #[test]
    fn listen_filter_keeps_glyph_without_bound_audio() {
        let Some(pandoc) = pandoc_command() else {
            eprintln!("skipping listen filter test: pandoc is required");
            return;
        };
        let temp = TempDir::new("listen-card-unbound");
        let filter = temp.path.join("listen-card.lua");
        fs::write(&filter, PANDOC_CODE_BREAK_FILTER).expect("write filter");
        let source = temp.path.join("report.md");
        fs::write(
            &source,
            "<div class=\"listen\">\n\n♫ **Brief audio edition** · 4 min · 3 chapters · [Narration script](report_narration.md)\n\n</div>\n",
        )
        .expect("write source");

        let output = Command::new(&pandoc)
            .arg(&source)
            .arg("--to=latex")
            .arg("--lua-filter")
            .arg(&filter)
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        let latex = String::from_utf8_lossy(&output.stdout);
        assert!(latex.contains(r"\BobListenCard{"), "{latex}");
        assert!(latex.contains("♫"), "{latex}");
        assert!(!latex.contains("\\href"), "{latex}");
        assert!(!latex.contains("report_narration.md"), "{latex}");
    }

    #[test]
    fn audio_path_components_are_percent_encoded() {
        use super::super::note_snapshot::percent_encode;
        assert_eq!(percent_encode("Research Notes"), "Research%20Notes");
        assert_eq!(
            percent_encode("lib/chat/hello+world.mp3"),
            "lib%2Fchat%2Fhello%2Bworld.mp3"
        );
    }

    #[test]
    fn audio_link_template_precedence_is_env_then_config_then_default() {
        use super::super::note_snapshot::resolve_uri_template;
        let resolve = |env: Option<&OsStr>, configured: Option<&str>| {
            resolve_uri_template(env, configured, DEFAULT_AUDIO_LINK_TEMPLATE)
        };
        assert_eq!(
            resolve(
                Some(OsStr::new("custom:{vault}:{path}")),
                Some("configured"),
            ),
            "custom:{vault}:{path}"
        );
        assert_eq!(resolve(None, Some("configured")), "configured");
        assert_eq!(resolve(None, None), DEFAULT_AUDIO_LINK_TEMPLATE);
        assert_eq!(resolve(Some(OsStr::new("")), None), "");
    }

    #[test]
    fn listen_card_xelatex_render_uses_only_existing_packages() {
        if pandoc_command().is_none()
            || Command::new("xelatex").arg("--version").output().is_err()
        {
            eprintln!("skipping listen card PDF test: pandoc and xelatex are required");
            return;
        }
        let temp = TempDir::new("listen-card-pdf");
        let source = temp.path.join("report.md");
        fs::write(
            &source,
            "# Report\n\n<div class=\"listen\">\n\n♫ **Brief audio edition** · 1 min · 1 chapter\n\n</div>\n",
        )
        .expect("write source");
        let target = temp.path.join("xlib/chat/report.pdf");
        fs::create_dir_all(target.parent().expect("target parent"))
            .expect("create output directory");
        let plan = CreatePlan {
            source: source.display().to_string(),
            source_kind: "markdown",
            target: target.clone(),
            sidecar: target.with_extension("md"),
            workflow: TargetWorkflow::Intake {
                library_destination: temp.path.join("lib/chat/report.pdf"),
            },
            stem: "report".to_string(),
            renamed_from: None,
            title: "Report".to_string(),
            title_source: "markdown",
            author: None,
            published: None,
            source_url: None,
            source_note: None,
            source_markdown: Some("# Report".to_string()),
            captured: None,
            id: None,
            marker: compose_marker(
                "ready",
                "obsidian_ref",
                "Report",
                None,
                &[],
            )
            .expect("compose marker"),
            audio: None,
            page_count_hint: None,
        };
        let render_path = render_temp_path(&target).expect("render path");
        let filter_path = temp.path.join("listen-card.lua");
        fs::write(&filter_path, PANDOC_CODE_BREAK_FILTER)
            .expect("write filter");
        let return_filter_path = temp.path.join("return-links.lua");
        fs::write(&return_filter_path, super::super::return_links::FILTER)
            .expect("write return-link filter");
        let filters = RenderFilters {
            code_break: filter_path,
            return_links: return_filter_path,
            report: temp.path.join("return-links.json"),
            diagrams: None,
        };
        let pandoc = pandoc_command().expect("pandoc checked above");
        render_temp_pdf(
            &pandoc,
            &plan,
            &render_path,
            &filters,
            Some("obsidian://open?vault=bob&file=lib%2Fchat%2Freport.mp3"),
        )
        .expect("render listen card PDF");
        let page_count = stamp_and_install(
            &render_path,
            &plan.target,
            &plan.marker,
            &PdfInfo::default(),
        )
        .expect("install listen card PDF");

        assert!(page_count > 0);
        let document =
            lopdf::Document::load(&target).expect("load rendered PDF");
        let uri_objects = document
            .objects
            .values()
            .filter(|object| {
                let debug = format!("{object:?}");
                debug.contains("URI") || debug.contains("obsidian")
            })
            .collect::<Vec<_>>();
        let has_play_uri = uri_objects.iter().any(|object| {
            format!("{object:?}").contains(
                "(obsidian://open?vault=bob&file=lib%2Fchat%2Freport.mp3)",
            )
        });
        assert!(
            has_play_uri,
            "rendered PDF should contain the Obsidian Play URI: {uri_objects:#?}"
        );
    }

    /// Modifier-letter tag glyphs from the return-link filter's `MOD` table,
    /// plus the pill arrow. Used to prove outlines stay clean and to parse
    /// pill rows out of `pdftotext` output.
    const RETURN_TAG_GLYPHS: &str = "ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏᵐⁿᵖʳˢᵗᵘᵛʷˣʸᶻ";
    const RETURN_GLYPH_LETTERS: [(char, char); 23] = [
        ('ᵃ', 'a'),
        ('ᵇ', 'b'),
        ('ᶜ', 'c'),
        ('ᵈ', 'd'),
        ('ᵉ', 'e'),
        ('ᶠ', 'f'),
        ('ᵍ', 'g'),
        ('ʰ', 'h'),
        ('ⁱ', 'i'),
        ('ʲ', 'j'),
        ('ᵏ', 'k'),
        ('ᵐ', 'm'),
        ('ⁿ', 'n'),
        ('ᵖ', 'p'),
        ('ʳ', 'r'),
        ('ˢ', 's'),
        ('ᵗ', 't'),
        ('ᵘ', 'u'),
        ('ᵛ', 'v'),
        ('ʷ', 'w'),
        ('ˣ', 'x'),
        ('ʸ', 'y'),
        ('ᶻ', 'z'),
    ];
    const RETURN_ALPHABET: &str = "abcdefghijkmnprstuvwxyz";

    /// Inverse of the filter's bijective base-23 `letters`: tag glyphs back
    /// to the 1-based link number.
    fn return_tag_number(tag: &str) -> Option<u32> {
        let mut number = 0u32;
        let mut length = 0;
        for glyph in tag.chars() {
            let (_, letter) = RETURN_GLYPH_LETTERS
                .iter()
                .find(|(glyph_value, _)| *glyph_value == glyph)?;
            let position = RETURN_ALPHABET.find(*letter)? as u32;
            number = number * 23 + position + 1;
            length += 1;
        }
        (length > 0).then_some(number)
    }

    fn resolve_pdf_object(
        document: &lopdf::Document,
        object: &lopdf::Object,
    ) -> Option<lopdf::Object> {
        match object {
            lopdf::Object::Reference(id) => {
                document.get_object(*id).ok().cloned()
            }
            other => Some(other.clone()),
        }
    }

    fn pdf_dictionary(
        document: &lopdf::Document,
        object: &lopdf::Object,
    ) -> Option<lopdf::Dictionary> {
        match resolve_pdf_object(document, object)? {
            lopdf::Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }

    /// Named destinations mapped to 1-based page numbers.
    fn named_dest_pages(
        document: &lopdf::Document,
    ) -> std::collections::BTreeMap<String, u32> {
        let mut dests = std::collections::BTreeMap::new();
        let page_numbers: std::collections::BTreeMap<lopdf::ObjectId, u32> =
            document
                .get_pages()
                .into_iter()
                .map(|(number, id)| (id, number))
                .collect();
        fn dest_page(
            document: &lopdf::Document,
            page_numbers: &std::collections::BTreeMap<lopdf::ObjectId, u32>,
            value: &lopdf::Object,
        ) -> Option<u32> {
            match resolve_pdf_object(document, value)? {
                lopdf::Object::Array(items) => items
                    .first()
                    .and_then(|page| page.as_reference().ok())
                    .and_then(|id| page_numbers.get(&id).copied()),
                lopdf::Object::Dictionary(dictionary) => {
                    let dest = dictionary.get(b"D").ok()?;
                    dest_page(
                        document,
                        page_numbers,
                        &resolve_pdf_object(document, dest)?,
                    )
                }
                _ => None,
            }
        }
        let Ok(catalog) = document.catalog() else {
            return dests;
        };
        let Some(names) = catalog
            .get(b"Names")
            .ok()
            .and_then(|object| pdf_dictionary(document, object))
        else {
            return dests;
        };
        let Some(tree) = names
            .get(b"Dests")
            .ok()
            .and_then(|object| pdf_dictionary(document, object))
        else {
            return dests;
        };
        // The xelatex name tree may nest intermediate `/Kids` nodes above
        // the leaf `/Names` arrays; walk the whole tree.
        fn collect(
            document: &lopdf::Document,
            page_numbers: &std::collections::BTreeMap<lopdf::ObjectId, u32>,
            node: &lopdf::Dictionary,
            dests: &mut std::collections::BTreeMap<String, u32>,
        ) {
            if let Some(lopdf::Object::Array(entries)) = node
                .get(b"Names")
                .ok()
                .and_then(|object| resolve_pdf_object(document, object))
            {
                let mut pairs = entries.iter();
                while let (Some(key), Some(value)) =
                    (pairs.next(), pairs.next())
                {
                    let lopdf::Object::String(name_bytes, _) = key else {
                        continue;
                    };
                    let name = String::from_utf8_lossy(name_bytes).into_owned();
                    if let Some(page) = dest_page(document, page_numbers, value)
                    {
                        dests.insert(name, page);
                    }
                }
            }
            if let Some(lopdf::Object::Array(kids)) = node
                .get(b"Kids")
                .ok()
                .and_then(|object| resolve_pdf_object(document, object))
            {
                for kid in &kids {
                    if let Some(child) = pdf_dictionary(document, kid) {
                        collect(document, page_numbers, &child, dests);
                    }
                }
            }
        }
        collect(document, &page_numbers, &tree, &mut dests);
        dests
    }

    /// Every GoTo link destination name in the document.
    fn goto_dest_names(document: &lopdf::Document) -> Vec<String> {
        let mut out = Vec::new();
        for (_, page_id) in document.get_pages() {
            let Ok(page) = document.get_dictionary(page_id) else {
                continue;
            };
            let Some(lopdf::Object::Array(annots)) = page
                .get(b"Annots")
                .ok()
                .and_then(|object| resolve_pdf_object(document, object))
            else {
                continue;
            };
            for annot in &annots {
                let Some(dictionary) = pdf_dictionary(document, annot) else {
                    continue;
                };
                let is_link = matches!(
                    dictionary.get(b"Subtype"),
                    Ok(lopdf::Object::Name(name)) if name == b"Link"
                );
                if !is_link {
                    continue;
                }
                let dest = dictionary
                    .get(b"A")
                    .ok()
                    .and_then(|action| pdf_dictionary(document, action))
                    .and_then(|action| action.get(b"D").ok().cloned())
                    .or_else(|| dictionary.get(b"Dest").ok().cloned());
                if let Some(dest) = dest
                    && let Some(lopdf::Object::String(bytes, _)) =
                        resolve_pdf_object(document, &dest)
                {
                    out.push(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
        }
        out
    }

    /// Every outline (bookmark) title in reading order.
    fn outline_titles(document: &lopdf::Document) -> Vec<String> {
        let mut titles = Vec::new();
        fn walk(
            document: &lopdf::Document,
            dictionary: &lopdf::Dictionary,
            titles: &mut Vec<String>,
        ) {
            let mut next = dictionary
                .get(b"First")
                .ok()
                .and_then(|object| object.as_reference().ok());
            while let Some(id) = next {
                let Ok(lopdf::Object::Dictionary(item)) =
                    document.get_object(id)
                else {
                    break;
                };
                if let Ok(title) = item.get(b"Title")
                    && let Ok(text) = lopdf::decode_text_string(title)
                {
                    titles.push(text);
                }
                walk(document, item, titles);
                next = item
                    .get(b"Next")
                    .ok()
                    .and_then(|object| object.as_reference().ok());
            }
        }
        if let Ok(catalog) = document.catalog()
            && let Some(outlines) = catalog
                .get(b"Outlines")
                .ok()
                .and_then(|object| pdf_dictionary(document, object))
        {
            walk(document, &outlines, &mut titles);
        }
        titles
    }

    /// `(printed page, tag glyphs)` for every `↩ p. N<tag>` pill on the page.
    fn page_pills(text: &str) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(position) = rest.find('↩') {
            let after = &rest[position + '↩'.len_utf8()..];
            rest = after;
            let mut tail = after.trim_start();
            if let Some(stripped) = tail.strip_prefix("p.") {
                tail = stripped.trim_start();
                let digits: String = tail
                    .chars()
                    .take_while(|char| char.is_ascii_digit())
                    .collect();
                if !digits.is_empty() {
                    tail = tail[digits.len()..].trim_start();
                    let tags: String = tail
                        .chars()
                        .take_while(|char| RETURN_TAG_GLYPHS.contains(*char))
                        .collect();
                    if !tags.is_empty()
                        && let Ok(page) = digits.parse()
                    {
                        out.push((page, tags));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn return_link_xelatex_render_pairs_pills_with_destinations() {
        if pandoc_command().is_none()
            || Command::new("xelatex").arg("--version").output().is_err()
        {
            eprintln!(
                "skipping return-link PDF test: pandoc and xelatex are required"
            );
            return;
        }
        let temp = TempDir::new("return-links-pdf");
        let mut source = "# Return Link Exercise\n\nA [solo link](#solo) opens the exercise and a [near-bottom link](#nearbottom) targets the landing.\n\n## Solo\n\nOne inbound link lands here.\n\n## Single\n\nThe [only link](#single) points here.\n\n## Lonely\n\nNobody links here.\n\n## Crowd\n\n".to_string();
        source.push_str(
            &["A", "B", "C", "D", "E", "F", "G", "H"]
                .iter()
                .map(|name| format!("[Fan {name}](#crowd)"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        source.push_str("\n\n## Wrap\n\n");
        source.push_str(
            &(1..=21)
                .map(|number| format!("[r{number}](#wrap)"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        source.push_str("\n\n#### Deep dive {#deep}\n\nA [deep link](#deep) plus [this is a deliberately long link label that should wrap across more than one typeset line](#deep).\n\n::: {#mydiv}\n\nDiv target body.\n\n:::\n\nA [div link](#mydiv).\n\nA [span link](#myspan) plus [target words]{#myspan} inline.\n");
        for number in 0..30 {
            source.push_str(&format!(
                "\nFiller paragraph {number} pads the exercise so return pills span several pages and page labels vary.\n"
            ));
        }
        source.push_str("\n## Near bottom landing {#nearbottom}\n\n| cell one | cell two |\n|----------|----------|\n| r1c1 | r1c2 |\n| r2c1 | r2c2 |\n");
        let source_path = temp.path.join("exercise.md");
        fs::write(&source_path, &source).expect("write source");
        let target = temp.path.join("xlib/chat/exercise.pdf");
        fs::create_dir_all(target.parent().expect("target parent"))
            .expect("create output directory");
        let plan = CreatePlan {
            source: source_path.display().to_string(),
            source_kind: "markdown",
            target: target.clone(),
            sidecar: target.with_extension("md"),
            workflow: TargetWorkflow::Intake {
                library_destination: temp.path.join("lib/chat/exercise.pdf"),
            },
            stem: "exercise".to_string(),
            renamed_from: None,
            title: "Return Link Exercise".to_string(),
            title_source: "markdown",
            author: None,
            published: None,
            source_url: None,
            source_note: None,
            source_markdown: Some(source.clone()),
            captured: None,
            id: None,
            marker: compose_marker(
                "ready",
                "obsidian_ref",
                "Return Link Exercise",
                None,
                &[],
            )
            .expect("compose marker"),
            audio: None,
            page_count_hint: None,
        };
        let render_path = render_temp_path(&target).expect("render path");
        let code_break_path = temp.path.join("filter.lua");
        fs::write(&code_break_path, PANDOC_CODE_BREAK_FILTER)
            .expect("write filter");
        let return_filter_path = temp.path.join("return-links.lua");
        fs::write(&return_filter_path, super::super::return_links::FILTER)
            .expect("write return-link filter");
        let filters = RenderFilters {
            code_break: code_break_path,
            return_links: return_filter_path,
            report: temp.path.join("return-links.json"),
            diagrams: None,
        };
        let pandoc = pandoc_command().expect("pandoc checked above");
        let outcome =
            render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
                .expect("render return-link PDF");
        let super::super::return_links::ReportOutcome::Report(report) = outcome
        else {
            panic!("render must write a return-link report: {outcome:?}");
        };
        assert_eq!(report.paired, 36);
        assert_eq!(report.targets, 8);
        assert_eq!(report.prefix, "bob:ret:");
        let page_count = stamp_and_install(
            &render_path,
            &plan.target,
            &plan.marker,
            &PdfInfo::default(),
        )
        .expect("install return-link PDF");
        assert!(page_count > 1, "fixture must span pages");

        let document =
            lopdf::Document::load(&target).expect("load rendered PDF");
        let dests = named_dest_pages(&document);
        let prefixed = dests
            .keys()
            .filter(|name| name.starts_with(&report.prefix))
            .count();
        assert_eq!(
            prefixed, report.paired as usize,
            "one named destination per tagged link"
        );
        for name in goto_dest_names(&document) {
            if let Some(target) = name.strip_prefix(&report.prefix) {
                assert!(
                    dests.contains_key(&name),
                    "return link {target} resolves to a named destination"
                );
            }
        }
        let titles = outline_titles(&document);
        assert!(
            !titles.is_empty(),
            "rendered PDF must carry outline bookmarks"
        );
        for title in &titles {
            assert!(
                !title.contains('↩')
                    && !title
                        .chars()
                        .any(|char| RETURN_TAG_GLYPHS.contains(char)),
                "outline title stays clean: {title}"
            );
        }

        if Command::new("pdftotext").arg("-v").output().is_err() {
            eprintln!(
                "skipping return-link page checks: pdftotext is required"
            );
            return;
        }
        let mut pill_count = 0;
        let mut heading_page = 0;
        let mut cell_page = 0;
        for page in 1..=page_count {
            let output = Command::new("pdftotext")
                .arg("-layout")
                .arg("-f")
                .arg(page.to_string())
                .arg("-l")
                .arg(page.to_string())
                .arg(&target)
                .arg("-")
                .output()
                .expect("run pdftotext");
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            for (printed, tag) in page_pills(&text) {
                let number = return_tag_number(&tag)
                    .expect("pill tag decodes to a link number");
                let anchor = format!("{}{number}", report.prefix);
                assert_eq!(
                    dests.get(&anchor),
                    Some(&printed),
                    "pill {tag} prints the page its destination sits on"
                );
                pill_count += 1;
            }
            if text.contains("Near bottom landing") {
                heading_page = page;
            }
            if text.contains("cell one") {
                cell_page = page;
            }
        }
        assert_eq!(
            pill_count, report.paired as usize,
            "every pill is accounted for"
        );
        assert_ne!(heading_page, 0, "heading text must render");
        assert_eq!(
            heading_page, cell_page,
            "near-bottom heading stays with its table"
        );
    }

    #[test]
    fn generic_diagram_text_box_matches_letter_minus_085in_margins() {
        assert!((GENERIC_DIAGRAM_TEXT_WIDTH_PT - 489.6).abs() < 1e-9);
        assert!((GENERIC_DIAGRAM_TEXT_HEIGHT_PT - 669.6).abs() < 1e-9);
    }

    fn skip_pandoc_xelatex(test_name: &str) -> bool {
        if pandoc_command().is_none()
            || Command::new("xelatex").arg("--version").output().is_err()
            || Command::new("pdftotext").arg("-v").output().is_err()
        {
            eprintln!(
                "skipping {test_name}: pandoc/xelatex/pdftotext are required"
            );
            return true;
        }
        false
    }

    const MERMAID_FLOW: &str = "flowchart LR\n  A --> B\n";

    fn mermaid_markdown() -> String {
        format!("# Diagrams\n\n``` {{#fig-flow .mermaid}}\n{MERMAID_FLOW}```\n")
    }

    fn mermaid_plan(temp: &TempDir, source: &Path) -> CreatePlan {
        let target = temp.path.join("xlib/chat/diagrams.pdf");
        fs::create_dir_all(target.parent().expect("target parent"))
            .expect("create output directory");
        CreatePlan {
            source: source.display().to_string(),
            source_kind: "markdown",
            target: target.clone(),
            sidecar: target.with_extension("md"),
            workflow: TargetWorkflow::Intake {
                library_destination: temp.path.join("lib/chat/diagrams.pdf"),
            },
            stem: "diagrams".to_string(),
            renamed_from: None,
            title: "Diagrams".to_string(),
            title_source: "markdown",
            author: None,
            published: None,
            source_url: None,
            source_note: None,
            source_markdown: Some(mermaid_markdown()),
            captured: None,
            id: None,
            marker: compose_marker(
                "ready",
                "obsidian_ref",
                "Diagrams",
                None,
                &[],
            )
            .expect("compose marker"),
            audio: None,
            page_count_hint: None,
        }
    }

    fn filters_with_optional_diagrams(
        dir: &Path,
        diagrams: Option<super::super::diagrams::DiagramFilter>,
    ) -> RenderFilters {
        let code_break = dir.join("filter.lua");
        fs::write(&code_break, PANDOC_CODE_BREAK_FILTER).expect("write filter");
        let return_links = dir.join("return-links.lua");
        fs::write(&return_links, super::super::return_links::FILTER)
            .expect("write return-link filter");
        RenderFilters {
            code_break,
            return_links,
            report: dir.join("return-links.json"),
            diagrams,
        }
    }

    fn pdf_haystack(path: &Path) -> String {
        // Callers gate on `skip_pandoc_xelatex`, which requires pdftotext.
        // There is no raw-bytes fallback: searching compressed PDF bytes for
        // visible text is not a valid extraction and lets negative assertions
        // pass falsely.
        let output = Command::new("pdftotext")
            .arg("-layout")
            .arg(path)
            .arg("-")
            .output()
            .expect("pdftotext is required for PDF text assertions");
        assert!(
            output.status.success(),
            "pdftotext failed on {}: {output:?}",
            path.display()
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn collected_flow_id(
        pandoc: &OsStr,
        source: &Path,
        workdir: &Path,
    ) -> String {
        let blocks = super::super::diagrams::collect(
            pandoc,
            source,
            super::super::diagrams::DiagramReader::Inferred,
            workdir,
        )
        .expect("collect");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        blocks[0].id.clone()
    }

    #[test]
    fn diagrams_swap_pdf_replaces_source_without_figure_caption() {
        if skip_pandoc_xelatex("diagrams swap pdf") {
            return;
        }
        let temp = TempDir::new("diagrams-swap-pdf");
        let source = temp.path.join("doc.md");
        fs::write(&source, mermaid_markdown()).expect("write source");
        let pandoc = pandoc_command().expect("pandoc checked above");
        let id =
            collected_flow_id(&pandoc, &source, &temp.path.join("collect"));
        let pdf = temp.path.join("fig.pdf");
        super::super::diagrams::write_one_page_pdf(&pdf);
        let results_path = temp.path.join("results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::diagrams::DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        super::super::diagrams::write_results(
            &results_path,
            &super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let swap = temp.path.join("diagrams.lua");
        fs::write(&swap, super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        let plan = mermaid_plan(&temp, &source);
        let render_path = render_temp_path(&plan.target).expect("render path");
        let filters = filters_with_optional_diagrams(
            &temp.path,
            Some(super::super::diagrams::DiagramFilter {
                filter: swap,
                results: results_path,
            }),
        );
        render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
            .expect("render swapped PDF");
        let text = pdf_haystack(&render_path);
        assert!(!text.contains("flowchart LR"), "source gone: {text}");
        assert!(!text.contains("Figure 1"), "no implicit caption: {text}");
    }

    #[test]
    fn diagrams_swap_pdf_error_keeps_note_and_source() {
        if skip_pandoc_xelatex("diagrams swap error pdf") {
            return;
        }
        let temp = TempDir::new("diagrams-swap-error-pdf");
        let source = temp.path.join("doc.md");
        fs::write(&source, mermaid_markdown()).expect("write source");
        let pandoc = pandoc_command().expect("pandoc checked above");
        let id =
            collected_flow_id(&pandoc, &source, &temp.path.join("collect"));
        let results_path = temp.path.join("results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::diagrams::DiagramOutcome::Error {
                error: "Parse error on line 4: bad".to_string(),
            },
        );
        super::super::diagrams::write_results(
            &results_path,
            &super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let swap = temp.path.join("diagrams.lua");
        fs::write(&swap, super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        let plan = mermaid_plan(&temp, &source);
        let render_path = render_temp_path(&plan.target).expect("render path");
        let filters = filters_with_optional_diagrams(
            &temp.path,
            Some(super::super::diagrams::DiagramFilter {
                filter: swap,
                results: results_path,
            }),
        );
        render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
            .expect("render fallback PDF");
        let text = pdf_haystack(&render_path);
        assert!(
            text.contains(
                "Mermaid diagram not rendered: Parse error on line 4"
            ),
            "{text}"
        );
        assert!(text.contains("flowchart LR"), "source kept: {text}");
    }

    #[test]
    fn diagrams_absent_results_match_unhooked_pdf() {
        if skip_pandoc_xelatex("diagrams corrupt pdf") {
            return;
        }
        let temp = TempDir::new("diagrams-corrupt-pdf");
        let source = temp.path.join("doc.md");
        fs::write(&source, mermaid_markdown()).expect("write source");
        let pandoc = pandoc_command().expect("pandoc checked above");
        let plan = mermaid_plan(&temp, &source);
        let baseline_path = temp.path.join("baseline.pdf");
        let hooked_path = temp.path.join("hooked.pdf");
        let swap = temp.path.join("diagrams.lua");
        fs::write(&swap, super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        render_temp_pdf(
            &pandoc,
            &plan,
            &baseline_path,
            &filters_with_optional_diagrams(&temp.path, None),
            None,
        )
        .expect("baseline");
        render_temp_pdf(
            &pandoc,
            &plan,
            &hooked_path,
            &filters_with_optional_diagrams(
                &temp.path,
                Some(super::super::diagrams::DiagramFilter {
                    filter: swap,
                    results: temp.path.join("missing.json"),
                }),
            ),
            None,
        )
        .expect("hooked missing results");
        assert_eq!(pdf_haystack(&baseline_path), pdf_haystack(&hooked_path));
    }

    fn write_executable(path: &Path, body: &str) {
        // Shared fork-safe scratch-then-copy writer (bead `bob-cli-5l`).
        crate::native::test_stub::write_executable_stub(path, body);
    }

    const TALL_WIDTH_PT: f64 = 514.5;
    const TALL_HEIGHT_PT: f64 = 714.75;
    const TALL_FLOW: &str = "flowchart TB\n  A --> B\n  B --> C\n";

    fn tall_heading_source() -> String {
        format!(
            "# Tall Diagrams\n\nA [model](#data-model-and-ownership) opens the exercise.\n\n### Data model and ownership\n\n```mermaid\n{TALL_FLOW}```\n\nCaption paragraph that follows the diagram and stays with it.\n"
        )
    }

    fn tall_heading_opt_out_source() -> String {
        format!(
            "---\nbob-return-links: false\n---\n\n# Tall Diagrams\n\nA [model](#data-model-and-ownership) opens the exercise.\n\n### Data model and ownership\n\n```mermaid\n{TALL_FLOW}```\n\nCaption paragraph that follows the diagram and stays with it.\n"
        )
    }

    fn render_tall_pdf(
        temp: &TempDir,
        source_text: &str,
        name: &str,
    ) -> (PathBuf, CreatePlan) {
        let source = temp.path.join(format!("{name}.md"));
        fs::write(&source, source_text).expect("write tall source");
        let pandoc = pandoc_command().expect("pandoc checked above");
        let id = collected_flow_id(
            &pandoc,
            &source,
            &temp.path.join(format!("{name}-collect")),
        );
        let pdf = temp.path.join(format!("{name}-fig.pdf"));
        super::super::diagrams::write_sized_pdf(
            &pdf,
            TALL_WIDTH_PT,
            TALL_HEIGHT_PT,
        );
        let results_path = temp.path.join(format!("{name}-results.json"));
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::diagrams::DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        super::super::diagrams::write_results(
            &results_path,
            &super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write tall results");
        let swap = temp.path.join(format!("{name}-diagrams.lua"));
        fs::write(&swap, super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        let plan = mermaid_plan(temp, &source);
        let render_path = render_temp_path(&plan.target).expect("render path");
        let filters = filters_with_optional_diagrams(
            &temp.path,
            Some(super::super::diagrams::DiagramFilter {
                filter: swap,
                results: results_path,
            }),
        );
        render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
            .expect("render tall PDF");
        (render_path, plan)
    }

    fn page_text(pdf: &Path, page: u32) -> String {
        let output = Command::new("pdftotext")
            .arg("-layout")
            .arg("-f")
            .arg(page.to_string())
            .arg("-l")
            .arg(page.to_string())
            .arg(pdf)
            .arg("-")
            .output()
            .expect("run pdftotext");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn page_count(pdf: &Path) -> u32 {
        let document = lopdf::Document::load(pdf).expect("load rendered PDF");
        document.get_pages().len() as u32
    }

    fn is_body_empty(text: &str) -> bool {
        let kept: String = text
            .split_whitespace()
            .filter(|token| !token.chars().all(|c| c.is_ascii_digit()))
            .collect::<Vec<_>>()
            .join(" ");
        kept.trim().is_empty()
    }

    #[derive(Debug, Clone)]
    struct BboxWord {
        text: String,
        y_min: f64,
        y_max: f64,
    }

    fn bbox_words(pdf: &Path, page: u32) -> Vec<BboxWord> {
        let output = Command::new("pdftotext")
            .arg("-bbox")
            .arg("-f")
            .arg(page.to_string())
            .arg("-l")
            .arg(page.to_string())
            .arg(pdf)
            .arg("-")
            .output()
            .expect("run pdftotext -bbox");
        assert!(output.status.success(), "{output:?}");
        let xml = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut out = Vec::new();
        let mut rest = xml.as_str();
        while let Some(start) = rest.find("<word ") {
            rest = &rest[start + "<word ".len()..];
            let Some(end_tag) = rest.find('>') else {
                break;
            };
            let attrs = &rest[..end_tag];
            rest = &rest[end_tag + 1..];
            let Some(close) = rest.find("</word>") else {
                break;
            };
            let word_text = rest[..close].to_string();
            rest = &rest[close + "</word>".len()..];
            let mut y_min = None;
            let mut y_max = None;
            for attr in attrs.split_whitespace() {
                if let Some(value) = attr.strip_prefix("yMin=\"") {
                    y_min = value
                        .strip_suffix('"')
                        .and_then(|v| v.parse::<f64>().ok());
                } else if let Some(value) = attr.strip_prefix("yMax=\"") {
                    y_max = value
                        .strip_suffix('"')
                        .and_then(|v| v.parse::<f64>().ok());
                }
            }
            if let (Some(y_min), Some(y_max)) = (y_min, y_max) {
                out.push(BboxWord {
                    text: word_text,
                    y_min,
                    y_max,
                });
            }
        }
        out
    }

    fn find_word<'a>(words: &'a [BboxWord], needle: &str) -> &'a BboxWord {
        words
            .iter()
            .find(|word| word.text == needle)
            .unwrap_or_else(|| panic!("word {needle} missing: {words:?}"))
    }

    #[test]
    fn tall_diagram_shares_page_with_heading_and_stays_in_margin() {
        if skip_pandoc_xelatex("tall diagram fit") {
            return;
        }
        let temp = TempDir::new("tall-fit");
        let (render_path, _) =
            render_tall_pdf(&temp, &tall_heading_source(), "tall");
        let pages = page_count(&render_path);
        assert!(pages >= 2, "fixture must span pages: {pages}");
        for page in 1..=pages {
            let text = page_text(&render_path, page);
            assert!(
                !is_body_empty(&text),
                "page {page} must not be body-empty: {text:?}"
            );
        }
        let mut heading_page = 0;
        let mut pill_page = 0;
        let mut top_page = 0;
        let mut bottom_page = 0;
        for page in 1..=pages {
            let text = page_text(&render_path, page);
            if text.contains("Data model") {
                heading_page = page;
            }
            if text.contains('↩') {
                if pill_page == 0 {
                    pill_page = page;
                }
            }
            if text.contains("diagramtop") {
                top_page = page;
            }
            if text.contains("diagrambottom") {
                bottom_page = page;
            }
        }
        assert_ne!(heading_page, 0, "heading must render");
        assert_ne!(pill_page, 0, "return pill must render");
        assert_ne!(top_page, 0, "diagramtop must render");
        assert_ne!(bottom_page, 0, "diagrambottom must render");
        assert_eq!(heading_page, pill_page, "heading and pill share a page");
        assert_eq!(
            heading_page, top_page,
            "heading and diagram top share a page"
        );
        assert_eq!(
            heading_page, bottom_page,
            "heading and diagram bottom share a page"
        );
        let words = bbox_words(&render_path, heading_page);
        let top = find_word(&words, "diagramtop");
        let bottom = find_word(&words, "diagrambottom");
        let margin_line = LETTER_HEIGHT_PT - GENERIC_MARGIN_IN * PT_PER_IN;
        assert!(
            bottom.y_max <= margin_line + 1.0,
            "diagrambottom above the margin line ({}): yMax={}",
            margin_line,
            bottom.y_max
        );
        let rendered_gap = (top.y_min - bottom.y_min).abs();
        let fixture_gap = TALL_HEIGHT_PT - 25.0;
        let expected = fixture_gap
            * super::super::diagrams::FIT_HEIGHT_BUDGET
            * GENERIC_DIAGRAM_TEXT_HEIGHT_PT
            / TALL_HEIGHT_PT;
        assert!(
            (rendered_gap - expected).abs() < 8.0,
            "diagram scaled to the budget: rendered {rendered_gap} vs expected {expected}"
        );
    }

    #[test]
    fn tall_diagram_opt_out_shares_page_without_pills() {
        if skip_pandoc_xelatex("tall diagram opt-out") {
            return;
        }
        let temp = TempDir::new("tall-opt-out");
        let (render_path, _) = render_tall_pdf(
            &temp,
            &tall_heading_opt_out_source(),
            "tall-opt-out",
        );
        let pages = page_count(&render_path);
        for page in 1..=pages {
            let text = page_text(&render_path, page);
            assert!(
                !is_body_empty(&text),
                "page {page} must not be body-empty: {text:?}"
            );
        }
        let mut heading_page = 0;
        let mut top_page = 0;
        let mut bottom_page = 0;
        for page in 1..=pages {
            let text = page_text(&render_path, page);
            if text.contains("Data model") {
                heading_page = page;
            }
            if text.contains("diagramtop") {
                top_page = page;
            }
            if text.contains("diagrambottom") {
                bottom_page = page;
            }
        }
        assert_ne!(heading_page, 0, "heading must render");
        assert_ne!(top_page, 0, "diagramtop must render");
        assert_ne!(bottom_page, 0, "diagrambottom must render");
        assert_eq!(
            heading_page, top_page,
            "heading and diagram top share a page"
        );
        assert_eq!(
            heading_page, bottom_page,
            "heading and diagram bottom share a page"
        );
        let words = bbox_words(&render_path, heading_page);
        let bottom = find_word(&words, "diagrambottom");
        let margin_line = LETTER_HEIGHT_PT - GENERIC_MARGIN_IN * PT_PER_IN;
        assert!(
            bottom.y_max <= margin_line + 1.0,
            "diagrambottom above the margin line ({}): yMax={}",
            margin_line,
            bottom.y_max
        );
    }

    #[test]
    fn tall_anchored_figure_div_shares_page_and_keeps_destinations() {
        if skip_pandoc_xelatex("tall figure div") {
            return;
        }
        let temp = TempDir::new("tall-div");
        let mut source = String::from(
            "# Figure Exercise\n\n[go to figure](#figure-tall) and [again](#figure-tall) open the exercise.\n\n",
        );
        for i in 0..8 {
            source.push_str(&format!(
                "Filler paragraph {i} pads the exercise so the anchored figure starts low on a page and page labels vary across the render.\n\n"
            ));
        }
        source.push_str(&format!(
            "<div id=\"figure-tall\">\n\n```mermaid\n{TALL_FLOW}```\n\nFigure caption inside the anchored div.\n\n</div>\n"
        ));
        let source_path = temp.path.join("div.md");
        fs::write(&source_path, &source).expect("write div source");
        let pandoc = pandoc_command().expect("pandoc checked above");
        let id = collected_flow_id(
            &pandoc,
            &source_path,
            &temp.path.join("div-collect"),
        );
        let pdf = temp.path.join("div-fig.pdf");
        super::super::diagrams::write_sized_pdf(
            &pdf,
            TALL_WIDTH_PT,
            TALL_HEIGHT_PT,
        );
        let results_path = temp.path.join("div-results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::diagrams::DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        super::super::diagrams::write_results(
            &results_path,
            &super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write div results");
        let swap = temp.path.join("div-diagrams.lua");
        fs::write(&swap, super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        let plan = mermaid_plan(&temp, &source_path);
        let render_path = render_temp_path(&plan.target).expect("render path");
        let filters = filters_with_optional_diagrams(
            &temp.path,
            Some(super::super::diagrams::DiagramFilter {
                filter: swap,
                results: results_path,
            }),
        );
        let outcome =
            render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
                .expect("render div PDF");
        let super::super::return_links::ReportOutcome::Report(report) = outcome
        else {
            panic!("div render must write a return-link report: {outcome:?}");
        };
        assert!(report.paired >= 2, "two div links pair: {report:?}");
        let document =
            lopdf::Document::load(&render_path).expect("load div PDF");
        let dests = named_dest_pages(&document);
        for name in goto_dest_names(&document) {
            if let Some(target) = name.strip_prefix(&report.prefix) {
                assert!(
                    dests.contains_key(&name),
                    "return link {target} resolves to a named destination"
                );
            }
        }
        let pages = page_count(&render_path);
        for page in 1..=pages {
            let text = page_text(&render_path, page);
            assert!(
                !is_body_empty(&text),
                "page {page} must not be body-empty: {text:?}"
            );
        }
        let mut pill_page = 0;
        let mut top_page = 0;
        for page in 1..=pages {
            let text = page_text(&render_path, page);
            if text.contains('↩') && pill_page == 0 {
                pill_page = page;
            }
            if text.contains("diagramtop") {
                top_page = page;
            }
        }
        assert_ne!(pill_page, 0, "div pills must render");
        assert_ne!(top_page, 0, "diagramtop must render");
        assert_eq!(pill_page, top_page, "pills and diagramtop share a page");
        let words = bbox_words(&render_path, top_page);
        let bottom = find_word(&words, "diagrambottom");
        let margin_line = LETTER_HEIGHT_PT - GENERIC_MARGIN_IN * PT_PER_IN;
        assert!(
            bottom.y_max <= margin_line + 1.0,
            "diagrambottom above the margin line ({}): yMax={}",
            margin_line,
            bottom.y_max
        );
    }

    #[test]
    fn short_diagram_prints_at_natural_size() {
        if skip_pandoc_xelatex("short diagram natural") {
            return;
        }
        let temp = TempDir::new("short-natural");
        let source = temp.path.join("doc.md");
        fs::write(&source, mermaid_markdown()).expect("write source");
        let pandoc = pandoc_command().expect("pandoc checked above");
        let id =
            collected_flow_id(&pandoc, &source, &temp.path.join("collect"));
        let pdf = temp.path.join("fig.pdf");
        super::super::diagrams::write_one_page_pdf(&pdf);
        let fixture_words = bbox_words(&pdf, 1);
        let fixture = find_word(&fixture_words, "diagram");
        let fixture_height = fixture.y_max - fixture.y_min;
        let results_path = temp.path.join("results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::diagrams::DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        super::super::diagrams::write_results(
            &results_path,
            &super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let swap = temp.path.join("diagrams.lua");
        fs::write(&swap, super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        let plan = mermaid_plan(&temp, &source);
        let render_path = render_temp_path(&plan.target).expect("render path");
        let filters = filters_with_optional_diagrams(
            &temp.path,
            Some(super::super::diagrams::DiagramFilter {
                filter: swap,
                results: results_path,
            }),
        );
        render_temp_pdf(&pandoc, &plan, &render_path, &filters, None)
            .expect("render short PDF");
        let pages = page_count(&render_path);
        let mut found = None;
        for page in 1..=pages {
            let words = bbox_words(&render_path, page);
            if let Some(word) = words.iter().find(|w| w.text == "diagram") {
                found = Some(word.clone());
                break;
            }
        }
        let rendered = found.expect("diagram label must render");
        let rendered_height = rendered.y_max - rendered.y_min;
        assert!(
            (rendered_height - fixture_height).abs() < 2.0,
            "short diagram not scaled: rendered {rendered_height} vs natural {fixture_height}"
        );
    }

    #[test]
    fn diagrams_filter_runs_first_on_generic_route() {
        let temp = TempDir::new("diagrams-generic-first");
        let calls = temp.path.join("calls.log");
        let fake_pandoc = temp.path.join("fake-pandoc");
        write_executable(
            &fake_pandoc,
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do echo \"PANDOC-ARG:$a\" >> \"{}\"; done\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\nprintf '%%PDF-1.4\\n%%EOF\\n' > \"$out\"\nexit 0\n",
                calls.display()
            ),
        );
        let source = temp.path.join("doc.md");
        fs::write(&source, mermaid_markdown()).expect("write source");
        let plan = mermaid_plan(&temp, &source);
        let render_path = temp.path.join("render.pdf");
        let swap = temp.path.join("diagrams.lua");
        fs::write(&swap, "-- swap").expect("write swap");
        let results = temp.path.join("results.json");
        fs::write(&results, "{}").expect("write results");
        let filters = filters_with_optional_diagrams(
            &temp.path,
            Some(super::super::diagrams::DiagramFilter {
                filter: swap,
                results,
            }),
        );
        render_temp_pdf(
            fake_pandoc.as_os_str(),
            &plan,
            &render_path,
            &filters,
            None,
        )
        .expect("fake render");
        let log = fs::read_to_string(&calls).expect("calls log");
        let diagrams = log.find("diagrams.lua").expect("diagrams filter");
        let cb = log.find("filter.lua").expect("code-break");
        let rl = log.find("return-links.lua").expect("return-links");
        assert!(diagrams < cb && cb < rl, "diagrams filter first: {log}");
        assert!(
            log.contains("bob-diagrams-results="),
            "results metadata: {log}"
        );
    }
}

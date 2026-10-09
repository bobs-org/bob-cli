use crate::native::env as bob_env;
use std::{
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

const DEFAULT_PARENT: &str = "obsidian_ref";
const DEFAULT_MARKDOWN_REF_TYPE: &str = "chat";
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
);
/// Pandoc Lua filter that gives long inline code somewhere to break and renders
/// listen-card Divs as a compact LaTeX callout.
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

return {{Meta = Meta}, {Code = Code, Div = Div}}
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateOptions {
    audio: Option<PathBuf>,
    author: Option<String>,
    dry_run: bool,
    force: bool,
    html: Option<OsString>,
    include_id: bool,
    listen: bool,
    name: Option<String>,
    no_audio: bool,
    output: Option<PathBuf>,
    parent: String,
    resolved_parent: Option<crate::native::parent_notes::ResolvedParent>,
    published: Option<String>,
    ref_type: Option<String>,
    status: String,
    title: Option<String>,
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
    captured: Option<String>,
    id: Option<String>,
    marker: String,
    audio: Option<AudioCopyPlan>,
    page_count_hint: Option<usize>,
}

/// Pandoc inputs for the Markdown render: the code-break/listen filter, the
/// return-link filter that runs after it, and the JSON report path the
/// return-link filter writes for bob.
struct RenderFilters {
    code_break: PathBuf,
    return_links: PathBuf,
    report: PathBuf,
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

fn print_renamed_if_needed(plan: &TargetPlan) {
    if let Some(line) = plan.renamed_line() {
        println!("{line}");
    }
}

pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("create")
        .about("Create a Highlights-ready PDF from Markdown, a PDF, or a URL")
        .alias("clip")
        .arg(
            Arg::new("target")
                .value_name("TARGET")
                .required(true)
                .value_parser(clap::builder::OsStringValueParser::new())
                .help("Markdown file, PDF file, PDF URL, arXiv paper URL, or web article URL"),
        )
        .arg(
            Arg::new("author")
                .long("author")
                .short('A')
                .value_name("NAME")
                .help("Override the derived author"),
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
        .arg(
            Arg::new("force")
                .long("force")
                .short('f')
                .action(ArgAction::SetTrue)
                .help("Overwrite an existing target PDF"),
        )
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
                .value_name("STEM")
                .help("Output filename stem [default: derived from TARGET]"),
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
                .help("Complete path for the generated PDF, including the .pdf filename"),
        )
        .arg(
            Arg::new("parent")
                .long("parent")
                .short('P')
                .value_name("NOTE")
                .default_value(DEFAULT_PARENT)
                .help("Bare Obsidian note target for the marker parent"),
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
                .help("Override the derived title"),
        )
        .arg(
            Arg::new("ref-type")
                .long("ref-type")
                .short('t')
                .value_name("DIR")
                .conflicts_with("output")
                .help("Single library subdirectory [default: chat for Markdown, papers for PDFs, blogs for web articles]"),
        )
        .arg(xlib_dir_arg())
        .after_help(
            "Targets:\n  Markdown file (.md) -> rendered with pandoc (default ref type chat)\n  Local PDF file (.pdf or %PDF- magic) -> stamped as-is (default ref type papers)\n  PDF URL (Content-Type PDF or sniffed %PDF-) -> downloaded, stamped as-is (default ref type papers)\n  arXiv paper URL (abs/html/pdf) -> PDF fetched from arxiv.org/pdf/<id>, metadata from the API (default ref type papers)\n  Web article URL (HTML 2xx or 403/429/503) -> captured with the web-article engine into a Highlights-ready PDF (default ref type blogs)\n\nWeb articles:\n  Captures the article in reader mode and re-typesets it with a Bob-owned print template; it never prints the live page. `bob ref scan` later moves the intake PDF into the library and writes the reference note. A site that blocks headless browsers is retried headed automatically: on Linux under a private Xvfb display or in an off-screen window on macOS; hosts with no browser fail closed with a hint. `-H, --html FILE` replays a page saved from a real browser instead of fetching (`-` reads stdin) and forces the web-article route. `-A, --author` and `-p, --published` override the derived author/publish date on every route. Environment: BOB_WEB_CLIP_ADAPTER replaces the adapter invocation; BOB_CHROME selects the browser executable; BOB_WEB_CLIP_TIMEOUT_SECS sets the adapter timeout in seconds (default 300); BOB_WEB_CLIP_KEEP_WORKDIR=1 keeps the scratch directory for debugging.\n\nListen:\n  `-L, --listen` narrates TARGET with `highlights.listen_command` (`BOB_HIGHLIGHTS_LISTEN_COMMAND` overrides), which must write MP3 audio to `{audio}`; bob shell-quotes `{target}`, `{pdf}`, `{audio}`, and `{title}` itself — do not quote them — and streams the command output unchanged. Preflights run first, the PDF is produced in private scratch, then the episode is bound beside the PDF; if the listen command fails nothing is written, and a failure after the episode exists keeps the scratch audio with a `kept:` line. If TARGET is already captured, `--listen` attaches the new episode (`xlib/<rel>.mp3`) for `bob ref scan` to pair, and the PDF and ref note stay untouched.\n\nAudio:\n  Create discovers audio as `--audio PATH`, then frontmatter `audio.episode_id` in the sase-listen library (Markdown only), then a sibling `<stem>_narration.md` hash matched against library manifests (`BOB_HIGHLIGHTS_AUDIO_LIBRARY`, then `highlights.audio_library`, then `$XDG_DATA_HOME/sase-listen/library`, then `~/.local/share/sase-listen/library`); `--no-audio` skips discovery. The copy lands beside the PDF with the source extension lowercased before the PDF is installed, reuses identical bytes, refuses different bytes without `--force`, and refuses when the mirrored library audio already exists. Only a companion already at the target's `<stem>.<ext>` counts as reused; audio beside the source is copied through the same rules.\n\nOutput:\n  Default target `<xlib-dir>/<ref-type>/<stem>.pdf`, becoming `<stem>_2`, `<stem>_3`, … when the name is taken by a different reference; the same reference refuses (`already captured` / `already queued`), and `-o, --output` keeps the exact path. `-o, --output` selects the complete path instead, including the filename (`.pdf` required, `~` expanded, cwd-relative). `--output` cannot be combined with `--ref-type` or `--name` because those only participate in default target derivation. `-N` sets the stem (with `-i`, the marker id); `-T` overrides the title; `-A` and `-p` override the derived author/publish date. Pandoc renders TOC/bookmarks and embeds the page-1 scan marker. Scan moves intake PDFs to the library before writing notes; library PDFs scan directly, other paths need `bob ref sync`. A `<div class=\"listen\">` card becomes a callout with a Play link when audio is bound. Same-document `#` links get a raised letter tag and a matching `↩ p. N` return pill under their target, dead ones render as plain text with a warning, and frontmatter `bob-return-links: false` turns this off.\n\nExamples:\n  bob ref create report.md\n  bob ref create paper.pdf -t papers\n  bob ref create https://example.com/paper.pdf -N my_paper\n  bob ref create https://arxiv.org/abs/1706.03762 -d\n  bob ref create https://arxiv.org/abs/1706.03762 -L\n  bob ref create https://example.com/essay -H saved.html\n  bob ref create https://example.com/essay -A \"Jane Doe\" -p 2026-01-02 -d",
        )
}

pub(super) fn run(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let target_os = matches
        .get_one::<OsString>("target")
        .expect("required by clap")
        .clone();
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
    let parent_input = matches
        .get_one::<String>("parent")
        .expect("defaulted by clap")
        .clone();
    // An explicit -P resolves before any work (pandoc, the browser, the
    // network, or a write) and the marker stores the canonical route. The
    // obsidian_ref default stays unresolved until ref-create-parent.
    let parent_given = matches
        .value_source("parent")
        .is_some_and(|source| source == clap::parser::ValueSource::CommandLine);
    let (parent, resolved_parent) = match parent_given {
        false => (parent_input, None),
        true => {
            match crate::native::parent_notes::resolve_parent(
                &config.bob_dir,
                &parent_input,
            ) {
                Ok(resolved) => (resolved.route.clone(), Some(resolved)),
                Err(error) => {
                    let styler = Styler::detect();
                    let message = error.message();
                    if let Some((first, hint)) = message.split_once("\nhint: ")
                    {
                        eprintln!(
                            "bob ref create: {}: {first}",
                            styler.red("error")
                        );
                        eprintln!("hint: {hint}");
                    } else {
                        eprintln!(
                            "bob ref create: {}: {message}",
                            styler.red("error")
                        );
                    }
                    return 1;
                }
            }
        }
    };
    let options = CreateOptions {
        audio: matches.get_one::<OsString>("audio").map(PathBuf::from),
        author: matches.get_one::<String>("author").cloned(),
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
        html: matches.get_one::<OsString>("html").cloned(),
        include_id: matches.get_flag("include-id"),
        listen: matches.get_flag("listen"),
        name: matches.get_one::<String>("name").cloned(),
        no_audio: matches.get_flag("no-audio"),
        output: matches.get_one::<OsString>("output").map(PathBuf::from),
        parent,
        resolved_parent,
        published: matches.get_one::<String>("published").cloned(),
        ref_type,
        status: matches
            .get_one::<String>("status")
            .expect("defaulted by clap")
            .clone(),
        title: matches.get_one::<String>("title").cloned(),
    };

    match create_pdf(&config, &target_os, &options) {
        Ok(()) => 0,
        Err(error) => {
            let styler = Styler::detect();
            // Errors may carry a `hint:` second line.
            let message = error.message;
            if let Some((first, hint)) = message.split_once("\nhint: ") {
                eprintln!("{COMMAND_NAME}: {}: {first}", styler.red("error"));
                eprintln!("hint: {hint}");
            } else {
                eprintln!("{COMMAND_NAME}: {}: {message}", styler.red("error"));
            }
            error.exit_code.unwrap_or(1)
        }
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
    if target_mod::looks_like_url(target_raw) {
        let (url, arxiv) = target_mod::resolve_url_syntactic(target_raw)?;
        let recorded = sources_mod::collect_recorded_source_urls(config)
            .map_err(|error| CommandError::new(error.message.clone()))?;
        let dedupe_key = match &arxiv {
            Some(paper) => paper.dedupe_key(),
            None => url.dedupe_key.clone(),
        };
        if let Some(command) = &listen
            && let Some(hit) = sources_mod::find_refusing_hit(
                &recorded,
                &dedupe_key,
                None,
                options.force,
            )
        {
            let attach = attach_mod::attach_for_dedupe_hit(config, &hit)?;
            let (source_line, target_value) = url_attach_lines(&url, &arxiv);
            return attach_mod::run_attach(
                config,
                command,
                &attach,
                &source_line,
                &target_value,
                options.dry_run,
                &mut scratch,
            );
        }
        check_dedupe_with_listen_hint(
            &recorded,
            &dedupe_key,
            None,
            options.force,
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
            target_mod::CreateSource::PdfUrl { url, downloaded } => {
                return create_pdf_url_route(
                    config,
                    url,
                    downloaded,
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
    let local_path = Path::new(target_os);
    match target_mod::resolve_local_target(local_path)? {
        target_mod::CreateSource::Markdown(path) => create_markdown_route(
            config,
            &path,
            &mut scratch,
            options,
            listen.as_ref(),
        ),
        target_mod::CreateSource::LocalPdf(path) => create_local_pdf_route(
            config,
            &path,
            &mut scratch,
            options,
            listen.as_ref(),
        ),
        _ => Err(CommandError::new("unexpected target classification")),
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
        name: validated_name,
        output: options.output.clone(),
        parent: options.parent.clone(),
        published: options.published.clone(),
        ref_type,
        status: options.status.clone(),
        title: options.title.clone(),
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
    recorded: &[sources_mod::RecordedSource],
    dedupe_key: &str,
    planned_target: Option<&Path>,
    force: bool,
) -> Result<()> {
    sources_mod::check_dedupe(recorded, dedupe_key, planned_target, force)
        .map_err(|error| {
            let mut message = error.message.clone();
            message.push_str(
                "\nhint: add --listen to narrate it and attach the episode to the existing capture",
            );
            if let Some(hint) = error.hint.clone() {
                message.push_str(&format!(" ({hint})"));
            }
            CommandError::new(message)
        })
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
        _ => DEFAULT_MARKDOWN_REF_TYPE.to_string(),
    })
}

fn plan_target_for(
    config: &Config,
    stem: &str,
    ref_type: &str,
    output: Option<&PathBuf>,
    force: bool,
    identity: &DefaultTargetIdentity<'_>,
) -> Result<TargetPlan> {
    match output {
        Some(output) => plan_exact_output(config, output, force),
        None => plan_default_target(
            config,
            std::ffi::OsStr::new(stem),
            ref_type,
            force,
            identity,
        ),
    }
}

fn create_markdown_route(
    config: &Config,
    source: &Path,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let mut plan = plan_markdown(config, source, options)?;
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
        print_plan(&plan, options, &styler);
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
    let filter_path = scratch.path().join("filter.lua");
    fs::write(&filter_path, PANDOC_CODE_BREAK_FILTER).map_err(|error| {
        CommandError::new(format!(
            "write pandoc filter {}: {error}",
            filter_path.display()
        ))
    })?;
    let return_filter_path = scratch.path().join("return-links.lua");
    fs::write(&return_filter_path, super::return_links::FILTER).map_err(
        |error| {
            CommandError::new(format!(
                "write pandoc filter {}: {error}",
                return_filter_path.display()
            ))
        },
    )?;
    let filters = RenderFilters {
        code_break: filter_path,
        return_links: return_filter_path,
        report: scratch.path().join("return-links.json"),
    };
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
                options.force,
            ) {
                return Err(super::listen::post_listen_error(
                    scratch,
                    &flow.scratch_audio,
                    error.to_string(),
                    bind_create_hint(&plan.source, &flow.scratch_audio),
                ));
            }
            let created =
                super::listen::install_listen_audio(flow, options.force)
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
    print_renamed_if_needed(&plan.target_plan());
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
    print_next_step(&plan.target_plan());
    Ok(())
}

fn create_local_pdf_route(
    config: &Config,
    source: &Path,
    scratch: &mut super::ScratchDir,
    options: &CreateOptions,
    listen: Option<&super::listen::ListenCommand>,
) -> Result<()> {
    let canonical = fs::canonicalize(source).map_err(|error| {
        CommandError::new(format!(
            "resolve PDF file {}: {error}",
            source.display()
        ))
    })?;
    // Attach when the target itself is an existing capture.
    if let Some(command) = listen
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
    let target_plan = plan_target_for(
        config,
        &pdf_plan.stem,
        &ref_type,
        options.output.as_ref(),
        options.force,
        &identity,
    )?;
    let final_stem = if options.output.is_some() {
        pdf_plan.stem.clone()
    } else {
        target_plan.stem.clone()
    };
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
    let id = if options.include_id {
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
                options.force,
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
    print_renamed_if_needed(&target_plan);
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
    print_next_step(&target_plan);
    Ok(())
}

fn create_pdf_url_route(
    config: &Config,
    url: super::clip_url::WebUrl,
    downloaded: PathBuf,
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
    let target_plan = plan_target_for(
        config,
        &pdf_plan.stem,
        &ref_type,
        options.output.as_ref(),
        options.force,
        &identity,
    )?;
    let final_stem = if options.output.is_some() {
        pdf_plan.stem.clone()
    } else {
        target_plan.stem.clone()
    };
    // The stem is only known after the fetch, so a race that captured
    // the URL mid-fetch attaches here instead of refusing.
    if let Some(command) = listen
        && let Some(hit) = sources_mod::find_refusing_hit(
            recorded,
            &url.dedupe_key,
            Some(&target_plan.target),
            options.force,
        )
    {
        let attach = attach_mod::attach_for_dedupe_hit(config, &hit)?;
        return attach_mod::run_attach(
            config,
            command,
            &attach,
            &format!("source: {} (PDF)", url.cleaned),
            &url.cleaned,
            options.dry_run,
            scratch,
        );
    }
    check_dedupe_with_listen_hint(
        recorded,
        &url.dedupe_key,
        Some(&target_plan.target),
        options.force,
    )?;
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
            options.force,
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
    print_renamed_if_needed(&target_plan);
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
    print_next_step(&target_plan);
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
    let target_plan = plan_target_for(
        config,
        &pdf_plan.stem,
        &ref_type,
        options.output.as_ref(),
        options.force,
        &identity,
    )?;
    let final_stem = if options.output.is_some() {
        pdf_plan.stem.clone()
    } else {
        target_plan.stem.clone()
    };
    // Post-capture dedupe with the final target (stem known only now).
    // Also check the user's URL spelling key (same arXiv key).
    // With --listen a hit attaches instead of refusing.
    if let Some(command) = listen {
        let hit = sources_mod::find_refusing_hit(
            recorded,
            &key,
            Some(&target_plan.target),
            options.force,
        )
        .or_else(|| {
            sources_mod::find_refusing_hit(
                recorded,
                &url.dedupe_key,
                Some(&target_plan.target),
                options.force,
            )
        });
        if let Some(hit) = hit {
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
                scratch,
            );
        }
    } else {
        check_dedupe_with_listen_hint(
            recorded,
            &key,
            Some(&target_plan.target),
            options.force,
        )?;
        check_dedupe_with_listen_hint(
            recorded,
            &url.dedupe_key,
            Some(&target_plan.target),
            options.force,
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
    // With --listen `{target}` is the cleaned URL (sase-listen does its
    // own arXiv rewrite) and `{pdf}` is the fetched scratch copy.
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
            options.force,
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
    print_renamed_if_needed(&target_plan);
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
    print_next_step(&target_plan);
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
    if let Some(explicit) = &options.audio {
        return companion_mod::plan_explicit_audio(
            config,
            target_plan,
            options.force,
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
    print_renamed_if_needed(target_plan);
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
    println!("writes: none");
}

#[cfg(test)]
fn plan_create(
    config: &Config,
    source: &Path,
    options: &CreateOptions,
) -> Result<CreatePlan> {
    plan_markdown(config, source, options)
}

fn plan_markdown(
    config: &Config,
    source: &Path,
    options: &CreateOptions,
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
    let target_plan = plan_target_for(
        config,
        &stem,
        &ref_type,
        options.output.as_ref(),
        options.force,
        &identity,
    )?;
    // With -i, the id follows the final (possibly suffixed) stem; the
    // marker is composed after planning for the same reason. For
    // `--output` there is no walk, so the id stays the base stem.
    let id = if options.include_id {
        if options.output.is_some() {
            Some(stem.clone())
        } else {
            Some(target_plan.stem.clone())
        }
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
    return_links: bool,
) -> Result<String> {
    let mut extras: Vec<(&str, MarkerValue)> = Vec::new();
    if let Some(author) = author.filter(|value| !value.is_empty()) {
        extras.push(("author", MarkerValue::String(author.to_string())));
    }
    if let Some(published) = published.filter(|value| !value.is_empty()) {
        extras.push(("published", MarkerValue::String(published.to_string())));
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
        if fs::read_to_string(&plan.source)
            .is_ok_and(|markdown| markdown.contains("class=\"listen\""))
        {
            eprintln!("warning: listen card has no bound companion audio");
        }
        return Ok(None);
    };

    let highlights = super::bob_config::load_highlights_config(
        &super::bob_config::config_path(),
    )
    .map_err(super::config_error)?;
    let template = resolve_audio_link_template(
        bob_env::var_os(ENV_AUDIO_LINK_TEMPLATE).as_deref(),
        highlights.audio_link_template(),
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
    Ok(Some(
        template
            .replace("{vault}", &percent_encode(vault))
            .replace("{path}", &percent_encode(&path)),
    ))
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
    if let Some(explicit) = &options.audio {
        return companion_mod::plan_explicit_audio(
            config,
            target_plan,
            options.force,
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
                options.force,
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
                options.force,
                source,
                "narration sha256".to_string(),
            );
        }
    }
    companion_mod::plan_reused_companion(target_plan, Some(source_md))
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn resolve_audio_link_template(
    env_template: Option<&OsStr>,
    configured: Option<&str>,
) -> String {
    env_template.map_or_else(
        || {
            configured
                .unwrap_or(DEFAULT_AUDIO_LINK_TEMPLATE)
                .to_string()
        },
        |value| value.to_string_lossy().into_owned(),
    )
}

fn render_temp_pdf(
    pandoc: &OsStr,
    plan: &CreatePlan,
    render_path: &Path,
    filters: &RenderFilters,
    audio_uri: Option<&str>,
) -> Result<super::return_links::ReportOutcome> {
    let source_path = Path::new(&plan.source);
    let resource_path = source_path.parent().unwrap_or_else(|| Path::new("."));
    let output = Command::new(pandoc)
        .arg(&plan.source)
        .arg("-o")
        .arg(render_path)
        .arg("--standalone")
        .arg("--toc")
        .arg("--toc-depth=3")
        .arg("--number-sections")
        .arg("--pdf-engine=xelatex")
        .arg(format!("--resource-path={}", resource_path.display()))
        .arg("--highlight-style=tango")
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
        .arg(format!(
            "header-includes={PANDOC_HEADER_INCLUDES}\n{}",
            super::return_links::HEADER_INCLUDES
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
    match &options.resolved_parent {
        Some(resolved) => println!("{}", resolved.dry_run_line()),
        None => println!("parent: {}", options.parent),
    }
}

fn print_plan(plan: &CreatePlan, options: &CreateOptions, styler: &Styler) {
    println!(
        "{} would create Highlights-ready PDF",
        styler.success_prefix(true)
    );
    println!("source: {}", plan.source);
    print_renamed_if_needed(&plan.target_plan());
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
    if !matches!(plan.workflow, TargetWorkflow::Intake { .. }) {
        print_next_step(&plan.target_plan());
    }
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
            dry_run: false,
            force: false,
            html: None,
            include_id: false,
            listen: false,
            name: None,
            no_audio: false,
            output: None,
            parent: DEFAULT_PARENT.to_string(),
            resolved_parent: None,
            published: None,
            ref_type: None,
            status: DEFAULT_STATUS.to_string(),
            title: None,
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
    fn plan_embeds_markdown_stem_id_when_opted_in() {
        let temp = TempDir::new("include-id");
        let source = temp
            .path
            .join("202608/xprompt_role_binding/xprompt_role_binding.md");
        fs::create_dir_all(source.parent().expect("source parent"))
            .expect("create source parent");
        fs::write(&source, "# Xprompt Role Binding\n").expect("write source");
        let mut options = options();
        options.include_id = true;

        let plan =
            plan_create(&config(&temp.path), &source, &options).expect("plan");

        assert_eq!(plan.id.as_deref(), Some("xprompt_role_binding"));
        let marker =
            parse_marker_with_normalization(&plan.marker).expect("marker");
        assert_eq!(
            marker.projection.get(FIELD_ID),
            Some(&MarkerValue::String("xprompt_role_binding".to_string()))
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
        assert_eq!(percent_encode("Research Notes"), "Research%20Notes");
        assert_eq!(
            percent_encode("lib/chat/hello+world.mp3"),
            "lib%2Fchat%2Fhello%2Bworld.mp3"
        );
    }

    #[test]
    fn audio_link_template_precedence_is_env_then_config_then_default() {
        assert_eq!(
            resolve_audio_link_template(
                Some(OsStr::new("custom:{vault}:{path}")),
                Some("configured"),
            ),
            "custom:{vault}:{path}"
        );
        assert_eq!(
            resolve_audio_link_template(None, Some("configured")),
            "configured"
        );
        assert_eq!(
            resolve_audio_link_template(None, None),
            DEFAULT_AUDIO_LINK_TEMPLATE
        );
        assert_eq!(resolve_audio_link_template(Some(OsStr::new("")), None), "");
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
        fs::write(&source_path, source).expect("write source");
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
}

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use sha2::Digest;

use super::{
    atomic_copy, bob_dir_arg, compose_marker, dry_run_arg, lib_dir_arg,
    plan_default_target, plan_exact_output, print_next_step, ref_dir_arg,
    stamp_and_install, xlib_dir_arg, CommandError, Config, PdfInfo, Result,
    TargetPlan, TargetWorkflow,
};
use crate::native::style::Styler;

const DEFAULT_PARENT: &str = "obsidian_ref";
const DEFAULT_REF_TYPE: &str = "chat";
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
const PANDOC_HEADER_INCLUDES: &str = concat!(
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
const PANDOC_CODE_BREAK_FILTER: &str = r#"local SEPARATORS = "[/_%-%.:,]"
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
    dry_run: bool,
    force: bool,
    include_id: bool,
    no_audio: bool,
    output: Option<PathBuf>,
    parent: String,
    ref_type: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AudioCopyPlan {
    source: PathBuf,
    dest: PathBuf,
    library_dest: Option<PathBuf>,
    origin: String,
    reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreatePlan {
    source: PathBuf,
    target: PathBuf,
    sidecar: PathBuf,
    workflow: TargetWorkflow,
    title: String,
    id: Option<String>,
    marker: String,
    audio: Option<AudioCopyPlan>,
}

impl CreatePlan {
    fn target_plan(&self) -> TargetPlan {
        TargetPlan {
            target: self.target.clone(),
            sidecar: self.sidecar.clone(),
            workflow: self.workflow.clone(),
        }
    }
}

pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("create")
        .about("Render Markdown as a Highlights-ready PDF")
        .arg(
            Arg::new("md-file")
                .value_name("MD_FILE")
                .required(true)
                .value_parser(clap::builder::OsStringValueParser::new())
                .help("Markdown file to render"),
        )
        .arg(
            Arg::new("audio")
                .long("audio")
                .short('a')
                .value_name("PATH")
                .value_parser(clap::builder::OsStringValueParser::new())
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
            Arg::new("include-id")
                .long("include-id")
                .short('i')
                .action(ArgAction::SetTrue)
                .help("Embed the Markdown filename without .md as marker id"),
        )
        .arg(lib_dir_arg())
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
            Arg::new("ref-type")
                .long("ref-type")
                .short('t')
                .value_name("DIR")
                .default_value(DEFAULT_REF_TYPE)
                .conflicts_with("output")
                .help("Single library subdirectory for the generated PDF"),
        )
        .arg(xlib_dir_arg())
        .after_help(
            "Examples:\n  bob highlights create report.md\n\nRenders a hyperlinked table of contents and PDF bookmarks with pandoc and embeds the page-1 marker used by `bob highlights scan`. By default the PDF is written to `<xlib-dir>/<ref-type>/<markdown-stem>.pdf`. `-o, --output` selects that complete path instead, including the filename; it requires a `.pdf` extension, expands a leading `~`, and resolves relative paths from the current directory. `--output` cannot be combined with `--ref-type` because `--ref-type` only participates in default target derivation. Scan moves intake PDFs into the library before writing reference notes. A PDF written directly into the library is still found by `bob highlights scan`. A PDF written outside the library and intake directories is not discovered by recursive scan; sync it with `bob highlights sync <PDF>`. A `<div class=\"listen\">` card is rendered as a callout with a Play link when companion audio is bound. Create discovers audio as `--audio PATH`, then frontmatter `audio.episode_id` in the sase-listen library, then a sibling `<stem>_narration.md` hash matched against library manifests (`BOB_HIGHLIGHTS_AUDIO_LIBRARY`, then `highlights.audio_library`, then `$XDG_DATA_HOME/sase-listen/library`, then `~/.local/share/sase-listen/library`); `--no-audio` skips discovery. The copy lands beside the PDF with the source extension lowercased before the PDF is installed, reuses identical bytes, refuses different bytes without `--force`, and refuses when the mirrored library audio already exists.",
        )
}

pub(super) fn run(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let source = PathBuf::from(
        matches
            .get_one::<OsString>("md-file")
            .expect("required by clap"),
    );
    let options = CreateOptions {
        audio: matches.get_one::<OsString>("audio").map(PathBuf::from),
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
        include_id: matches.get_flag("include-id"),
        no_audio: matches.get_flag("no-audio"),
        output: matches.get_one::<OsString>("output").map(PathBuf::from),
        parent: matches
            .get_one::<String>("parent")
            .expect("defaulted by clap")
            .clone(),
        ref_type: matches
            .get_one::<String>("ref-type")
            .expect("defaulted by clap")
            .clone(),
        status: matches
            .get_one::<String>("status")
            .expect("defaulted by clap")
            .clone(),
    };

    match create_pdf(&config, &source, &options) {
        Ok(()) => 0,
        Err(error) => {
            let styler = Styler::detect();
            eprintln!("bob highlights: {}: {error}", styler.red("error"));
            1
        }
    }
}

pub(super) fn pandoc_command() -> Option<OsString> {
    if let Some(command) =
        env::var_os(ENV_PANDOC_COMMAND).filter(|value| !value.is_empty())
    {
        return Some(command);
    }

    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join("pandoc"))
        .find(|candidate| candidate.is_file())
        .map(PathBuf::into_os_string)
}

fn create_pdf(
    config: &Config,
    source: &Path,
    options: &CreateOptions,
) -> Result<()> {
    let plan = plan_create(config, source, options)?;
    let styler = Styler::detect();

    if options.dry_run {
        print_plan(&plan, options, &styler);
        println!("writes: none");
        return Ok(());
    }

    let pandoc = pandoc_command().ok_or_else(|| {
        CommandError::new(format!(
            "pandoc command not found; install pandoc or set {ENV_PANDOC_COMMAND}"
        ))
    })?;
    let parent = plan.target.parent().ok_or_else(|| {
        CommandError::new(format!(
            "target has no parent directory: {}",
            plan.target.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CommandError::new(format!(
            "create output directory {}: {error}",
            parent.display()
        ))
    })?;

    let render_path = render_temp_path(&plan.target)?;
    let filter_path = code_break_filter_path();
    fs::write(&filter_path, PANDOC_CODE_BREAK_FILTER).map_err(|error| {
        CommandError::new(format!(
            "write pandoc filter {}: {error}",
            filter_path.display()
        ))
    })?;
    let _ = fs::remove_file(&render_path);
    let audio_uri = play_uri(config, &plan)?;
    let render_result = render_temp_pdf(
        &pandoc,
        &plan,
        &render_path,
        &filter_path,
        audio_uri.as_deref(),
    );
    let mut audio_created: Option<PathBuf> = None;
    if render_result.is_ok()
        && let Some(audio) = &plan.audio
        && !audio.reused
    {
        match atomic_copy(&audio.source, &audio.dest) {
            Ok(()) => audio_created = Some(audio.dest.clone()),
            Err(error) => {
                let _ = fs::remove_file(&render_path);
                let _ = fs::remove_file(&filter_path);
                return Err(error);
            }
        }
    }
    let install_result = render_result.and_then(|()| {
        stamp_and_install(
            &render_path,
            &plan.target,
            &plan.marker,
            &PdfInfo::default(),
        )
    });
    let _ = fs::remove_file(&render_path);
    let _ = fs::remove_file(&filter_path);
    let page_count = match install_result {
        Ok(page_count) => page_count,
        Err(error) => {
            if let Some(created) = audio_created {
                let _ = fs::remove_file(created);
            }
            return Err(error);
        }
    };

    println!(
        "{} created Highlights-ready PDF",
        styler.success_prefix(false)
    );
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
    print_next_step(&plan.target_plan());
    Ok(())
}

fn plan_create(
    config: &Config,
    source: &Path,
    options: &CreateOptions,
) -> Result<CreatePlan> {
    validate_markdown_path(source)?;
    let source = fs::canonicalize(source).map_err(|error| {
        CommandError::new(format!(
            "resolve Markdown file {}: {error}",
            source.display()
        ))
    })?;
    let id = options
        .include_id
        .then(|| derive_marker_id(&source))
        .transpose()?;
    let markdown = fs::read_to_string(&source).map_err(|error| {
        CommandError::new(format!(
            "read Markdown file {} as UTF-8: {error}",
            source.display()
        ))
    })?;
    let title = extract_title(&markdown, &source)?;
    let marker = compose_marker(
        &options.status,
        &options.parent,
        &title,
        id.as_deref(),
        &[],
    )?;
    let target_plan = match &options.output {
        Some(output) => plan_exact_output(config, output, options.force)?,
        None => {
            let stem = source.file_stem().ok_or_else(|| {
                CommandError::new(format!(
                    "Markdown file has no file stem: {}",
                    source.display()
                ))
            })?;
            plan_default_target(config, stem, &options.ref_type, options.force)?
        }
    };

    let audio =
        plan_audio_copy(config, &source, &markdown, &target_plan, options)?;

    Ok(CreatePlan {
        source,
        target: target_plan.target,
        sidecar: target_plan.sidecar,
        workflow: target_plan.workflow,
        title,
        id,
        marker,
        audio,
    })
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

fn render_temp_path(target: &Path) -> Result<PathBuf> {
    let stem = target.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "target has no file stem: {}",
            target.display()
        ))
    })?;
    let mut name = OsString::from(".");
    name.push(stem);
    name.push(format!(".{}.render.pdf", process::id()));
    Ok(target.with_file_name(name))
}

fn code_break_filter_path() -> PathBuf {
    env::temp_dir().join(format!("bob-highlights-create.{}.lua", process::id()))
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
        env::var_os(ENV_AUDIO_LINK_TEMPLATE).as_deref(),
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
    if options.no_audio {
        if options.audio.is_some() {
            return Err(CommandError::new(
                "--no-audio cannot be used with --audio",
            ));
        }
        return Ok(None);
    }
    if let Some(explicit) = &options.audio {
        let resolved = resolve_audio_arg_path(explicit)?;
        let source = super::audio::validate_explicit_audio(&resolved)
            .map(|path| fs::canonicalize(&path).unwrap_or(path))?;
        return plan_audio_copy_for_source(
            config,
            target_plan,
            options.force,
            source,
            "--audio".to_string(),
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
            return plan_audio_copy_for_source(
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
            return plan_audio_copy_for_source(
                config,
                target_plan,
                options.force,
                source,
                "narration sha256".to_string(),
            );
        }
    }
    if let Some(existing) = super::audio::audio_beside(&target_plan.target)
        .or_else(|| super::audio::audio_beside(source_md))
    {
        let extension = existing
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_lowercase();
        let dest = target_plan.target.with_extension(&extension);
        let library_dest = match &target_plan.workflow {
            TargetWorkflow::Intake {
                library_destination,
            } => Some(library_destination.with_extension(&extension)),
            _ => None,
        };
        if let Some(library_dest) = &library_dest
            && library_dest.exists()
            && existing != *library_dest
        {
            return Err(CommandError::new(format!(
                "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob highlights scan would refuse to move the new audio over it)",
                dest.display(),
                library_dest.display()
            )));
        }
        return Ok(Some(AudioCopyPlan {
            source: existing.clone(),
            dest,
            library_dest,
            origin: "existing companion".to_string(),
            reused: true,
        }));
    }
    Ok(None)
}

fn plan_audio_copy_for_source(
    config: &Config,
    target_plan: &TargetPlan,
    force: bool,
    source: PathBuf,
    origin: String,
) -> Result<Option<AudioCopyPlan>> {
    let _ = config;
    let extension = source
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_lowercase();
    if extension.is_empty() {
        return Err(CommandError::new(format!(
            "audio file has no extension: {}",
            source.display()
        )));
    }
    let dest = target_plan.target.with_extension(&extension);
    let library_dest = match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => Some(library_destination.with_extension(&extension)),
        _ => None,
    };
    if let Some(library_dest) = &library_dest
        && library_dest.exists()
    {
        return Err(CommandError::new(format!(
            "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob highlights scan would refuse to move the new audio over it)",
            dest.display(),
            library_dest.display()
        )));
    }
    if dest.exists() {
        if files_have_identical_bytes(&source, &dest)? {
            return Ok(Some(AudioCopyPlan {
                source,
                dest,
                library_dest,
                origin,
                reused: true,
            }));
        }
        if !force {
            return Err(CommandError::new(format!(
                "target audio already exists: {}; pass --force to overwrite it",
                dest.display()
            )));
        }
        return Ok(Some(AudioCopyPlan {
            source,
            dest,
            library_dest,
            origin,
            reused: false,
        }));
    }
    Ok(Some(AudioCopyPlan {
        source,
        dest,
        library_dest,
        origin,
        reused: false,
    }))
}

fn resolve_audio_arg_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(CommandError::new(
            "audio path must include a nonempty filename",
        ));
    }
    let expanded = super::super::env::expand_tilde(path);
    if expanded.is_absolute() {
        return Ok(expanded);
    }
    let cwd = env::current_dir().map_err(|error| {
        CommandError::new(format!("resolve current directory: {error}"))
    })?;
    Ok(cwd.join(expanded))
}

fn files_have_identical_bytes(source: &Path, dest: &Path) -> Result<bool> {
    if source == dest {
        return Ok(true);
    }
    let source_bytes = fs::read(source).map_err(|error| {
        CommandError::new(format!(
            "read {} for audio comparison: {error}",
            source.display()
        ))
    })?;
    let dest_bytes = fs::read(dest).map_err(|error| {
        CommandError::new(format!(
            "read {} for audio comparison: {error}",
            dest.display()
        ))
    })?;
    Ok(source_bytes == dest_bytes)
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
    filter_path: &Path,
    audio_uri: Option<&str>,
) -> Result<()> {
    let resource_path = plan.source.parent().unwrap_or_else(|| Path::new("."));
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
        .arg(filter_path)
        .arg("-V")
        .arg("colorlinks=true")
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
        .arg(format!("header-includes={PANDOC_HEADER_INCLUDES}"))
        .arg("--metadata")
        .arg(format!("title={}", plan.title))
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
        return Err(CommandError::new(format!(
            "pandoc failed while rendering {} (exit {}):\n{}",
            plan.source.display(),
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            detail
        )));
    }

    Ok(())
}

fn print_plan(plan: &CreatePlan, options: &CreateOptions, styler: &Styler) {
    println!(
        "{} would create Highlights-ready PDF",
        styler.success_prefix(true)
    );
    println!("source: {}", plan.source.display());
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
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
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
                process::id()
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
            dry_run: false,
            force: false,
            include_id: false,
            no_audio: false,
            output: None,
            parent: DEFAULT_PARENT.to_string(),
            ref_type: DEFAULT_REF_TYPE.to_string(),
            status: DEFAULT_STATUS.to_string(),
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
        assert!(
            latex.contains(
                r"obsidian://open?vault=Research\%20Notes\&file=lib\%2Fchat\%2Freport.mp3"
            ),
            "{latex}"
        );
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
            source,
            target: target.clone(),
            sidecar: target.with_extension("md"),
            workflow: TargetWorkflow::Intake {
                library_destination: temp.path.join("lib/chat/report.pdf"),
            },
            title: "Report".to_string(),
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
        };
        let render_path = render_temp_path(&target).expect("render path");
        let filter_path = temp.path.join("listen-card.lua");
        fs::write(&filter_path, PANDOC_CODE_BREAK_FILTER)
            .expect("write filter");
        let pandoc = pandoc_command().expect("pandoc checked above");
        render_temp_pdf(
            &pandoc,
            &plan,
            &render_path,
            &filter_path,
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
}

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
    process::{self, Command, Stdio},
};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use lopdf::{dictionary, Document, Object};

use super::{
    atomic_save_pdf, bob_dir_arg, dry_run_arg, lib_dir_arg,
    parse_marker_with_normalization, pdf_text_string, ref_dir_arg,
    render_marker, validate_marker_parent_value, validate_required_marker_keys,
    CommandError, Config, MarkerValue, Projection, Result, FIELD_ID,
    FIELD_PARENT, FIELD_STATUS,
};
use crate::native::style::Styler;

const DEFAULT_PARENT: &str = "obsidian_ref";
const DEFAULT_REF_TYPE: &str = "chat";
const DEFAULT_STATUS: &str = "ready";
const ENV_PANDOC_COMMAND: &str = "BOB_PANDOC_COMMAND";
/// LaTeX preamble that wraps long code blocks instead of overflowing the page.
///
/// Pandoc emits every fenced block as a `Highlighting` environment, which does
/// not wrap by default, so a single long log or command line silently runs off
/// the right margin.
const PANDOC_HEADER_INCLUDES: &str = concat!(
    r"\usepackage{fvextra}",
    r"\DefineVerbatimEnvironment{Highlighting}{Verbatim}",
    r"{breaklines,breakanywhere,commandchars=\\\{\}}",
);
/// Pandoc Lua filter that gives long inline code somewhere to break.
///
/// Inline code is typeset as an unbreakable `\texttt` box, so paths and
/// identifiers such as `src/sase/ace/tui/modals/plans_detail.py` overflow the
/// text block. Splitting the span after each separator lets pandoc escape every
/// piece normally while LaTeX gains a legal break point between the pieces.
const PANDOC_CODE_BREAK_FILTER: &str = r#"local SEPARATORS = "[/_%-%.:,]"

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
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateOptions {
    dry_run: bool,
    force: bool,
    include_id: bool,
    parent: String,
    ref_type: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreatePlan {
    source: PathBuf,
    target: PathBuf,
    sidecar: PathBuf,
    title: String,
    id: Option<String>,
    marker: String,
}

pub(super) fn command() -> ClapCommand {
    ClapCommand::new("create")
        .about("Render Markdown as a Highlights-ready PDF")
        .arg(
            Arg::new("md-file")
                .value_name("MD_FILE")
                .required(true)
                .value_parser(clap::builder::OsStringValueParser::new())
                .help("Markdown file to render"),
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
                .help("Single library subdirectory for the generated PDF"),
        )
        .after_help(
            "Renders a hyperlinked table of contents and PDF bookmarks with pandoc, then embeds the page-1 marker used by `bob highlights scan`.",
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
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
        include_id: matches.get_flag("include-id"),
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
    let result = render_and_install(&pandoc, &plan, &render_path, &filter_path);
    let _ = fs::remove_file(&render_path);
    let _ = fs::remove_file(&filter_path);
    let page_count = result?;

    println!(
        "{} created Highlights-ready PDF",
        styler.success_prefix(false)
    );
    println!("pdf: {}", plan.target.display());
    println!("title: {}", plan.title);
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    if let Some(id) = &plan.id {
        println!("id: {id}");
    }
    println!("pages: {page_count}");
    println!("next: bob highlights scan");
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
    validate_ref_type(&options.ref_type)?;
    let marker = compose_marker(
        &options.status,
        &options.parent,
        &title,
        id.as_deref(),
    )?;
    let stem = source.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "Markdown file has no file stem: {}",
            source.display()
        ))
    })?;
    let target = config
        .lib_dir
        .join(&options.ref_type)
        .join(stem)
        .with_extension("pdf");
    let sidecar = target.with_extension("md");

    if sidecar.exists() {
        return Err(CommandError::new(format!(
            "refusing to create {} because Highlights would treat the existing Markdown file as its sidecar: {}",
            target.display(),
            sidecar.display()
        )));
    }
    if target.exists() && !options.force {
        return Err(CommandError::new(format!(
            "target PDF already exists: {}; pass --force to overwrite it",
            target.display()
        )));
    }

    Ok(CreatePlan {
        source,
        target,
        sidecar,
        title,
        id,
        marker,
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

fn validate_ref_type(ref_type: &str) -> Result<()> {
    let path = Path::new(ref_type);
    let mut components = path.components();
    let valid = matches!(
        components.next(),
        Some(Component::Normal(value)) if !value.is_empty()
    ) && components.next().is_none();
    if !valid {
        return Err(CommandError::new(format!(
            "ref type must be one directory name, not a path: {ref_type:?}"
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

fn compose_marker(
    status: &str,
    parent: &str,
    title: &str,
    id: Option<&str>,
) -> Result<String> {
    validate_marker_parent_value(
        parent,
        2,
        &MarkerValue::String(parent.to_string()),
    )?;
    let mut projection = Projection::new();
    projection.insert(
        FIELD_STATUS.to_string(),
        MarkerValue::String(status.to_string()),
    );
    projection.insert(
        FIELD_PARENT.to_string(),
        MarkerValue::String(format!("[[{parent}]]")),
    );
    projection
        .insert("title".to_string(), MarkerValue::String(title.to_string()));
    if let Some(id) = id {
        projection
            .insert(FIELD_ID.to_string(), MarkerValue::String(id.to_string()));
    }
    validate_required_marker_keys(&projection, "create marker")?;
    let marker = render_marker(&projection)?;
    let normalized = parse_marker_with_normalization(&marker)?;
    validate_required_marker_keys(&normalized.projection, "create marker")?;
    render_marker(&normalized.projection)
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

fn render_and_install(
    pandoc: &OsStr,
    plan: &CreatePlan,
    render_path: &Path,
    filter_path: &Path,
) -> Result<usize> {
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

    let mut document = Document::load(render_path).map_err(|error| {
        CommandError::new(format!(
            "read rendered PDF {}: {error}",
            render_path.display()
        ))
    })?;
    let page_count = document.get_pages().len();
    embed_marker(&mut document, &plan.marker)?;
    atomic_save_pdf(&plan.target, &mut document)?;
    Ok(page_count)
}

fn embed_marker(document: &mut Document, marker: &str) -> Result<()> {
    let first_page_id = document
        .page_iter()
        .next()
        .ok_or_else(|| CommandError::new("rendered PDF has no first page"))?;
    let annotation_id = document.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(24),
            Object::Integer(24),
        ],
        "Contents" => pdf_text_string(marker),
    });
    let annotation = Object::Reference(annotation_id);
    let annots = document
        .get_dictionary(first_page_id)
        .ok()
        .and_then(|page| page.get(b"Annots").ok())
        .cloned();

    match annots {
        None => document
            .get_object_mut(first_page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| {
                CommandError::new(format!(
                    "read rendered PDF first page: {error}"
                ))
            })?
            .set("Annots", Object::Array(vec![annotation])),
        Some(Object::Array(_)) => document
            .get_object_mut(first_page_id)
            .and_then(Object::as_dict_mut)
            .and_then(|page| page.get_mut(b"Annots"))
            .and_then(Object::as_array_mut)
            .map_err(|error| {
                CommandError::new(format!(
                    "read rendered PDF page annotations: {error}"
                ))
            })?
            .push(annotation),
        Some(Object::Reference(id)) => document
            .get_object_mut(id)
            .and_then(Object::as_array_mut)
            .map_err(|error| {
                CommandError::new(format!(
                    "read rendered PDF annotation array: {error}"
                ))
            })?
            .push(annotation),
        Some(_) => {
            return Err(CommandError::new(
                "rendered PDF first-page /Annots value is not an array",
            ));
        }
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
    println!("sidecar_guard: {}", plan.sidecar.display());
    println!("title: {}", plan.title);
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    if let Some(id) = &plan.id {
        println!("id: {id}");
    }
    println!("marker:");
    print!("{}", plan.marker);
}

#[cfg(test)]
mod tests {
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
            dry_run: false,
            force: false,
            include_id: false,
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
    fn plan_derives_ref_type_output_and_valid_marker() {
        let temp = TempDir::new("plan");
        let source = temp.path.join("report.md");
        fs::write(&source, "# Report\n").expect("write source");
        let mut options = options();
        options.ref_type = "books".to_string();

        let plan =
            plan_create(&config(&temp.path), &source, &options).expect("plan");

        assert_eq!(plan.target, temp.path.join("lib/books/report.pdf"));
        assert_eq!(plan.sidecar, temp.path.join("lib/books/report.md"));
        let marker =
            parse_marker_with_normalization(&plan.marker).expect("marker");
        assert_eq!(
            marker.projection.get(FIELD_PARENT),
            Some(&MarkerValue::String(format!("[[{DEFAULT_PARENT}]]")))
        );
        assert_eq!(
            marker.projection.get(FIELD_STATUS),
            Some(&MarkerValue::String(DEFAULT_STATUS.to_string()))
        );
        assert!(!marker.projection.contains_key(FIELD_ID));
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
    fn plan_refuses_existing_pdf_without_force() {
        let temp = TempDir::new("overwrite");
        let source = temp.path.join("report.md");
        fs::write(&source, "# Report\n").expect("write source");
        let target = temp.path.join("lib/chat/report.pdf");
        fs::create_dir_all(target.parent().expect("target parent"))
            .expect("create target parent");
        fs::write(&target, b"existing").expect("write target");

        let error = plan_create(&config(&temp.path), &source, &options())
            .expect_err("must refuse overwrite");
        assert!(error.to_string().contains("--force"), "{error}");

        let mut forced = options();
        forced.force = true;
        assert!(plan_create(&config(&temp.path), &source, &forced).is_ok());
    }

    #[test]
    fn plan_refuses_highlights_markdown_sidecar_even_with_force() {
        let temp = TempDir::new("sidecar");
        let source = temp.path.join("report.md");
        fs::write(&source, "# Report\n").expect("write source");
        let sidecar = temp.path.join("lib/chat/report.md");
        fs::create_dir_all(sidecar.parent().expect("sidecar parent"))
            .expect("create sidecar parent");
        fs::write(&sidecar, "# Sidecar\n").expect("write sidecar");
        let mut forced = options();
        forced.force = true;

        let error = plan_create(&config(&temp.path), &source, &forced)
            .expect_err("must refuse sidecar collision");
        assert!(error.to_string().contains("sidecar"), "{error}");
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
    fn marker_rejects_wikilink_parent_and_unknown_status() {
        assert!(compose_marker("ready", "[[obsidian_ref]]", "Report", None)
            .is_err());
        assert!(
            compose_marker("unknown", "obsidian_ref", "Report", None).is_err()
        );
    }
}

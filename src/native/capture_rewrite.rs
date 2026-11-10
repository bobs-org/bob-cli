use std::{
    ffi::OsString,
    io::{self, IsTerminal, Read},
    iter,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture_flag_guard,
    capture_language::{
        self, AliasStamp, DraftRewrite, RewriteRule, SigilAliasTip, TextEdit,
    },
    style::Styler,
};

const COMMAND_NAME: &str = "bob capture-rewrite";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    if let Some(hit) = capture_flag_guard::find_unknown_long_flag(
        &args,
        &capture_flag_guard::capture_rewrite_spec(),
    ) {
        let message =
            capture_flag_guard::unexpected_argument_message(&hit.flag);
        if hit.format_json {
            return print_error(&message, OutputFormat::Json);
        }
        return print_clap_error(
            command.error(clap::error::ErrorKind::UnknownArgument, message),
        );
    }
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let cursor = matches.get_one::<usize>("cursor").copied();
    let raw_text = match raw_text_from_matches(&matches) {
        Ok(raw_text) => raw_text,
        Err(error) => return print_error(&error, output_format),
    };
    if capture_language::normalize_task_text(&raw_text).is_empty() {
        return print_error(&missing_text_error(), output_format);
    }
    if let Some(position) = cursor
        && (position > raw_text.len() || !raw_text.is_char_boundary(position))
    {
        return print_error(
            "--cursor must be a UTF-8 byte boundary within TEXT",
            output_format,
        );
    }

    let result = CaptureRewriteResult::new(raw_text, cursor);
    print_success(&result, output_format);
    0
}

fn missing_text_error() -> String {
    capture_language::missing_text_error()
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Apply the capture grammar's automatic draft rewrites")
        .long_about(
            "Apply the capture grammar's editor typing assists to TEXT and \
report the resulting edits, cursor, and a human summary.\n\n\
The command is purely lexical and completely read-only: it never opens the \
vault, never reads the clipboard, never touches the filesystem, and takes no \
--bob-dir. It implements five rules.\n\n\
Block-ID separator toggle (requires --cursor): typing '^' immediately after \
a complete plain '@route:id' or '@route+id', ':' after '@route^id' or \
'@route+id', or '+' after '@route:id' or '@route^id' consumes that character \
and switches the separator. Typing ':' right after a sealed '@route^id@' \
gives '@route:id@', and '^' after '@route:id@' gives '@route^id@'. A glued \
'.' after '@route:id' / '@route^id' switches between ':' and '^' \
('@route:id.' becomes '@route^id', and '@route^id.' becomes '@route:id'); \
after '@route+id' it stays literal. Sealed '@route:id@.' and \
'@route^id@.' also switch between the two sealed forms. A glued '.' also \
switches complete task-bullet IDs between ':id' and '^id'. For example, \
'@file:id+' becomes \
'@file+id', '@file^id+' becomes '@file+id', \
'@file+id^' becomes '@file^id', '@file+id:' becomes '@file:id', \
'@file^id@:' becomes '@file:id@', and '@file:id@^' becomes '@file^id@' \
(`switch_block_id_separator`). The caret stays just past the unchanged ID \
(and seal, when sealed). Task IDs at the end of task-list bullets and \
project-note task bullets also switch: ':id^' becomes '^id', '^id:' becomes \
':id', and a glued '.' switches between the two, including nested lines. \
Only the token ending at --cursor is examined; \
the key must sit against the ID (or seal) with no space, and it must end the token. \
Suffixes already inside the token ('#name', '=...'), '@@', '@!', standalone \
'^route:id', '&note:id', '!note:id', incomplete \
markers, same-separator repeats after a seal, '+' after a seal, and tokens the \
contextual parser does not select are left unchanged. On a `>` transfer \
line, '+' after '@route:id' or '@route^id' in a source or destination slot \
switches to '@route+id' ('@inbox:bench+ > @db' becomes '@inbox+bench > \
@db', '@a+x > @c:tid+' becomes '@a+x > @c+tid'); ':' or '^' after a \
transfer '+id' stays literal, since it would break the transfer. Without \
--cursor this \
rule is skipped rather than searching the draft.\n\n\
Relation-label pipe attach (requires --cursor): typing '|' after \
whitespace that follows a relation token deletes that whitespace, so \
'~note:id |' becomes '~note:id|'. Only the '|' token ending at --cursor \
is examined; it fires only where bob's completion grammar would open a \
'relation_label' field, so prose pipes, mid-line relations, \
already-labelled tokens, partial targets, escapes, and newlines are \
left unchanged. The caret lands just past the glued '|'. This is an editor \
assist: 'bob capture' and 'bob capture-parse' keep the literal ' |' \
text. Without --cursor this rule is skipped rather than searching the \
draft.\n\n\
Sigil alias (requires --cursor): typing '.' immediately after '@route' \
becomes '^', or ';' becomes ':', only where bob's completion grammar would \
open the corresponding Block ID picker. A '.' glued to a complete \
'@route:id' / '@route^id' toggles its separator instead of expanding as an \
alias. A whole-item ';' becomes ':' only \
where the Task Link picker opens. A '- .' or '- ;' first-level bullet in a \
sealed project-note item becomes '- ^' or '- :' where the Move picker would \
open, and '- ^.' becomes '- ^^'. Typing '.' after a space or tab that \
follows body text becomes '@' wherever a bare '@' would open route \
completion. It stays literal at a line or bullet-body start, after a \
checkbox, glued to a word, inside an unclosed backtick span or a wikilink, \
and in positions with no route field. For example, 'Buy milk .' becomes \
'Buy milk @', while '@errands.' still becomes '@errands^' as the first '.' \
rule. A spaced '.' that does not become '@' becomes '&' wherever a bare '&' \
would open the Depends On picker, and a spaced ';' that does not become ':' \
becomes '*' wherever a bare '*' would open the Blocks picker; both follow \
the same literal positions. For example, '@home .' becomes '@home &' and \
'Buy milk ;' becomes 'Buy milk *'. Typing '\\' or '/' becomes '#' wherever \
bob's completion grammar would open a '#' menu: task type, section, drawer \
and drawer bullet, Pomodoro name, named-start and past-session name, split \
type, merge destination, and wikilink heading. A '\\' or '/' at a line \
start, glued to a word, or right after another '\\' or '/' stays literal. A \
'/' also stays literal where bob already reads it as part of a field (Go \
To, the '=/' split, wikilink note paths, '@@' routes, section and Pomodoro \
names); block-ID fields do not count. For example, '@cash,' \
becomes '@cash+', '@cash+goog-exit,' becomes '@cash+goog-exit +', and \
'Buy milk ,' becomes 'Buy milk +'; '@errands.' and '@errands;' still become \
'@errands^' and '@errands:'. Bob's completion grammar decides when a \
comma is eligible; prose and other picker contexts keep it literal. The \
aliases use the same picker behavior as the real sigils. Typing '=' after a \
space or tab aliases '~' only where a bare '~' would open the Links To \
relation picker. It stays literal at a line or bullet-body start, glued to a \
word, inside an unclosed backtick span, or inside a wikilink; 'Fix it =' \
becomes 'Fix it ~', while 'Fix it -' stays literal. Typing '=' after a bare \
'~' flips it to '~~' (Linked From), and '~~=' flips it back to '~' only \
where the result opens an empty Links To target. The past-session dash alias \
still works inside x= tokens, such as 'x=2-' to 'x=2~'. Typing a lowercase \
'x' or 's' that ends a close selection aliases '!' (complete) or '*' (park): \
'xx' becomes 'x!', 'xs' becomes 'x*', 'x=1=s' becomes 'x=1=*', and \
'@r:id=xx' becomes '@r:id=x!'. Competing wildcards ('x!s' to 'x!*') stay a \
viable prefix. The letters stay literal in past names ('x=1s'), after a \
dangling ',' or '@', on child bullets, and in prose. 'bob capture' and \
'bob capture-parse' keep the literal text. Without --cursor this rule is \
skipped rather than searching the draft.\n\n\
Relation direction toggle (requires --cursor): typing '~' or '=' immediately \
after a complete '~note:id' or '~~note:id' flips the direction and consumes \
the trigger. After a labelled relation, only '~' flips, because '=' is valid \
label text. The token must be glued to the trigger, be complete, and open a \
relation target or label in bob's completion grammar; spaced keys, protected or \
escaped tokens, partial or invalid relations, and text glued after the caret \
stay unchanged. The cursor lands just past the flipped relation, and the rule \
is 'toggle_relation_direction'. Without --cursor this rule is skipped rather \
than searching the draft.\n\n\
Bare '@@' absorption: typing a bare '@@' inside an item that already \
carries a local destination marker (or, absent that, when the draft already \
carries exactly one other '@@' declaration) rewrites the bare token to \
'@@<payload>', deletes the token it absorbed, and deletes every other \
declaration token in the draft, so the result always carries at most one \
declaration. Only the bare '@@' at --cursor (or, when --cursor is omitted, \
the last bare '@@' in source order) is a candidate. An item whose single \
            local marker cannot be expressed as a declaration -- '@route#Section', \
            '@route+block-id' (an existing task destination), \
            '@route+block-id#section', '@route+block-id!' (a task toggle), \
'@route^block-id', '@route:block-id', or a trailing bare '#' -- is left \
untouched with a notice explaining why; an item with more than one local \
marker is left untouched with no notice, since 'bob capture-parse' already \
reports that duplicate. Feeding a rewrite's own output back in is a no-op, \
because the claiming token is no longer bare and no longer ends in a toggle \
trigger. A whole-item `x[<N>][!<M>]` close item is never rewritten or \
absorbed, and a `@@` declaration never applies to it. A `>` transfer line \
never absorbs either: a bare `@@` on the line is left untouched with an \
`@@ cannot take a capture transfer` notice naming the destination.\n\n\
Typing tips (requires --cursor): when nothing changed, the JSON \
'sigil_alias_tip' field says whether an unshifted alias key would have typed \
the sigil that ends at --cursor. bob proves it by swapping each candidate \
key ('.' for '@' '^' '&', ';' for ':' '*', ',' for '+', '=' for '~', '/' or \
'\\' for '#') in for the sigil and running the whole rewrite; a key counts \
only when that forward rewrite yields exactly TEXT with the caret unchanged. \
The object carries the lesson 'id', the 'sigil', the preferred 'keys', the \
sigil's UTF-8 byte 'range', and an optional 'where' phrase. An explicit null \
means no tip is proved and marks a bob that reports tips; the field is \
absent without --cursor or when the draft changed. Every rewrite that fires \
the sigil alias rule also carries an 'alias' object with its 'id', 'key', \
and 'sigil'. Human output keeps 'no rewrite' and adds one dim 'tip' line.\n\n\
These assists are editor-only: 'bob capture' and 'bob capture-parse' keep \
the literal TEXT, so '@file:id^' is not silently rewritten there. Only a \
missing TEXT or a bad flag is an error; every other input succeeds with \
'changed: false' when nothing needed to change. If TEXT is omitted and \
stdin is piped, it reads the complete piped stdin stream.",
        )
        .after_help(
            "Examples:\n  bob capture-rewrite -c 17 -f json -- 'Do work @file:id^'\n  bob capture-rewrite -c 17 -f json -- 'Do work @file:id.'\n  bob capture-rewrite -c 17 -f json -- 'Do work @file:id+'\n  bob capture-rewrite -c 17 -f json -- 'Do work @file^id+'\n  bob capture-rewrite -c 20 -f json -- 'Fix trip ~work:foo |'\n  bob capture-rewrite -c 9 -f json -- '@errands.'\n  bob capture-rewrite -c 8 -f json -- 'Fix it ='\n  bob capture-rewrite -c 9 -f json -- 'Fix it ~='\n  bob capture-rewrite -c 21 -f json -- 'Fix it ~tasks:target='\n  bob capture-rewrite -c 6 -f json -- '@cash,'\n  bob capture-rewrite -c 16 -f json -- '@cash+goog-exit,'\n  bob capture-rewrite -c 1 -f json -- ';'\n  bob capture-rewrite -c 10 -f json -- 'Buy milk \\'\n  printf '@cash^goog-exit@\\n- .' | bob capture-rewrite -c 20 -f json\n  bob capture-rewrite -c 10 -f json -- 'Buy milk /'\n  bob capture-rewrite -c 10 -f json -- 'Buy milk @'\n  bob capture-rewrite -c 16 -f json -- 'Buy milk @dev @@'\n  bob capture-rewrite -- 'Called Morgan Stanley @cash+goog-exit @@'\n  bob capture-rewrite -c 13 -f json -- '@inbox:bench+ > @db'\n  bob capture-rewrite -f json -- '@a+x > @c @@'\n  printf 'Buy milk @dev @@' | bob capture-rewrite -f json\n  printf '@@foo\\nBuy milk @@\\n' | bob capture-rewrite -f json\n  printf 'Buy milk @dev\\n- more detail @@\\n' | bob capture-rewrite -f json",
        )
        .disable_help_flag(true)
        .arg(cursor_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(text_arg())
}

fn cursor_arg() -> Arg {
    Arg::new("cursor")
        .long("cursor")
        .short('c')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .help("UTF-8 byte offset of the editor's insertion point")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help(
            "Capture text; multiple args are joined with spaces; an unknown \
             `--flag` where TEXT would begin is rejected, later tokens stay literal",
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

/// Mirror `bob capture-parse`'s convention: join every TEXT argument with
/// spaces, or read the complete piped stdin stream when TEXT is omitted.
fn raw_text_from_matches(matches: &ArgMatches) -> Result<String, String> {
    if let Some(values) = matches.get_many::<OsString>("text") {
        return Ok(values
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "));
    }

    if io::stdin().is_terminal() {
        return Ok(String::new());
    }

    let mut text = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|error| format!("read stdin: {error}"))?;
    Ok(text)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureRewriteResult {
    ok: bool,
    schema_version: u32,
    input: String,
    text: String,
    changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<&'static str>,
    edits: Vec<EditJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
    /// The forward alias that fired; present exactly on
    /// `rule: "expand_sigil_alias"` responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<AliasJson>,
    /// Present exactly when `--cursor` was given and nothing changed: an
    /// object when bob proves an alias for the sigil ending at the cursor,
    /// an explicit `null` otherwise. The `null` marks a bob that reports
    /// tips, so a client can tell it apart from an older bob that omits the
    /// key.
    #[serde(skip_serializing_if = "Option::is_none")]
    sigil_alias_tip: Option<Option<TipJson>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AliasJson {
    id: &'static str,
    key: String,
    sigil: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TipJson {
    id: &'static str,
    sigil: String,
    keys: Vec<String>,
    range: SourceRange,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    position: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EditJson {
    range: SourceRange,
    replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct SourceRange {
    start: usize,
    end: usize,
}

impl CaptureRewriteResult {
    fn new(input: String, cursor: Option<usize>) -> Self {
        let rewrite = capture_language::rewrite_draft(&input, cursor);
        let sigil_alias_tip = match cursor {
            Some(position) if rewrite.rule.is_none() => Some(
                capture_language::sigil_alias_tip(&input, position)
                    .map(tip_json),
            ),
            _ => None,
        };
        Self::from_rewrite(input, rewrite, sigil_alias_tip)
    }

    fn from_rewrite(
        input: String,
        rewrite: DraftRewrite,
        sigil_alias_tip: Option<Option<TipJson>>,
    ) -> Self {
        let changed = rewrite.rule.is_some();
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            input,
            text: rewrite.text,
            changed,
            cursor: rewrite.cursor,
            rule: rewrite.rule.map(RewriteRule::code),
            edits: rewrite.edits.iter().map(edit_json).collect(),
            summary: rewrite.summary,
            notices: rewrite.notices,
            alias: rewrite.alias.map(alias_json),
            sigil_alias_tip,
        }
    }
}

fn alias_json(stamp: AliasStamp) -> AliasJson {
    AliasJson {
        id: stamp.id.code(),
        key: stamp.key,
        sigil: stamp.sigil.to_string(),
    }
}

fn tip_json(tip: SigilAliasTip) -> TipJson {
    TipJson {
        id: tip.id.code(),
        sigil: tip.sigil.to_string(),
        keys: tip.keys,
        range: SourceRange {
            start: tip.range.0,
            end: tip.range.1,
        },
        position: tip.position,
    }
}

fn edit_json(edit: &TextEdit) -> EditJson {
    EditJson {
        range: SourceRange {
            start: edit.start,
            end: edit.end,
        },
        replacement: edit.replacement.clone(),
    }
}

fn print_success(result: &CaptureRewriteResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureRewriteResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

fn print_human_success_with_styler(
    result: &CaptureRewriteResult,
    styler: &Styler,
) {
    if !result.changed {
        println!("{}", styler.dim("no rewrite"));
        for notice in &result.notices {
            println!("  {notice}");
        }
        if let Some(Some(tip)) = &result.sigil_alias_tip {
            println!("{}", styler.dim(&tip_line(tip)));
        }
        return;
    }

    let rule = result.rule.unwrap_or("rewrite");
    println!(
        "Capture rewrite {} {}",
        styler.separator(),
        styler.cyan(rule)
    );
    println!();
    println!("  {}  {}", styler.dim("before"), result.input);
    println!("  {}  {}", styler.dim("after "), styler.green(&result.text));
    if let Some(summary) = &result.summary {
        println!();
        println!("  {summary}");
    }
    for notice in &result.notices {
        println!("  {notice}");
    }
}

/// The one dim human line for a proved tip, naming bob's first key.
fn tip_line(tip: &TipJson) -> String {
    let key = tip.keys.first().map_or("", String::as_str);
    let mut line = format!("tip  next time {key} \u{2192} {}", tip.sigil);
    if let Some(position) = tip.position {
        line.push(' ');
        line.push_str(position);
    }
    line
}

fn success_json(result: &CaptureRewriteResult) -> String {
    serde_json::to_string(result).expect("serialize capture rewrite result")
}

fn print_error(message: &str, output_format: OutputFormat) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {message}"),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": message }))
        }
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(input: &str, cursor: Option<usize>) -> CaptureRewriteResult {
        CaptureRewriteResult::new(input.to_string(), cursor)
    }

    fn json(input: &str, cursor: Option<usize>) -> serde_json::Value {
        serde_json::from_str(&success_json(&rewrite(input, cursor)))
            .expect("json")
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn cli_joins_text_arguments_with_spaces() {
        let matches = build_cli()
            .try_get_matches_from(vec![
                COMMAND_NAME,
                "--",
                "Buy",
                "milk",
                "@dev",
                "@@",
            ])
            .expect("parse arguments");
        assert_eq!(
            raw_text_from_matches(&matches).expect("text"),
            "Buy milk @dev @@"
        );
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Human);
    }

    #[test]
    fn cli_accepts_the_json_format_alias() {
        let matches = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "json", "@@"])
            .expect("parse arguments");
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Json);
    }

    #[test]
    fn cli_rejects_an_unknown_format() {
        let error = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "yaml", "@@"])
            .expect_err("unknown format");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn json_shape_absorbs_the_local_marker() {
        let value = json("Buy milk @dev @@", Some(16));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Buy milk @dev @@");
        assert_eq!(value["text"], "Buy milk @@dev");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 14);
        assert_eq!(value["rule"], "absorb_local_marker");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 9, "end": 14 }, "replacement": "" },
                { "range": { "start": 14, "end": 16 }, "replacement": "@@dev" },
            ])
        );
        assert_eq!(value["summary"], "Moved @dev into @@dev");
        assert!(value.get("notices").is_none(), "{value}");
    }

    #[test]
    fn json_omits_cursor_when_not_supplied() {
        let value = json("Buy milk @dev @@", None);
        assert_eq!(value["changed"], true);
        assert!(value.get("cursor").is_none(), "{value}");
    }

    #[test]
    fn json_reports_no_rewrite_with_no_bare_at_at() {
        let value = json("Buy milk @dev", None);
        assert_eq!(value["ok"], true);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "Buy milk @dev");
        assert!(value.get("rule").is_none(), "{value}");
        assert!(value.get("summary").is_none(), "{value}");
    }

    #[test]
    fn json_shape_switches_a_block_id_separator() {
        let value = json("Do work @file:id^", Some(17));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Do work @file:id^");
        assert_eq!(value["text"], "Do work @file^id");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 16);
        assert_eq!(value["rule"], "switch_block_id_separator");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 8, "end": 17 }, "replacement": "@file^id" },
            ])
        );
        assert_eq!(value["summary"], "Changed @file:id to @file^id");
        assert!(value.get("notices").is_none(), "{value}");
    }

    #[test]
    fn json_shape_switches_a_colon_marker_to_plus() {
        let value = json("Do work @file:id+", Some(17));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Do work @file:id+");
        assert_eq!(value["text"], "Do work @file+id");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 16);
        assert_eq!(value["rule"], "switch_block_id_separator");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 8, "end": 17 }, "replacement": "@file+id" },
            ])
        );
        assert_eq!(value["summary"], "Changed @file:id to @file+id");
    }

    #[test]
    fn json_shape_attaches_a_relation_label_pipe() {
        let value = json("Fix trip ~tasks:target |", Some(24));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Fix trip ~tasks:target |");
        assert_eq!(value["text"], "Fix trip ~tasks:target|");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 23);
        assert_eq!(value["rule"], "attach_relation_label");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 22, "end": 23 }, "replacement": "" },
            ])
        );
        assert_eq!(value["summary"], "Attached | to ~tasks:target");
        assert!(value.get("notices").is_none(), "{value}");
    }

    #[test]
    fn json_shape_expands_a_route_sigil_alias() {
        let value = json("@errands.", Some(9));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "@errands.");
        assert_eq!(value["text"], "@errands^");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 9);
        assert_eq!(value["rule"], "expand_sigil_alias");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 8, "end": 9 }, "replacement": "^" },
            ])
        );
        assert_eq!(value["summary"], "Typed ^ for .");
        assert!(value.get("notices").is_none(), "{value}");
    }

    #[test]
    fn json_shape_expands_comma_to_parent_task_picker() {
        let value = json("@cash,", Some(6));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "@cash,");
        assert_eq!(value["text"], "@cash+");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 6);
        assert_eq!(value["rule"], "expand_sigil_alias");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 5, "end": 6 }, "replacement": "+" },
            ])
        );
        assert_eq!(value["summary"], "Typed + for ,");
    }

    #[test]
    fn json_shape_expands_a_glued_target_comma_to_parent_continuation() {
        let raw = "@cash+goog-exit,";
        let value = json(raw, Some(raw.len()));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], raw);
        assert_eq!(value["text"], "@cash+goog-exit +");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], raw.len() + 1);
        assert_eq!(value["rule"], "expand_sigil_alias");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                {
                    "range": { "start": raw.len() - 1, "end": raw.len() },
                    "replacement": " +"
                },
            ])
        );
        assert_eq!(value["summary"], "Typed + for ,");
    }

    #[test]
    fn json_shape_expands_a_whole_item_task_link_alias() {
        let value = json(";", Some(1));
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], ";");
        assert_eq!(value["text"], ":");
        assert_eq!(value["changed"], true);
        assert_eq!(value["cursor"], 1);
        assert_eq!(value["rule"], "expand_sigil_alias");
        assert_eq!(
            value["edits"],
            serde_json::json!([
                { "range": { "start": 0, "end": 1 }, "replacement": ":" },
            ])
        );
        assert_eq!(value["summary"], "Typed : for ;");
        assert!(value.get("notices").is_none(), "{value}");
    }

    #[test]
    fn json_does_not_search_for_a_separator_toggle_without_a_cursor() {
        let value = json("Do work @file:id^", None);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "Do work @file:id^");
        assert!(value.get("rule").is_none(), "{value}");
        assert!(value.get("cursor").is_none(), "{value}");
    }

    #[test]
    fn json_reports_a_rule_a5_notice_without_changing_text() {
        let value = json("note @notes#Ideas @@", None);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "note @notes#Ideas @@");
        assert_eq!(value["notices"].as_array().expect("notices").len(), 1);
        assert!(value["notices"][0]
            .as_str()
            .expect("notice")
            .contains("cannot take a section"));
    }

    #[test]
    fn json_reports_an_existing_task_destination_notice_without_changing_text()
    {
        let value = json("@cash+goog-exit @@", None);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "@cash+goog-exit @@");
        assert_eq!(value["notices"].as_array().expect("notices").len(), 1);
        let notice = value["notices"][0].as_str().expect("notice");
        assert!(
            notice.contains("cannot take an existing task destination"),
            "{notice}"
        );
        assert!(notice.contains("@cash+goog-exit"), "{notice}");
    }

    #[test]
    fn human_output_is_plain_without_color() {
        let styler = Styler::plain();
        assert!(!styler.is_color());
        print_human_success_with_styler(
            &rewrite("Buy milk @dev @@", Some(16)),
            &styler,
        );
        print_human_success_with_styler(
            &rewrite("Buy milk @dev", None),
            &styler,
        );
    }

    #[test]
    fn close_items_are_never_rewritten() {
        for raw in ["x", "x1!2", "=", "@r:id=x", "^r:id=x", "Text @r:id=x"] {
            let value = json(raw, None);
            assert_eq!(value["changed"], false, "{raw}");
            assert_eq!(value["text"], raw, "{raw}");
        }
        // A `@@` declaration leaves a close item alone with a notice.
        let value = json("@@foo\nx", None);
        assert_eq!(value["changed"], false);
        assert_eq!(value["text"], "@@foo\nx");
    }

    #[test]
    fn missing_text_reports_a_usage_error() {
        assert_eq!(
            missing_text_error(),
            "task text is required; pass TEXT or pipe it on stdin"
        );
    }
}

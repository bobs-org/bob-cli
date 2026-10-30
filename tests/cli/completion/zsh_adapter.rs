//! Stubbed-compsys and real-zpty coverage for the embedded `_bob` zsh adapter.
//!
//! The stubbed tests source the adapter file (read through
//! `CARGO_MANIFEST_DIR`) under `zsh -f` with stubbed `_describe`,
//! `_files`, `_message`, `compset` and `compdef`, plus a fake `bob` on
//! `PATH` that replays canned protocol 1 lines and records its argv.
//! The zpty test drives a real interactive zsh through
//! `zmodload zsh/zpty` with no extra crate dependencies. Tests that
//! need a shell print a note and pass when `zsh` is absent.

use crate::support::*;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Absolute path of the embedded adapter source.
fn adapter_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/native/completion/adapters/_bob.zsh")
}

/// True when `zsh` runs; prints the skip note otherwise.
fn have_zsh() -> bool {
    let ok = Command::new("zsh")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !ok {
        println!("skipped: zsh not found on PATH");
    }
    ok
}

/// Stubbed compsys widgets. Each records its call into `$RECORD`; the
/// `_describe` stub additionally records every candidate of the named
/// group array (resolved the same `(@P)` way the adapter builds them).
const STUB_WIDGETS: &str = r#"
_describe() {
  print -r -- "DESCRIBE $*" >> $RECORD
  local args=("$@") i=1 descr="" aname=""
  while (( i <= $#args )); do
    case ${args[$i]} in
      -t|-S|-M) (( i+=2 )); continue ;;
      -*) (( i+=1 )); continue ;;
      *) if [[ -z $descr ]]; then descr=${args[$i]}; else aname=${args[$i]}; break; fi; (( i+=1 ));;
    esac
  done
  local vals=("${(@P)aname}")
  local v
  for v in "$vals[@]"; do print -r -- "CAND $v" >> $RECORD; done
  return 0
}
_files() { print -r -- "FILES $*" >> $RECORD; return 0 }
_message() { print -r -- "MESSAGE $*" >> $RECORD; return 0 }
compset() { print -r -- "COMPSET $*" >> $RECORD; return 0 }
compdef() { print -r -- "COMPDEF $*" >> $RECORD; return 0 }
"#;

/// Outcome of one stubbed driver run.
struct Stubbed {
    record: Vec<String>,
    argv: Vec<String>,
    stdout: String,
    stderr: String,
    bob_stderr: String,
}

/// Run one stubbed scenario.
///
/// `canned` holds the fake-`bob` protocol lines. `pre` runs after
/// sourcing the adapter but before `_bob` (user styles, `NO_COLOR`);
/// `words_line` sets `words`, `CURRENT`, `PREFIX`, `SUFFIX` and
/// `curcontext`; `tail` runs after `_bob`. With `noisy`, the fake
/// `bob` also writes to stderr, and `_bob`'s own stderr is captured.
fn run_stubbed(
    canned: &str,
    pre: &str,
    words_line: &str,
    tail: &str,
    noisy: bool,
) -> Stubbed {
    let temp = TempDir::new("bob-zsh-adapter");
    let dir = temp.path();
    let fake = if noisy {
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$ARGV_RECORD\"\necho \"NOISE ON STDERR\" >&2\ncat \"$CANNED\"\n"
    } else {
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$ARGV_RECORD\"\ncat \"$CANNED\"\n"
    };
    write_executable(&dir.join("bob"), fake);
    write_file(&dir.join("canned.txt"), canned);
    let q =
        |name: &str| shell_single_quote(&dir.join(name).display().to_string());
    let mut driver = String::new();
    driver.push_str("RECORD=");
    driver.push_str(&q("record.txt"));
    driver.push_str("; : > $RECORD\n");
    driver.push_str("export ARGV_RECORD=");
    driver.push_str(&q("argv.txt"));
    driver.push_str(" CANNED=");
    driver.push_str(&q("canned.txt"));
    driver.push('\n');
    driver.push_str("export PATH=");
    driver.push_str(&shell_single_quote(&dir.display().to_string()));
    driver.push_str(":$PATH\n");
    driver.push_str(STUB_WIDGETS);
    driver.push_str("source ");
    driver.push_str(&shell_single_quote(&adapter_path().display().to_string()));
    driver.push('\n');
    driver.push_str(pre);
    driver.push_str(words_line);
    driver.push('\n');
    if noisy {
        driver.push_str("_bob 2>");
        driver.push_str(&q("bob-stderr.txt"));
        driver.push('\n');
    } else {
        driver.push_str("_bob\n");
    }
    driver.push_str(tail);
    let driver_path = dir.join("driver.zsh");
    write_file(&driver_path, &driver);
    let output = Command::new("zsh")
        .arg("-f")
        .arg(&driver_path)
        // The host may export NO_COLOR; the adapter then uses the plain
        // header. Tests that want that path export it in `pre`.
        .env_remove("NO_COLOR")
        .env("LC_ALL", utf8_locale())
        .output()
        .expect("run stubbed zsh driver");
    let read_lines = |name: &str| {
        fs::read_to_string(dir.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    Stubbed {
        record: read_lines("record.txt"),
        argv: read_lines("argv.txt"),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        bob_stderr: fs::read_to_string(dir.join("bob-stderr.txt"))
            .unwrap_or_default(),
    }
}

/// Choose a UTF-8 locale installed on both supported CI hosts.
fn utf8_locale() -> &'static str {
    let locales = Command::new("locale")
        .arg("-a")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).to_ascii_lowercase()
        })
        .unwrap_or_default();
    if locales
        .lines()
        .any(|locale| locale.trim() == "c.utf-8" || locale.trim() == "c.utf8")
    {
        "C.UTF-8"
    } else if locales.lines().any(|locale| {
        locale.trim() == "en_us.utf-8" || locale.trim() == "en_us.utf8"
    }) {
        "en_US.UTF-8"
    } else {
        // Locale names differ across libc implementations. The zsh output
        // assertions below accept its escaped representation as well.
        "C"
    }
}

/// Compsys state for completing `bob capture --format` with an empty
/// cursor word.
const CAPTURE_FORMAT_WORDS: &str = "words=(bob capture --format); CURRENT=3; PREFIX=\"\"; SUFFIX=\"\"; curcontext=\":completion:complete:bob:argument-1:\";";
/// Compsys state for completing a bare `bob `.
const BARE_BOB_WORDS: &str = "words=(bob); CURRENT=2; PREFIX=\"\"; SUFFIX=\"\"; curcontext=\":completion:complete:bob:argument-1:\";";
/// Report `_bob`'s exit status on driver stdout.
const PRINT_RET: &str = "print -r -- \"RET $?\"\n";

#[test]
fn request_argv_unquotes_words_and_passes_suffix() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "capture\tCapture things\tcommands\tspace\n",
        "",
        "words=(bob '\"quoted\"' 'two words'); CURRENT=4; PREFIX=\"pre\"; SUFFIX=\"suf\"; curcontext=\":completion:complete:bob:argument-1:\";",
        PRINT_RET,
        false,
    );
    assert!(
        out.stdout.contains("RET 0"),
        "expected RET 0, stdout:\n{}",
        out.stdout
    );
    let expected = [
        "__complete",
        "zsh",
        "--protocol",
        "1",
        "--suffix",
        "suf",
        "--",
        "bob",
        "quoted",
        "two words",
        "pre",
    ];
    assert_eq!(out.argv, expected, "fake bob argv:\n{:?}", out.argv);
    assert!(out.stderr.is_empty(), "driver stderr:\n{}", out.stderr);
}

#[test]
fn candidate_groups_keep_order_and_nospace_flag() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "capture\tCapture things\tcommands\tspace\nc2\t\tcommands\tspace\n--route\tRoute\toptions\tspace\nsept\tS value\toptions\tnospace\n",
        "",
        CAPTURE_FORMAT_WORDS,
        PRINT_RET,
        false,
    );
    assert!(
        out.stdout.contains("RET 0"),
        "expected RET 0, stdout:\n{}",
        out.stdout
    );
    let describes: Vec<&str> = out
        .record
        .iter()
        .filter(|line| line.starts_with("DESCRIBE "))
        .map(String::as_str)
        .collect();
    assert_eq!(
        describes,
        [
            "DESCRIBE -V -t commands commands _bob_group_0",
            "DESCRIBE -V -t options options _bob_group_1",
            "DESCRIBE -V -t options options _bob_group_2 -S ",
        ],
        "group order and -S placement, record:\n{}",
        out.record.join("\n")
    );
    let candidates: Vec<&str> = out
        .record
        .iter()
        .filter(|line| line.starts_with("CAND "))
        .map(String::as_str)
        .collect();
    assert_eq!(
        candidates,
        [
            "CAND capture:Capture things",
            "CAND c2",
            "CAND --route:Route",
            "CAND sept:S value",
        ],
        "candidates, record:\n{}",
        out.record.join("\n")
    );
}

#[test]
fn colon_escaping_empty_fields_and_defaults() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "a:b\tdesc\tg\tspace\nplain\t\t\tspace\nbare\t\t\t\n",
        "",
        BARE_BOB_WORDS,
        PRINT_RET,
        false,
    );
    let candidates: Vec<&str> = out
        .record
        .iter()
        .filter(|line| line.starts_with("CAND "))
        .map(String::as_str)
        .collect();
    assert_eq!(
        candidates,
        ["CAND a\\:b:desc", "CAND plain", "CAND bare",],
        "colon escaping and empty descriptions, record:\n{}",
        out.record.join("\n")
    );
    // Empty groups fall back to `values`.
    assert!(
        out.record
            .iter()
            .any(|line| line == "DESCRIBE -V -t values values _bob_group_1"),
        "empty group defaults to values, record:\n{}",
        out.record.join("\n")
    );
    assert!(
        out.stdout.contains("RET 0"),
        "expected RET 0, stdout:\n{}",
        out.stdout
    );
}

#[test]
fn directives_map_to_native_widgets() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "!prefix 7\n!dirs\n!files\n!files *.md\n!files-in /vault root\t*.md\n!message enter text\n!future blah\nplain\tA value\tg\tspace\n",
        "",
        CAPTURE_FORMAT_WORDS,
        PRINT_RET,
        false,
    );
    for needle in [
        "COMPSET -P 7",
        "FILES -/",
        "FILES -g *.md",
        "FILES -W /vault root -g *.md",
        "MESSAGE -r enter text",
        "CAND plain:A value",
    ] {
        assert!(
            out.record.iter().any(|line| line == needle),
            "missing {needle:?}, record:\n{}",
            out.record.join("\n")
        );
    }
    // Bare `!files` calls `_files` with no arguments.
    assert!(
        out.record.iter().any(|line| line == "FILES "),
        "bare !files calls _files with no args, record:\n{}",
        out.record.join("\n")
    );
    // Unknown directives from a newer bob are ignored.
    assert!(
        !out.record.iter().any(|line| line.contains("future")),
        "unknown directive leaked through, record:\n{}",
        out.record.join("\n")
    );
    assert!(
        out.stdout.contains("RET 0"),
        "expected RET 0, stdout:\n{}",
        out.stdout
    );
}

#[test]
fn empty_output_returns_one() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed("", "", BARE_BOB_WORDS, PRINT_RET, false);
    assert!(
        out.stdout.contains("RET 1"),
        "empty output must return 1, stdout:\n{}",
        out.stdout
    );
}

#[test]
fn stderr_is_discarded() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "capture\tCapture things\tcommands\tspace\n",
        "",
        BARE_BOB_WORDS,
        PRINT_RET,
        true,
    );
    assert!(
        !out.bob_stderr.contains("NOISE"),
        "_bob leaked bob stderr: {:?}",
        out.bob_stderr
    );
    assert!(
        out.stdout.contains("RET 0"),
        "candidates still complete, stdout:\n{}",
        out.stdout
    );
    assert!(
        out.record
            .iter()
            .any(|line| line == "CAND capture:Capture things"),
        "record:\n{}",
        out.record.join("\n")
    );
}

#[test]
fn default_styles_use_green_headers() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "capture\tCapture things\tcommands\tspace\n",
        "",
        BARE_BOB_WORDS,
        "print -r -- \"RET $?\"\nzstyle -L\n",
        false,
    );
    assert!(
        out.stdout.contains("%B%F{green}"),
        "default header is bold green, stdout:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("group-name ''"),
        "group-name is set, stdout:\n{}",
        out.stdout
    );
}

#[test]
fn user_descriptions_format_wins() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "capture\tCapture things\tcommands\tspace\n",
        "zstyle ':completion:*:*:bob:*:descriptions' format 'USER-FMT'\n",
        BARE_BOB_WORDS,
        "print -r -- \"RET $?\"\nzstyle -L\n",
        false,
    );
    assert!(
        out.stdout.contains("USER-FMT"),
        "user format survives, stdout:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("%B%F{green}"),
        "bob leaves a user format alone, stdout:\n{}",
        out.stdout
    );
}

#[test]
fn no_color_uses_plain_header() {
    if !have_zsh() {
        return;
    }
    let out = run_stubbed(
        "capture\tCapture things\tcommands\tspace\n",
        "export NO_COLOR=1\n",
        BARE_BOB_WORDS,
        "print -r -- \"RET $?\"\nzstyle -L\n",
        false,
    );
    assert!(
        !out.stdout.contains("%B%F{green}"),
        "no green header under NO_COLOR, stdout:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("format "),
        "a plain header is still set, stdout:\n{}",
        out.stdout
    );
}

/// Static properties of the adapter source: no `emulate -L zsh` (it
/// breaks `_describe`), TAB splitting that preserves empty fields, an
/// ownership stamp, stderr discarded, empty-output safety, the
/// loadautofunc guard for the first TAB, and no bob-syntax parsing
/// (the command words are only forwarded to `bob __complete`).
#[test]
fn adapter_source_has_required_properties() {
    let source =
        fs::read_to_string(adapter_path()).expect("read _bob.zsh source");
    let mut lines = source.lines();
    assert_eq!(lines.next(), Some("#compdef bob"));
    assert!(
        lines
            .next()
            .unwrap_or("")
            .starts_with("# Generated by bob completion (protocol "),
        "second line is the ownership stamp"
    );
    assert!(
        !source.contains("emulate"),
        "adapter must not contain emulate (it breaks _describe)"
    );
    // `words` appears exactly once: forwarding the command line to bob.
    assert_eq!(
        source.matches("words").count(),
        1,
        "adapter never parses bob syntax, it only forwards words"
    );
    for required in [
        "command bob __complete",
        "2>/dev/null",
        "(( $#lines )) || return 1",
        "loadautofunc",
        "(@ps:\\t:)",
        "//:/\\\\:",
    ] {
        assert!(
            source.contains(required),
            "adapter must contain {required:?}"
        );
    }
}

/// Real interactive zsh through `zmodload zsh/zpty`: the very first
/// TAB completes immediately and renders rows, `-` offers `--route`,
/// and the `--format` slot shows its values.
#[test]
fn real_zsh_first_tab_completes() {
    if !have_zsh() {
        return;
    }
    let temp = TempDir::new("bob-zsh-zpty");
    let dir = temp.path();
    let adapter = fs::read(adapter_path()).expect("read _bob.zsh source");
    fs::write(dir.join("_bob"), &adapter).expect("stage _bob adapter");
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_bob"), dir.join("bob"))
        .expect("link test bob on PATH");
    let driver = ZPTY_DRIVER;
    write_file(&dir.join("driver.zsh"), driver);
    let mut command = Command::new("zsh");
    command.arg("-f").arg(dir.join("driver.zsh")).arg(dir);
    command.env("BOB_COMPLETE_DEADLINE_MS", BOB_COMPLETION_TEST_DEADLINE_MS);
    let output = match try_output_with_timeout(command, 30) {
        Ok(output) => output,
        Err(error) => panic!("zpty readiness deadline exceeded: {error}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stdout.contains("ZPTY-MISSING") {
        println!("skipped: zsh/zpty not usable on this host");
        return;
    }
    assert!(
        output.status.success(),
        "zpty driver failed, stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    for marker in [
        "SETUP OK",
        "FIRST-TAB OK",
        "DASH-TAB OK",
        "FORMAT-TAB OK",
        "FORMAT-HEADER OK",
    ] {
        assert!(
            stdout.contains(marker),
            "missing {marker:?}, stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

/// One pty session covering all three real-completion scenarios.
/// `capture-task` is exactly ambiguous (its `-id`/`-sections`/`-s`
/// rows share the typed prefix), so the very first TAB renders rows:
/// that proves the `loadautofunc` guard ran `_bob` immediately
/// instead of merely loading it. (`cap` would first extend to the
/// unambiguous `capture` with no listing, which cannot distinguish
/// the two.)
const ZPTY_DRIVER: &str = r#"
zmodload zsh/zpty || { print "ZPTY-MISSING"; exit 2 }
zmodload zsh/zselect || { print "ZSELECT-MISSING"; exit 2 }
TMP=$1
shift
typeset -i deadline=$((SECONDS + 25))
typeset -i sync_count=0
READ_RESULT=
PTY_FD=
wait_for() {
  local expected=$1 expected2=$2 remaining chunk
  READ_RESULT=
  while true; do
    if chunk=$(zpty -rt pb); then
      READ_RESULT+=$chunk
      if [[ $READ_RESULT == *"$expected"* && ( -z "$expected2" || $READ_RESULT == *"$expected2"* ) ]]; then
        return 0
      fi
    fi
    remaining=$((deadline - SECONDS))
    (( remaining > 0 )) || { print -r -- "READINESS TIMEOUT: $READ_RESULT"; return 1; }
    zselect -t 10 "$PTY_FD" || true
  done
}
# Empty the edit buffer in-band, then run a marker command and wait for its
# output. ZLE reads ^U and the command in order, so the next scenario always
# starts on an empty line once the marker prints; the quotes keep the marker
# out of the echoed command line. (A ^C here would race: the shell can still
# read the next scenario's first byte into the doomed line.)
reset_line() {
  (( sync_count += 1 ))
  zpty -w -n pb $'\x15'
  zpty -w pb "print -r -- SY''NC-$sync_count"
  wait_for "SYNC-$sync_count" ''
}
# The pty shell inherits PATH instead of having it typed: a long PATH can
# overflow the tty input queue (1024 bytes on macOS) and lose the setup line.
export PATH="$TMP:$PATH"
zpty -d pb 2>/dev/null
zpty -b pb zsh -f
PTY_FD=$REPLY
# bindkey -e pins ^U to kill-whole-line even when $EDITOR selects vi keys.
zpty -w pb "PS1='ZP> '; bindkey -e"
zpty -w pb "fpath=($TMP \$fpath)"
# -i skips compaudit's interactive insecure-directory prompt so the test is
# hermetic on any host; it does not change how completion renders.
zpty -w pb "autoload -Uz compinit && compinit -D -i && print -r -- COMPINIT-DONE"
wait_for COMPINIT-DONE '' || { print -r -- "SETUP-READ-FAILED"; zpty -d pb; exit 1 }
print -r -- "SETUP OK"
zpty -w -n pb $'bob capture-task\t'
if wait_for capture-task-sections ''; then print -r -- "FIRST-TAB OK"; else print -r -- "FIRST-TAB MISS"; fi
reset_line || { print -r -- "PROMPT-READ-FAILED"; zpty -d pb; exit 1 }
zpty -w -n pb $'bob capture -\t'
if wait_for --route ''; then print -r -- "DASH-TAB OK"; else print -r -- "DASH-TAB MISS"; fi
reset_line || { print -r -- "PROMPT-READ-FAILED"; zpty -d pb; exit 1 }
zpty -w -n pb $'bob capture --format \t'
if wait_for human ''; then print -r -- "FORMAT-TAB OK"; else print -r -- "FORMAT-TAB MISS"; fi
reset_line || { print -r -- "PROMPT-READ-FAILED"; zpty -d pb; exit 1 }
zpty -w -n pb $'bob capture --format \t'
if wait_for format ''; then print -r -- "FORMAT-HEADER OK"; else print -r -- "FORMAT-HEADER MISS"; fi
zpty -d pb
"#;

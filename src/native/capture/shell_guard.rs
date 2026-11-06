//! Shell redirect guards for `bob capture` transfer and merge-in lines.
//!
//! Unquoted, `bob capture @a+x > @c` never reaches bob as a transfer: the
//! shell consumes `> @c` as a stdout redirect, bob sees only `@a+x` (a real
//! Ensure Next write), and the shell creates an empty file `@c`. This guard
//! refuses before planning when stdout is a regular file whose basename
//! starts with `@`, so the accidental write never happens.
//!
//! Unquoted, `bob capture New task < @a+x` never reaches bob as a merge-in:
//! the shell consumes `< @a+x` as a stdin redirect, bob sees only the new
//! task text, and stdin points at a regular file whose basename starts with
//! `@`. With draft arguments present, that stdin redirect refuses before
//! planning with `shell_ate_merge_in_arrow`.
//!
//! Help (`--help`) and pure expansion (`--expand`) keep their existing
//! behavior: the guards run only on the real capture path, after expansion
//! handling. Pipes, TTYs, sockets, devices, and unsupported platforms never
//! trigger. Normal stdin capture without draft arguments keeps working:
//! the stdin guard only applies when the draft came from argv.

use std::path::{Path, PathBuf};

/// Machine-readable code for the shell-ate-arrow refusal.
pub(crate) const SHELL_ATE_TRANSFER_ARROW_CODE: &str =
    "shell_ate_transfer_arrow";
/// Canonical copy from the transfer-capture contract.
pub(crate) const SHELL_ATE_TRANSFER_ARROW_MESSAGE: &str = "your shell ate “>”: quote the draft (bob capture '@a+x > @c') and delete the empty file “@c”";

/// Machine-readable code for the shell-ate-`<` refusal.
pub(crate) const SHELL_ATE_MERGE_IN_ARROW_CODE: &str =
    "shell_ate_merge_in_arrow";
/// Canonical copy from the merge-in contract.
pub(crate) const SHELL_ATE_MERGE_IN_ARROW_MESSAGE: &str =
    "your shell ate “<”: quote the draft (bob capture 'New task < @a+x')";

/// Machine-readable code for the shell-expanded-tilde refusal.
pub(crate) const SHELL_EXPANDED_TILDE_CODE: &str = "shell_expanded_tilde";
/// Canonical copy: the shell expanded an unquoted `~` before bob saw it.
pub(crate) const SHELL_EXPANDED_TILDE_MESSAGE: &str =
    "your shell expanded ~; quote the capture: bob capture 'x=~'";

/// Whether an argv draft contains a token starting with `x=` followed by the
/// non-empty `$HOME` value: the shell expanded an unquoted `~` before bob
/// saw it. Only argv drafts are checked; piped stdin can carry a literal
/// `$HOME` path on purpose. A `pushd`-style `x=~1` expansion cannot be
/// detected and is documented instead.
pub(crate) fn expanded_tilde_token(raw_text: &str) -> Option<String> {
    let home = std::env::var_os("HOME")?.to_string_lossy().into_owned();
    if home.is_empty() {
        return None;
    }
    for token in raw_text.split(|c: char| c.is_whitespace()) {
        if token.len() <= 2 || !token.starts_with("x=") {
            continue;
        }
        let after = &token[2..];
        if after.starts_with(&home) {
            return Some(token.to_string());
        }
    }
    None
}

/// Rewrite an expanded-tilde token by putting `~` back in place of `$HOME`.
#[allow(dead_code)]
pub(crate) fn unexpand_tilde_token(token: &str) -> Option<String> {
    let home = std::env::var_os("HOME")?.to_string_lossy().into_owned();
    if home.is_empty() {
        return None;
    }
    let after = token.strip_prefix("x=")?;
    let rest = after.strip_prefix(&home)?;
    Some(format!("x=~{rest}"))
}

/// Whether `path` looks like a shell-ate-arrow redirect target: basename
/// starts with `@` and the path is a regular file.
pub(crate) fn is_shell_redirect_target(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let starts_with_at = path
            .file_name()
            .is_some_and(|name| name.as_bytes().first() == Some(&b'@'));
        if !starts_with_at {
            return false;
        }
    }
    #[cfg(not(unix))]
    {
        let starts_with_at = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('@'));
        if !starts_with_at {
            return false;
        }
    }
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Platform path for one file descriptor: where fd 0 (stdin) or fd 1
/// (stdout) currently points, when the platform exposes it. Returns `None`
/// on unsupported platforms or when the path cannot be read.
pub(crate) fn fd_redirect_path(fd: i32) -> Option<PathBuf> {
    match fd {
        0 => stdin_redirect_path(),
        1 => stdout_redirect_path(),
        _ => None,
    }
}

/// Platform stdout path: where file descriptor 1 currently points, when
/// the platform exposes it. Returns `None` on unsupported platforms or
/// when the path cannot be read.
pub(crate) fn stdout_redirect_path() -> Option<PathBuf> {
    fd_path(1)
}

/// Platform stdin path: where file descriptor 0 currently points, when
/// the platform exposes it. Returns `None` on unsupported platforms or
/// when the path cannot be read.
pub(crate) fn stdin_redirect_path() -> Option<PathBuf> {
    fd_path(0)
}

fn fd_path(fd: i32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        macos_fd_path(fd)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fd;
        None
    }
}

/// Whether stdout is currently a shell-ate-arrow redirect target.
pub(crate) fn stdout_is_redirect_target() -> Option<PathBuf> {
    fd_is_redirect_target(1)
}

/// Whether stdin is currently a shell-ate-`<` redirect target: with draft
/// arguments present, a regular stdin file whose basename starts with `@`.
pub(crate) fn stdin_is_redirect_target() -> Option<PathBuf> {
    fd_is_redirect_target(0)
}

fn fd_is_redirect_target(fd: i32) -> Option<PathBuf> {
    let path = fd_redirect_path(fd)?;
    if !is_shell_redirect_target(&path) {
        // A deleted `@c` still points at a regular file through the open
        // descriptor even though the path no longer resolves. Check the
        // descriptor itself before giving up.
        if !fd_is_regular_file(fd) {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let starts_with_at = path
                .file_name()
                .is_some_and(|name| name.as_bytes().first() == Some(&b'@'));
            if !starts_with_at {
                return None;
            }
        }
        #[cfg(not(unix))]
        {
            let starts_with_at = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('@'));
            if !starts_with_at {
                return None;
            }
        }
    }
    // Confirm the descriptor itself is a regular file so pipes, TTYs,
    // sockets, and `/dev/null` never trigger.
    if !fd_is_regular_file(fd) {
        return None;
    }
    Some(path)
}

#[cfg(target_os = "macos")]
fn macos_fd_path(fd: i32) -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0 as libc::c_char; 1024];
    let ret = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) };
    if ret == -1 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(OsStr::from_bytes(cstr.to_bytes())))
}

fn fd_is_regular_file(fd: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::metadata(format!("/proc/self/fd/{fd}"))
            .is_ok_and(|meta| meta.is_file())
    }
    #[cfg(target_os = "macos")]
    {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::fstat(fd, &mut stat) };
        if ret != 0 {
            return false;
        }
        (stat.st_mode as libc::mode_t & libc::S_IFMT) == libc::S_IFREG
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fd;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_prefixed_regular_files_trigger() {
        let dir = tempfile::tempdir().expect("transfer guard scratch dir");
        let target = dir.path().join("@c");
        std::fs::write(&target, "").expect("empty @c");
        assert!(is_shell_redirect_target(&target));
        let plain = dir.path().join("c.md");
        std::fs::write(&plain, "").expect("plain file");
        assert!(!is_shell_redirect_target(&plain));
    }

    #[test]
    fn directories_and_missing_files_do_not_trigger() {
        let dir = tempfile::tempdir().expect("transfer guard scratch dir");
        assert!(!is_shell_redirect_target(dir.path()));
        let at_dir = dir.path().join("@dir");
        std::fs::create_dir(&at_dir).expect("at dir");
        assert!(!is_shell_redirect_target(&at_dir));
        assert!(!is_shell_redirect_target(&dir.path().join("@missing")));
    }

    #[test]
    fn pipes_do_not_trigger() {
        assert!(!is_shell_redirect_target(Path::new("pipe:[12345]")));
    }
}

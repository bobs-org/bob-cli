//! Shell redirect guard for `bob capture` transfer lines.
//!
//! Unquoted, `bob capture @a+x > @c` never reaches bob as a transfer: the
//! shell consumes `> @c` as a stdout redirect, bob sees only `@a+x` (a real
//! Ensure Next write), and the shell creates an empty file `@c`. This guard
//! refuses before planning when stdout is a regular file whose basename
//! starts with `@`, so the accidental write never happens.
//!
//! Help (`--help`) and pure expansion (`--expand`) keep their existing
//! behavior: the guard runs only on the real capture path, after expansion
//! handling. Pipes, TTYs, and unsupported platforms never trigger.

use std::path::{Path, PathBuf};

/// Machine-readable code for the shell-ate-arrow refusal.
pub(crate) const SHELL_ATE_TRANSFER_ARROW_CODE: &str =
    "shell_ate_transfer_arrow";
/// Canonical copy from the transfer-capture contract.
pub(crate) const SHELL_ATE_TRANSFER_ARROW_MESSAGE: &str = "your shell ate “>”: quote the draft (bob capture '@a+x > @c') and delete the empty file “@c”";

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

/// Platform stdout path: where file descriptor 1 currently points, when
/// the platform exposes it. Returns `None` on unsupported platforms or
/// when the path cannot be read.
pub(crate) fn stdout_redirect_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link("/proc/self/fd/1").ok()
    }
    #[cfg(target_os = "macos")]
    {
        macos_stdout_path()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Whether stdout is currently a shell-ate-arrow redirect target.
pub(crate) fn stdout_is_redirect_target() -> Option<PathBuf> {
    let path = stdout_redirect_path()?;
    if !is_shell_redirect_target(&path) {
        // A deleted `@c` still points at a regular file through the open
        // descriptor even though the path no longer resolves. Check the
        // descriptor itself before giving up.
        if !fd_is_regular_file() {
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
    if !fd_is_regular_file() {
        return None;
    }
    Some(path)
}

#[cfg(target_os = "macos")]
fn macos_stdout_path() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0 as libc::c_char; 1024];
    let ret = unsafe { libc::fcntl(1, libc::F_GETPATH, buf.as_mut_ptr()) };
    if ret == -1 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(OsStr::from_bytes(cstr.to_bytes())))
}

fn fd_is_regular_file() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::metadata("/proc/self/fd/1").is_ok_and(|meta| meta.is_file())
    }
    #[cfg(target_os = "macos")]
    {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::fstat(1, &mut stat) };
        if ret != 0 {
            return false;
        }
        (stat.st_mode as libc::mode_t & libc::S_IFMT) == libc::S_IFREG
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
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

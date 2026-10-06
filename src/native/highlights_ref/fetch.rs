//! A `curl`-based URL fetcher for Highlights PDF and arXiv targets.
//!
//! The fetcher runs `curl` (or `BOB_HIGHLIGHTS_CURL`) as a subprocess with
//! redirects disabled and follows up to [`MAX_REDIRECTS`] hops itself, so
//! every hop passes through [`validate_and_clean`](super::clip_url::validate_and_clean)
//! and a redirect to a private host is refused. hosts with no browser
//! (apollo) can still fetch PDFs this way; only article pages need the
//! clip adapter.
//!
//! HTTP error statuses are returned to the caller, not treated as fetch
//! failures: a 404 is routing information, while a curl exit code is a
//! transport failure.

use std::{
    env, fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Command,
};

use super::{clip_url::validate_and_clean, CommandError};

/// Environment override replacing the `curl` program. This is the test
/// seam, like `BOB_PANDOC_COMMAND`.
pub(super) const ENV_CURL_OVERRIDE: &str = "BOB_HIGHLIGHTS_CURL";

/// Refusal limit for downloaded PDFs, matching the vault-sync cap the
/// clip adapter enforces (`PDF_MAX_BYTES` there).
pub(super) const PDF_MAX_BYTES: u64 = 95 * 1024 * 1024;

/// Maximum redirect hops followed before giving up.
pub(super) const MAX_REDIRECTS: usize = 10;

/// Serializes tests that point `BOB_HIGHLIGHTS_CURL` at fake scripts:
/// environment variables are process-global, so parallel tests must not
/// race on the same key.
#[cfg(test)]
pub(super) static CURL_TEST_LOCK: std::sync::Mutex<()> =
    std::sync::Mutex::new(());

/// What one `curl` invocation returned, after redirects were followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FetchResult {
    /// The final HTTP status code (a 404 is returned, not raised).
    pub(super) status: u16,
    /// The final `Content-Type` header value, trimmed of whitespace.
    pub(super) content_type: String,
    /// The final URL after redirects.
    pub(super) final_url: String,
    /// The downloaded body.
    pub(super) path: PathBuf,
    /// Its size in bytes.
    pub(super) bytes: u64,
}

/// A fetch failure: the message goes after `error:`, the hint after
/// `hint:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FetchError {
    pub(super) message: String,
    pub(super) hint: Option<String>,
}

impl FetchError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub(super) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(super) fn message(&self) -> &str {
        &self.message
    }

    pub(super) fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

impl From<CommandError> for FetchError {
    fn from(error: CommandError) -> Self {
        Self::new(error.message)
    }
}

/// Download `url` to `dest` (overwritten), following redirects manually.
///
/// `max_time_secs` caps the whole `curl` invocation for one hop.
pub(super) fn fetch_url(
    url: &str,
    dest: &Path,
    max_time_secs: u64,
) -> std::result::Result<FetchResult, FetchError> {
    fetch_with_curl(url, dest, max_time_secs, &curl_program())
}

/// The `curl` program: `BOB_HIGHLIGHTS_CURL` when set and non-empty,
/// otherwise plain `curl` resolved through `PATH`.
fn curl_program() -> String {
    env::var(ENV_CURL_OVERRIDE)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "curl".to_string())
}

fn fetch_with_curl(
    url: &str,
    dest: &Path,
    max_time_secs: u64,
    curl: &str,
) -> std::result::Result<FetchResult, FetchError> {
    let start_host = url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    if std::io::stderr().is_terminal() {
        eprintln!("fetching {start_host}…");
    }
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let (status, content_type, redirect_url) =
            curl_one_hop(curl, &current, dest, max_time_secs)?;
        let redirect = redirect_url.trim().to_string();
        if !(300..400).contains(&status) || redirect.is_empty() {
            let bytes = fs::metadata(dest)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if bytes > PDF_MAX_BYTES {
                return Err(FetchError::new(format!(
                    "download larger than 95 MiB ({} bytes): {current}",
                    bytes,
                )));
            }
            return Ok(FetchResult {
                status,
                content_type,
                final_url: current,
                path: dest.to_path_buf(),
                bytes,
            });
        }
        let base = url::Url::parse(&current).map_err(|error| {
            FetchError::new(format!(
                "cannot resolve redirect for {current}: {error}"
            ))
        })?;
        let next = base
            .join(&redirect)
            .map_err(|error| {
                FetchError::new(format!(
                    "cannot resolve redirect {redirect:?} for {current}: {error}"
                ))
            })?
            .to_string();
        // Every hop passes validation, so a redirect to a private host
        // is refused instead of fetched.
        validate_and_clean(&next)?;
        current = next;
    }
    Err(FetchError::new(format!("too many redirects for {url}")))
}

/// Run one `curl` hop without `-L` and split its `-w` trailer into
/// `(status, content_type, redirect_url)`.
fn curl_one_hop(
    curl: &str,
    url: &str,
    dest: &Path,
    max_time_secs: u64,
) -> std::result::Result<(u16, String, String), FetchError> {
    let output = Command::new(curl)
        .arg("-sS")
        .arg("--proto")
        .arg("=http,https")
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-time")
        .arg(max_time_secs.to_string())
        .arg("--max-filesize")
        .arg("95M")
        .arg("-o")
        .arg(dest)
        .arg("-w")
        .arg("%{http_code}\n%{content_type}\n%{redirect_url}\n")
        .arg("--user-agent")
        .arg(format!(
            "bob-cli/{} (+https://github.com/bobs-org/bob-cli)",
            env!("CARGO_PKG_VERSION"),
        ))
        .arg(url)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                FetchError::new(format!("curl command not found: {curl}"))
                    .with_hint(format!(
                        "install curl or set {ENV_CURL_OVERRIDE}"
                    ))
            } else {
                FetchError::new(format!("run curl {curl}: {error}"))
            }
        })?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(map_curl_exit(code, url, &stderr));
    }
    parse_curl_trailer(&String::from_utf8_lossy(&output.stdout), url)
}

/// Split curl's `-w` trailer (`<code>\n<content_type>\n<redirect>\n`)
/// into its parts.
fn parse_curl_trailer(
    stdout: &str,
    url: &str,
) -> std::result::Result<(u16, String, String), FetchError> {
    let mut parts: Vec<&str> = stdout.split('\n').collect();
    // The trailer ends with a newline, leaving one empty tail element.
    if parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.len() < 3 {
        return Err(FetchError::new(format!(
            "could not parse curl status for {url}: {stdout:?}"
        )));
    }
    let status: u16 = parts[0].parse().map_err(|_| {
        FetchError::new(format!(
            "could not parse curl status for {url}: {stdout:?}"
        ))
    })?;
    // The redirect URL is the final line; everything between the status
    // and it is the content type.
    let redirect_url = parts[parts.len() - 1].to_string();
    let content_type = parts[1..parts.len() - 1].join("\n").trim().to_string();
    Ok((status, content_type, redirect_url))
}

/// Map a failed `curl` exit code to its message.
fn map_curl_exit(code: i32, url: &str, stderr: &str) -> FetchError {
    match code {
        6 => FetchError::new(format!("cannot resolve host: {url}")),
        7 => FetchError::new(format!("connection failed: {url}")),
        28 => FetchError::new(format!("timed out fetching {url}")),
        35 | 60 => FetchError::new(format!("TLS failure fetching {url}")),
        63 => FetchError::new(format!("download larger than 95 MiB: {url}")),
        _ if stderr.is_empty() => {
            FetchError::new(format!("curl failed (exit {code}) for {url}"))
        }
        _ => FetchError::new(format!("curl failed (exit {code}): {stderr}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake curl: serves canned bodies by URL, prints the `-w` trailer
    /// lines, and exits on demand. The request URL is the last argument
    /// and the `-o` argument is the destination file.
    fn write_fake_curl(dir: &Path) -> PathBuf {
        let path = dir.join("fake-curl.sh");
        let script = r#"#!/bin/sh
# Fake curl for fetch.rs tests. Serves canned bodies from $ROOT/<name>
# selected by the request URL, prints the -w trailer (status,
# content-type, redirect URL), and exits as told.
ROOT="$FAKE_CURL_ROOT"
dest=""
url=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
case "$url" in
  *"/two-hop-start"*) printf '302\ntext/html\n/two-hop-mid\n' ;;
  *"/two-hop-mid"*) printf '302\ntext/html\n/two-hop-end?x=1\n' ;;
  *"/two-hop-end"*) cp "$ROOT/paper.pdf" "$dest"; printf '200\napplication/pdf\n\n' ;;
  *"/relative-start"*) printf '302\ntext/html\npapers/paper.pdf\n' ;;
  *"/private-redirect"*) printf '302\ntext/html\nhttp://10.0.0.1/secret\n' ;;
  *"/missing"*) printf '404\ntext/html; charset=utf-8\n\n' ;;
  *"/exit-63"*) printf 'curl: body too large\n' >&2; exit 63 ;;
  *"/exit-28"*) printf 'curl: timeout\n' >&2; exit 28 ;;
  *"/exit-9"*) printf 'curl: strange failure\n' >&2; exit 9 ;;
  *) cp "$ROOT/paper.pdf" "$dest"; printf '200\napplication/pdf\n\n' ;;
esac
"#;
        fs::write(&path, script).expect("write fake curl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("chmod fake curl");
        }
        path
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-fetch-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            name,
        ));
        fs::create_dir_all(&dir).expect("create fetch test dir");
        fs::write(dir.join("paper.pdf"), b"%PDF-1.4 fake body\n")
            .expect("write canned PDF body");
        dir
    }

    #[test]
    fn fetch_covers_redirects_statuses_and_exit_codes() {
        let _guard = CURL_TEST_LOCK.lock().expect("lock curl env");
        let dir = test_dir("cases");
        let fake = write_fake_curl(&dir);
        unsafe { env::set_var("FAKE_CURL_ROOT", &dir) };
        unsafe { env::set_var(ENV_CURL_OVERRIDE, &fake) };

        // A plain 200 PDF download.
        let dest = dir.join("paper.pdf.out");
        let fetched = fetch_url("https://example.com/paper.pdf", &dest, 30)
            .expect("fetch 200 PDF");
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.content_type, "application/pdf");
        assert_eq!(fetched.final_url, "https://example.com/paper.pdf");
        assert_eq!(fetched.bytes, b"%PDF-1.4 fake body\n".len() as u64);
        assert_eq!(
            fs::read(&dest).expect("read downloaded body"),
            b"%PDF-1.4 fake body\n"
        );

        // A two-hop redirect chain ending at a PDF.
        let dest = dir.join("two-hop.pdf.out");
        let fetched = fetch_url("https://example.com/two-hop-start", &dest, 30)
            .expect("follow two-hop redirect");
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.final_url, "https://example.com/two-hop-end?x=1");

        // A relative Location resolves against the current URL.
        let dest = dir.join("relative.pdf.out");
        let fetched =
            fetch_url("https://example.com/relative-start", &dest, 30)
                .expect("follow relative redirect");
        assert_eq!(fetched.final_url, "https://example.com/papers/paper.pdf");

        // A redirect to a private host is refused, not fetched.
        let dest = dir.join("private.pdf.out");
        let error =
            fetch_url("https://example.com/private-redirect", &dest, 30)
                .expect_err("private redirect must be refused");
        assert!(
            error.message.contains("private"),
            "unexpected message: {}",
            error.message
        );

        // A 404 is returned to the caller for routing.
        let dest = dir.join("missing.out");
        let fetched = fetch_url("https://example.com/missing", &dest, 30)
            .expect("404 is returned");
        assert_eq!(fetched.status, 404);
        assert_eq!(fetched.content_type, "text/html; charset=utf-8");

        // Curl exit codes map to their messages.
        let dest = dir.join("exit-63.out");
        let error = fetch_url("https://example.com/exit-63", &dest, 30)
            .expect_err("exit 63 must fail");
        assert!(
            error.message.contains("95 MiB"),
            "unexpected message: {}",
            error.message
        );
        let error = fetch_url("https://example.com/exit-28", &dest, 30)
            .expect_err("exit 28 must fail");
        assert!(
            error.message.contains("timed out"),
            "unexpected message: {}",
            error.message
        );
        let error = fetch_url("https://example.com/exit-9", &dest, 30)
            .expect_err("other exits must fail");
        assert!(
            error.message.contains("curl failed (exit 9)"),
            "unexpected message: {}",
            error.message
        );
        assert!(
            error.message.contains("strange failure"),
            "stderr must be included: {}",
            error.message
        );

        unsafe { env::remove_var(ENV_CURL_OVERRIDE) };
        unsafe { env::remove_var("FAKE_CURL_ROOT") };
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fetch_reports_a_missing_curl_with_its_hint() {
        let _guard = CURL_TEST_LOCK.lock().expect("lock curl env");
        let dir = test_dir("missing-curl");
        unsafe {
            env::set_var(ENV_CURL_OVERRIDE, dir.join("no-such-curl-binary"))
        };
        let error =
            fetch_url("https://example.com/paper.pdf", &dir.join("out"), 30)
                .expect_err("missing curl must fail");
        assert!(
            error.message.contains("not found"),
            "unexpected message: {}",
            error.message
        );
        assert_eq!(
            error.hint.as_deref(),
            Some("install curl or set BOB_HIGHLIGHTS_CURL"),
        );
        unsafe { env::remove_var(ENV_CURL_OVERRIDE) };
        fs::remove_dir_all(&dir).ok();
    }
}

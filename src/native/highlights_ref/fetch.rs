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

use crate::native::env as bob_env;
use std::{
    env, fs,
    net::{IpAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    process::Command,
};

use super::{
    clip_url::{is_non_global_literal, validate_and_clean},
    CommandError,
};

/// Environment override replacing the `curl` program. This is the test
/// seam, like `BOB_PANDOC_COMMAND`.
pub(super) const ENV_CURL_OVERRIDE: &str = "BOB_HIGHLIGHTS_CURL";

/// Environment override replacing DNS for the resolved-address check.
/// Comma-separated `host=ip` pairs with `*` as a wildcard host (for
/// example `example.com=93.184.216.34,*.example.org=93.184.216.34`). When
/// set, it replaces DNS entirely and an unlisted host fails as a network
/// error. This is the test seam; `tests/cli/support.rs::bob_command()`
/// defaults it to a wildcard public address.
pub(super) const ENV_RESOLVE_OVERRIDE: &str = "BOB_HIGHLIGHTS_RESOLVE";

/// Refusal limit for downloaded PDFs, matching the vault-sync cap the
/// clip adapter enforces (`PDF_MAX_BYTES` there).
pub(super) const PDF_MAX_BYTES: u64 = 95 * 1024 * 1024;

/// Maximum redirect hops followed before giving up.
pub(super) const MAX_REDIRECTS: usize = 10;

// Tests below point `BOB_HIGHLIGHTS_CURL` at fake scripts through
// thread-local overrides (see `crate::native::env`), so they need no
// serializing lock: each test's values are invisible to the others.
// The fake script itself runs as a child process and only inherits
// process environment, so `curl_one_hop` forwards the overrides
// explicitly with `crate::native::env::inherit_overrides`.

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
    progress: Option<&dyn Fn(&str)>,
) -> std::result::Result<FetchResult, FetchError> {
    fetch_with_curl(url, dest, max_time_secs, &curl_program(), progress)
}

/// The `curl` program: `BOB_HIGHLIGHTS_CURL` when set and non-empty,
/// otherwise plain `curl` resolved through `PATH`.
fn curl_program() -> String {
    bob_env::var(ENV_CURL_OVERRIDE)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "curl".to_string())
}

fn fetch_with_curl(
    url: &str,
    dest: &Path,
    max_time_secs: u64,
    curl: &str,
    progress: Option<&dyn Fn(&str)>,
) -> std::result::Result<FetchResult, FetchError> {
    let start_host = url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    if let Some(report) = progress {
        report(&format!("fetching {start_host}…"));
    }
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let pin = resolve_host_for_pin(&current)?;
        let (status, content_type, redirect_url) =
            curl_one_hop(curl, &current, dest, max_time_secs, &pin)?;
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

/// The address curl is pinned to for one hop: the hostname, its port,
/// and the checked IP address.
struct ResolvePin {
    host: String,
    port: u16,
    addr: IpAddr,
}

/// Resolve `url`'s host and refuse it when any resolved address is
/// non-global under the same predicate URL validation uses. The returned
/// pin keeps curl on the address that was checked, so DNS cannot change
/// between the check and the fetch.
fn resolve_host_for_pin(
    url: &str,
) -> std::result::Result<ResolvePin, FetchError> {
    let parsed = url::Url::parse(url).map_err(|error| {
        FetchError::new(format!("invalid URL {url:?}: {error}"))
    })?;
    let host = parsed.host_str().unwrap_or_default().to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    let mut addresses = resolve_host_addresses(&host, port)?;
    if addresses.is_empty() {
        return Err(FetchError::new(format!("cannot resolve host: {url}")));
    }
    for addr in &addresses {
        if is_non_global_literal(addr) {
            return Err(FetchError::new(format!(
                "{host} resolves to a private address ({addr})"
            )));
        }
    }
    // Pin to the first checked address; every resolved address above is
    // already known global.
    let addr = addresses.remove(0);
    Ok(ResolvePin { host, port, addr })
}

/// Resolve `host` to its IP addresses: real DNS, or the
/// [`ENV_RESOLVE_OVERRIDE`] table when it is set (which replaces DNS
/// entirely, so an unlisted host fails as a network error).
fn resolve_host_addresses(
    host: &str,
    port: u16,
) -> std::result::Result<Vec<IpAddr>, FetchError> {
    if let Some(table) = bob_env::var(ENV_RESOLVE_OVERRIDE)
        .ok()
        .filter(|value| !value.is_empty())
    {
        let mut wildcard: Option<IpAddr> = None;
        for pair in table.split(',') {
            let (name, ip) = pair.split_once('=').unwrap_or(("", ""));
            let name = name.trim();
            let ip: IpAddr = ip.trim().parse().map_err(|_| {
                FetchError::new(format!("cannot resolve host: {host}"))
            })?;
            if name == "*" {
                wildcard = Some(ip);
            } else if name.eq_ignore_ascii_case(host) {
                return Ok(vec![ip]);
            }
        }
        return wildcard.map(|ip| vec![ip]).ok_or_else(|| {
            FetchError::new(format!("cannot resolve host: {host}"))
        });
    }
    (host, port)
        .to_socket_addrs()
        .map(|addrs| addrs.map(|addr| addr.ip()).collect())
        .map_err(|_| FetchError::new(format!("cannot resolve host: {host}")))
}

/// Run one `curl` hop without `-L` and split its `-w` trailer into
/// `(status, content_type, redirect_url)`.
fn curl_one_hop(
    curl: &str,
    url: &str,
    dest: &Path,
    max_time_secs: u64,
    pin: &ResolvePin,
) -> std::result::Result<(u16, String, String), FetchError> {
    let mut command = Command::new(curl);
    // Tests point the fake curl at per-test directories through
    // thread-local overrides, which the child cannot inherit: forward
    // them explicitly (`FAKE_CURL_ROOT`, `FAKE_CURL_ARGV_LOG`).
    crate::native::env::inherit_overrides(&mut command);
    let output = command
        .arg("-q")
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
        // Pin curl to the address the resolved-address check vetted, so
        // DNS cannot change between the check and the fetch.
        .arg("--resolve")
        .arg(format!("{}:{}:{}", pin.host, pin.port, pin.addr))
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
    /// and the `-o` argument is the destination file. Logs its argv
    /// (one per line) to `$FAKE_CURL_ARGV_LOG` when set.
    fn write_fake_curl(dir: &Path) -> PathBuf {
        let path = dir.join("fake-curl.sh");
        let script = r#"#!/bin/sh
# Fake curl for fetch.rs tests. Serves canned bodies from $ROOT/<name>
# selected by the request URL, prints the -w trailer (status,
# content-type, redirect URL), and exits as told.
ROOT="$FAKE_CURL_ROOT"
if [ -n "$FAKE_CURL_ARGV_LOG" ]; then
  printf '%s\n' "$@" >> "$FAKE_CURL_ARGV_LOG"
fi
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
        let dir = test_dir("cases");
        let fake = write_fake_curl(&dir);
        // Thread-local overrides: parallel tests never observe them. The
        // fake script sees them because `curl_one_hop` forwards overrides
        // to the child process explicitly.
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("FAKE_CURL_ROOT", Some(dir.as_os_str())),
            (ENV_CURL_OVERRIDE, Some(fake.as_os_str())),
            (
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("*=203.0.113.1")),
            ),
        ]);

        // A plain 200 PDF download.
        let dest = dir.join("paper.pdf.out");
        let fetched =
            fetch_url("https://example.com/paper.pdf", &dest, 30, None)
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
        let fetched =
            fetch_url("https://example.com/two-hop-start", &dest, 30, None)
                .expect("follow two-hop redirect");
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.final_url, "https://example.com/two-hop-end?x=1");

        // A relative Location resolves against the current URL.
        let dest = dir.join("relative.pdf.out");
        let fetched =
            fetch_url("https://example.com/relative-start", &dest, 30, None)
                .expect("follow relative redirect");
        assert_eq!(fetched.final_url, "https://example.com/papers/paper.pdf");

        // A redirect to a private host is refused, not fetched.
        let dest = dir.join("private.pdf.out");
        let error =
            fetch_url("https://example.com/private-redirect", &dest, 30, None)
                .expect_err("private redirect must be refused");
        assert!(
            error.message.contains("private"),
            "unexpected message: {}",
            error.message
        );

        // A 404 is returned to the caller for routing.
        let dest = dir.join("missing.out");
        let fetched = fetch_url("https://example.com/missing", &dest, 30, None)
            .expect("404 is returned");
        assert_eq!(fetched.status, 404);
        assert_eq!(fetched.content_type, "text/html; charset=utf-8");

        // Curl exit codes map to their messages.
        let dest = dir.join("exit-63.out");
        let error = fetch_url("https://example.com/exit-63", &dest, 30, None)
            .expect_err("exit 63 must fail");
        assert!(
            error.message.contains("95 MiB"),
            "unexpected message: {}",
            error.message
        );
        let error = fetch_url("https://example.com/exit-28", &dest, 30, None)
            .expect_err("exit 28 must fail");
        assert!(
            error.message.contains("timed out"),
            "unexpected message: {}",
            error.message
        );
        let error = fetch_url("https://example.com/exit-9", &dest, 30, None)
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

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn curl_passes_q_as_first_argument() {
        let dir = test_dir("curl-q");
        let fake = write_fake_curl(&dir);
        let argv_log = dir.join("argv.log");
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("FAKE_CURL_ROOT", Some(dir.as_os_str())),
            (ENV_CURL_OVERRIDE, Some(fake.as_os_str())),
            (
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("*=203.0.113.1")),
            ),
            ("FAKE_CURL_ARGV_LOG", Some(argv_log.as_os_str())),
        ]);

        let dest = dir.join("out.pdf");
        fetch_url("https://example.com/paper.pdf", &dest, 30, None)
            .expect("fetch with fake curl");
        let logged = fs::read_to_string(&argv_log).expect("read argv log");
        let first = logged.lines().next().unwrap_or_default().to_string();
        assert_eq!(
            first, "-q",
            "curl's first argument must be -q so ~/.curlrc cannot inject -L:\n{logged}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fetch_reports_a_missing_curl_with_its_hint() {
        let dir = test_dir("missing-curl");
        let missing = dir.join("no-such-curl-binary");
        let _guard = crate::native::env::TestEnvGuard::set(&[
            (ENV_CURL_OVERRIDE, Some(missing.as_os_str())),
            (
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("*=203.0.113.1")),
            ),
        ]);
        let error = fetch_url(
            "https://example.com/paper.pdf",
            &dir.join("out"),
            30,
            None,
        )
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
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fetch_refuses_private_resolved_addresses_and_unlisted_hosts() {
        let dir = test_dir("resolve-check");
        let fake = write_fake_curl(&dir);
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("FAKE_CURL_ROOT", Some(dir.as_os_str())),
            (ENV_CURL_OVERRIDE, Some(fake.as_os_str())),
        ]);

        // A host resolving to a private address is refused before curl
        // runs, with the resolved address in the message.
        {
            let _table = crate::native::env::TestEnvGuard::set(&[(
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new(
                    "example.com=10.0.0.1,*=203.0.113.1",
                )),
            )]);
            let dest = dir.join("private-resolve.out");
            let error =
                fetch_url("https://example.com/paper.pdf", &dest, 30, None)
                    .expect_err("private resolution must be refused");
            assert!(
                error.message.contains("resolves to a private address")
                    && error.message.contains("10.0.0.1"),
                "unexpected message: {}",
                error.message
            );
        }

        // Mapped private literals are refused by the same predicate.
        {
            let _table = crate::native::env::TestEnvGuard::set(&[(
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("*=::ffff:10.0.0.1")),
            )]);
            let dest = dir.join("private-resolve.out");
            let error =
                fetch_url("https://example.com/paper.pdf", &dest, 30, None)
                    .expect_err("mapped private resolution must be refused");
            assert!(
                error.message.contains("resolves to a private address"),
                "unexpected message: {}",
                error.message
            );
        }

        // When the table is set, an unlisted host fails as a network
        // error without touching DNS.
        {
            let _table = crate::native::env::TestEnvGuard::set(&[(
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("other.example=203.0.113.1")),
            )]);
            let dest = dir.join("private-resolve.out");
            let error =
                fetch_url("https://example.com/paper.pdf", &dest, 30, None)
                    .expect_err("unlisted host must fail");
            assert!(
                error.message.contains("cannot resolve host"),
                "unexpected message: {}",
                error.message
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn curl_pins_the_checked_address_with_resolve() {
        let dir = test_dir("curl-resolve-pin");
        let fake = write_fake_curl(&dir);
        let argv_log = dir.join("argv.log");
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("FAKE_CURL_ROOT", Some(dir.as_os_str())),
            (ENV_CURL_OVERRIDE, Some(fake.as_os_str())),
            ("FAKE_CURL_ARGV_LOG", Some(argv_log.as_os_str())),
            (
                ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("example.com=93.184.216.34")),
            ),
        ]);

        let dest = dir.join("out.pdf");
        fetch_url("https://example.com/paper.pdf", &dest, 30, None)
            .expect("fetch with fake curl");
        let logged = fs::read_to_string(&argv_log).expect("read argv log");
        assert!(
            logged
                .lines()
                .any(|line| line == "example.com:443:93.184.216.34"),
            "curl must be pinned with --resolve to the checked address:\n{logged}"
        );
        assert!(
            logged
                .lines()
                .last()
                .unwrap_or_default()
                .ends_with("/paper.pdf")
                || logged.contains("https://example.com/paper.pdf"),
            "the request URL must still be passed:\n{logged}"
        );

        fs::remove_dir_all(&dir).ok();
    }
}

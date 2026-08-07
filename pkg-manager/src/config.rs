//! Where the package repository lives.
//!
//! The default points at `10.0.2.2`, which is the host under QEMU user
//! networking - convenient for developing on a laptop, useless for an appliance
//! that has been deployed. A real installation talks to a repository on a
//! server somewhere, and it has to be possible to say so without rebuilding the
//! image.
//!
//! Three places are consulted, most specific first:
//!
//! 1. `ZRPKG_REPO_URL` in the environment, for one-off overrides and scripts.
//! 2. `/persistent/etc/zrpkg.conf` on the data disk, which is what the panel
//!    writes and what survives a reboot.
//! 3. `/etc/vakt/zrpkg.conf` baked into the image, for shipping an appliance
//!    that already knows where its repository is.
//!
//! Falling all the way through gives the QEMU default.
//!
//! Note what is *not* on this list: the repository URL is not a trust decision.
//! Packages are verified against `/etc/vakt/trusted.key` whatever server they
//! came from, so pointing this at a hostile mirror gets you failed signature
//! checks, not compromised packages.

use anyhow::{Result, bail};
use std::fmt;
use std::path::{Path, PathBuf};

/// Preferred location: on the data disk, so it survives a reboot and can be
/// written by the panel.
pub const PERSISTENT_CONF: &str = "/persistent/etc/zrpkg.conf";
/// Image-baked default, for an appliance shipped pre-pointed at a repository.
pub const FALLBACK_CONF: &str = "/etc/vakt/zrpkg.conf";
/// The environment variable that overrides both.
pub const REPO_URL_ENV: &str = "ZRPKG_REPO_URL";

/// The QEMU host under user networking, which is where a development
/// repository runs.
pub const DEFAULT_REPO_URL: &str = "http://10.0.2.2:8080";

/// Where a setting came from, so `zrpkg update` can say which file to edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Environment,
    File(PathBuf),
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Environment => write!(f, "${}", REPO_URL_ENV),
            Source::File(path) => write!(f, "{}", path.display()),
            Source::Default => write!(f, "built-in default"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub repo_url: String,
    pub source: Source,
}

/// Resolves the repository URL from the environment, then the config files,
/// then the built-in default.
pub fn load() -> Config {
    load_from(PERSISTENT_CONF, FALLBACK_CONF)
}

/// Split out from [`load`] so the precedence rules are testable without
/// writing to the real system paths.
fn load_from(persistent: &str, fallback: &str) -> Config {
    if let Ok(raw) = std::env::var(REPO_URL_ENV) {
        if let Ok(url) = normalise(&raw) {
            return Config {
                repo_url: url,
                source: Source::Environment,
            };
        }
    }

    for candidate in [persistent, fallback] {
        let path = Path::new(candidate);
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(raw) = parse(&text) else { continue };
        match normalise(&raw) {
            Ok(url) => {
                return Config {
                    repo_url: url,
                    source: Source::File(path.to_path_buf()),
                };
            }
            Err(e) => {
                // A malformed file is worth saying out loud rather than
                // silently falling through to a URL the user did not choose.
                eprintln!("\x1b[1;33m{}: ignoring repo_url ({})\x1b[0m", candidate, e);
            }
        }
    }

    Config {
        repo_url: DEFAULT_REPO_URL.to_string(),
        source: Source::Default,
    }
}

/// Reads `repo_url` out of a `key=value` file. `#` starts a comment, and
/// unknown keys are ignored so the file can carry notes.
pub fn parse(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "repo_url" | "repo" | "url" => return Some(value.to_string()),
            _ => {}
        }
    }
    None
}

/// Checks a URL is one this client can actually fetch from, and strips the
/// trailing slash so `{base}/{name}.zrp` never produces a double slash.
///
/// Rejecting anything but http and https is not pedantry: every other scheme
/// reqwest might accept would be a way to make the package manager read from
/// somewhere the caller did not intend.
pub fn normalise(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');

    if url.is_empty() {
        bail!("the URL is empty");
    }

    let scheme = match url.split_once("://") {
        Some((scheme, rest)) if !rest.is_empty() => scheme.to_ascii_lowercase(),
        _ => bail!("expected a URL like https://packages.example.com"),
    };

    if scheme != "http" && scheme != "https" {
        bail!(
            "'{}' is not a scheme zrpkg can fetch from; use http or https",
            scheme
        );
    }

    Ok(url.to_string())
}

/// Renders the file the panel writes, and that a deployed appliance ships.
pub fn render(repo_url: &str) -> String {
    format!(
        "# Vakt OS package repository.\n\
         # Packages are verified against /etc/vakt/trusted.key whatever server\n\
         # they come from, so this setting decides where to fetch, not what to\n\
         # trust.\n\
         repo_url={}\n",
        repo_url
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `cargo test` runs tests in the same binary on separate threads by
    /// default, and environment variables are process-global - two tests
    /// that touch REPO_URL_ENV concurrently can genuinely race, which is
    /// exactly what the `unsafe` on `env::set_var`/`remove_var` is warning
    /// about. Every test below that sets, clears, or depends on this
    /// variable being unset takes this lock first, so at most one of them
    /// runs at a time.
    static REPO_URL_ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn reads_the_repo_url() {
        assert_eq!(
            parse("repo_url=https://packages.example.com\n").as_deref(),
            Some("https://packages.example.com")
        );
    }

    #[test]
    fn accepts_the_shorter_spellings() {
        assert_eq!(
            parse("url = https://a.example\n").as_deref(),
            Some("https://a.example")
        );
        assert_eq!(
            parse("repo=https://b.example\n").as_deref(),
            Some("https://b.example")
        );
    }

    #[test]
    fn comments_blank_lines_and_unknown_keys_are_ignored() {
        let text = "# the VPS\n\nport=9000\nrepo_url=https://vps.example.com:8443\n";
        assert_eq!(parse(text).as_deref(), Some("https://vps.example.com:8443"));
    }

    #[test]
    fn a_file_with_no_url_yields_nothing() {
        assert_eq!(parse("# nothing here\nrepo_url=\n"), None);
    }

    #[test]
    fn trailing_slashes_are_stripped() {
        assert_eq!(
            normalise("https://packages.example.com/").unwrap(),
            "https://packages.example.com"
        );
        assert_eq!(
            normalise("  http://10.0.2.2:8080///  ").unwrap(),
            "http://10.0.2.2:8080"
        );
    }

    #[test]
    fn a_path_prefix_is_kept() {
        // Serving the repository from a subdirectory of an existing site is a
        // perfectly ordinary way to deploy it.
        assert_eq!(
            normalise("https://example.com/vakt/packages").unwrap(),
            "https://example.com/vakt/packages"
        );
    }

    #[test]
    fn only_http_and_https_are_accepted() {
        assert!(normalise("https://a.example").is_ok());
        assert!(normalise("HTTP://a.example").is_ok());

        for bad in [
            "file:///etc/passwd",
            "ftp://a.example",
            "a.example",
            "",
            "https://",
        ] {
            assert!(normalise(bad).is_err(), "{} should have been rejected", bad);
        }
    }

    #[test]
    fn the_rendered_file_round_trips() {
        let rendered = render("https://packages.example.com");
        assert_eq!(
            parse(&rendered).as_deref(),
            Some("https://packages.example.com")
        );
    }

    #[test]
    fn the_environment_wins_over_everything() {
        let _guard = REPO_URL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var(REPO_URL_ENV, "https://from-the-environment.example/") };
        let config = load_from("/nonexistent/persistent", "/nonexistent/fallback");
        unsafe { std::env::remove_var(REPO_URL_ENV) };

        assert_eq!(config.repo_url, "https://from-the-environment.example");
        assert_eq!(config.source, Source::Environment);
    }

    #[test]
    fn the_persistent_file_wins_over_the_image_default() {
        let _guard = REPO_URL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zrpkg-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let persistent = dir.join("persistent.conf");
        let fallback = dir.join("fallback.conf");
        std::fs::write(&persistent, render("https://vps.example.com")).unwrap();
        std::fs::write(&fallback, render("http://10.0.2.2:8080")).unwrap();

        let config = load_from(persistent.to_str().unwrap(), fallback.to_str().unwrap());
        assert_eq!(config.repo_url, "https://vps.example.com");
        assert_eq!(config.source, Source::File(persistent));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_nothing_configured_the_qemu_host_is_used() {
        let _guard = REPO_URL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let config = load_from("/nonexistent/persistent", "/nonexistent/fallback");
        assert_eq!(config.repo_url, DEFAULT_REPO_URL);
        assert_eq!(config.source, Source::Default);
    }
}

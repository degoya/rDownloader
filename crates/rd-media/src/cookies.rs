//! Materialises an authentication profile's cookies for the tools that want a file (RD-080-04):
//! `yt-dlp --cookies`, `gallery-dl --cookies`; streamlink takes them one by one (RD-1320-03).
//!
//! yt-dlp cannot be handed a cookie jar; it wants a file. That file is the credential, so
//! three properties are non-negotiable and each is enforced here rather than by the caller:
//!
//! * **Only cookies for the target site are written.** The stored profile is scoped to a
//!   host, but a browser export routinely carries rows for analytics and CDN domains that
//!   happened to be in the same jar. Handing those to an extractor would send one site's
//!   session to another. Rows are filtered, not merely trusted, by the rule every cookie
//!   import follows, `rd_http::CookieScope` (RD-120-52): no public suffix, no widening past
//!   the profile's scope, subdomains only when the profile includes them.
//! * **The file is private and short-lived.** It is created with mode `0o600` on Unix,
//!   inside the process's temp directory, and it is deleted when the guard drops — which
//!   covers success, failure and cancellation alike, because the guard lives on the stack
//!   of the whole run.
//! * **The path, not the contents, is what reaches the process table.** Cookie values never
//!   appear in an argument, and the file name carries no site or profile identity.

use std::path::Path;

use chrono::Utc;
use rd_core::{AuthMethod, AuthProfile, AuthProfileSelection, CookieRow};
use rd_db::Database;
use rd_http::CookieScope;
use rd_secrets::SecretStore;
use secrecy::ExposeSecret;
use tempfile::{Builder, NamedTempFile};
use url::Url;

/// A cookie file that deletes itself when dropped.
///
/// Held by value for the duration of a download; every exit path from the runner drops it,
/// including the early `return` the cancellation branch takes.
///
/// `Debug` prints the path only — the contents are the credential.
#[derive(Debug)]
pub struct CookieFile {
    handle: NamedTempFile,
}

impl CookieFile {
    /// Path to hand to `--cookies`.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.handle.path()
    }

    /// Writes `rows`, already scoped by [`rows_for_selection`], as a Netscape cookie file.
    pub fn from_rows(rows: &[CookieRow]) -> Result<Self, CookieError> {
        write_file(&rd_core::to_netscape_file(rows))
    }
}

/// Why cookies could not be materialised.
///
/// Every variant is reported with a stable code and *without* the cookie material; the
/// runner turns these into a `Failure` the UI translates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieError {
    /// The pinned profile is missing, disabled or past its expiry.
    ProfileUnusable,
    /// The profile does not carry cookies (Basic and Bearer cannot be given to yt-dlp).
    NotCookieBased,
    /// The stored blob could not be read from the vault.
    SecretUnavailable,
    /// The blob is not a cookie file or header.
    Malformed,
    /// Parsed fine, but nothing in it applies to this page's host.
    NoCookiesForHost,
    /// The file could not be created.
    WriteFailed,
}

impl CookieError {
    /// Stable error code, following the `<domain>.<subject>_<condition>` convention.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ProfileUnusable => "media.cookie_profile_unusable",
            Self::NotCookieBased => "media.cookie_profile_not_cookies",
            Self::SecretUnavailable => "media.cookie_secret_unavailable",
            Self::Malformed => "media.cookie_profile_malformed",
            Self::NoCookiesForHost => "media.cookie_scope_empty",
            Self::WriteFailed => "media.cookie_write_failed",
        }
    }

    /// English fallback message; the UI translates by `code`.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::ProfileUnusable => "The selected cookie profile is missing, disabled or expired",
            Self::NotCookieBased => "The selected profile does not store browser cookies",
            Self::SecretUnavailable => "The stored cookies could not be read",
            Self::Malformed => "The stored cookies are not a valid cookie file",
            Self::NoCookiesForHost => "The profile has no cookies for this site",
            Self::WriteFailed => "The temporary cookie file could not be written",
        }
    }

    /// The failure a download, a check or a tool run reports; never retried, because a
    /// pinned profile does not repair itself.
    #[must_use]
    pub fn failure(self) -> rd_core::Failure {
        rd_core::Failure::coded(rd_core::FailureKind::Permanent, self.code(), self.message())
    }
}

/// Keeps only the rows that may be sent to `page_url`'s host, each in the form `scope` stores
/// it.
///
/// This is the containment step. It is separated from the I/O so it can be tested directly,
/// because "a foreign domain's cookie was written into the file" is not a failure that shows
/// up in an integration test — it shows up in somebody else's access log.
///
/// The rule is the HTTP engine's, not a copy of it (RD-120-52): a row must pass
/// [`CookieScope::admit`], and one that does is written for the scope's host — never for the
/// parent domain it came with — reaching subdomains only when
/// [`CookieScope::reaches_subdomains`] says so. Header cookies, which `rd_core` binds to the
/// scope host with subdomains, get that same reach. A refused row is dropped rather than
/// refusing the file, because stripping a browser export's foreign rows is this step's job.
#[must_use]
pub fn rows_for_url(rows: Vec<CookieRow>, scope: &CookieScope, page_url: &Url) -> Vec<CookieRow> {
    let Some(host) = page_url.host_str() else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|row| scope.admit(&row.domain).is_ok())
        .map(|row| CookieRow {
            domain: scope.host().to_owned(),
            include_subdomains: scope.reaches_subdomains(),
            ..row
        })
        .filter(|row| row.matches_host(host))
        .collect()
}

/// Builds a private, self-deleting cookie file for `page_url` from `profile`.
///
/// `secret` is the decrypted blob the vault returned for the profile.
pub fn materialize(
    profile: &AuthProfile,
    secret: &secrecy::SecretString,
    page_url: &Url,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<CookieFile, CookieError> {
    CookieFile::from_rows(&scoped_rows(profile, secret, page_url, now)?)
}

/// The rows of `profile` that may be sent to `page_url`, in the form its scope stores them.
fn scoped_rows(
    profile: &AuthProfile,
    secret: &secrecy::SecretString,
    page_url: &Url,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<CookieRow>, CookieError> {
    if !profile.is_usable(now) {
        return Err(CookieError::ProfileUnusable);
    }
    if profile.method != AuthMethod::Cookies {
        return Err(CookieError::NotCookieBased);
    }
    // A header-format blob has no domain of its own; the profile's scope host supplies it,
    // which is the same host the profile was approved for.
    let rows = rd_core::parse_cookie_file(secret.expose_secret(), &profile.scope.host)
        .map_err(|_| CookieError::Malformed)?;
    // The same scope the HTTP engine builds for this profile (`rd-scheduler`'s
    // `assemble_client`).
    let scope = profile
        .scope
        .probe_url()
        .and_then(|url| CookieScope::new(&url, profile.scope.include_subdomains).ok())
        .ok_or(CookieError::ProfileUnusable)?;
    let scoped = rows_for_url(rows, &scope, page_url);
    if scoped.is_empty() {
        return Err(CookieError::NoCookiesForHost);
    }
    Ok(scoped)
}

/// Resolves a job's profile selection into a scoped, self-deleting cookie file for `page_url`.
///
/// The one answer to "which profile, which cookie file" (RD-1320-02), shared by the yt-dlp
/// download, its format query, the LinkGrabber check and the gallery-dl and streamlink runs.
///
/// Returns `Ok(None)` when the job asked for no profile and none matches the page, which is
/// the ordinary case for a public video. A *pinned* profile that cannot be used is an error
/// rather than a silent fallback: the user asked for that session specifically, and quietly
/// downloading the public version of a private page is the kind of "help" that produces a
/// 30-second trailer named like the full episode. A disabled or expired profile is never
/// matched automatically ([`Database::match_auth_profile`]) and refused when pinned.
pub async fn for_selection(
    database: &Database,
    secrets: &SecretStore,
    selection: AuthProfileSelection,
    page_url: &Url,
) -> Result<Option<CookieFile>, CookieError> {
    rows_for_selection(database, secrets, selection, page_url)
        .await?
        .map(|rows| CookieFile::from_rows(&rows))
        .transpose()
}

/// [`for_selection`] before the file is written: the scoped rows, for a caller that also
/// sends them itself (streamlink's thumbnail fetch, RD-1320-03). Never logged, never shown.
pub async fn rows_for_selection(
    database: &Database,
    secrets: &SecretStore,
    selection: AuthProfileSelection,
    page_url: &Url,
) -> Result<Option<Vec<CookieRow>>, CookieError> {
    let (profile, pinned) = match selection {
        AuthProfileSelection::None => return Ok(None),
        AuthProfileSelection::Auto => (
            database
                .match_auth_profile(page_url)
                .await
                .map_err(|_| CookieError::SecretUnavailable)?,
            false,
        ),
        AuthProfileSelection::Pinned(id) => (
            database
                .auth_profile(id)
                .await
                .map_err(|_| CookieError::SecretUnavailable)?,
            true,
        ),
    };
    let Some(profile) = profile else {
        return if pinned {
            Err(CookieError::ProfileUnusable)
        } else {
            Ok(None)
        };
    };
    // An automatically matched Basic/Bearer profile is not an error: it simply has nothing
    // the tools can use. Pinning one is a mistake worth reporting.
    if profile.method != AuthMethod::Cookies && !pinned {
        return Ok(None);
    }
    let Some(reference) = profile.secret_ref.as_deref() else {
        return if pinned {
            Err(CookieError::SecretUnavailable)
        } else {
            Ok(None)
        };
    };
    let secret = secrets
        .get(reference)
        .await
        .map_err(|_| CookieError::SecretUnavailable)?;
    match scoped_rows(&profile, &secret, page_url, Utc::now()) {
        Ok(rows) => Ok(Some(rows)),
        // A profile that merely happens to match and has nothing for this host is not a
        // failure; one the user pinned is.
        Err(CookieError::NoCookiesForHost) if !pinned => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_file(contents: &str) -> Result<CookieFile, CookieError> {
    use std::io::Write;

    // No site or profile name in the prefix: a temp directory is world-readable, and the
    // file name would otherwise say which site the user has a session for.
    let mut builder = Builder::new();
    builder.prefix("rd-cookies-").suffix(".txt");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o600));
    }
    let mut handle = builder.tempfile().map_err(|_| CookieError::WriteFailed)?;
    handle
        .write_all(contents.as_bytes())
        .map_err(|_| CookieError::WriteFailed)?;
    handle.flush().map_err(|_| CookieError::WriteFailed)?;
    Ok(CookieFile { handle })
}

#[cfg(test)]
#[path = "cookies_tests.rs"]
mod tests;

//! A recording's cookie profile (RD-1320-03): the same profile the media download would use,
//! by the same rules (pinned, none, or the one whose scope matches the channel), resolved once
//! per recording or probe in `rd_media::cookie_rows_for`.
//!
//! streamlink is handed the scoped rows as a Netscape cookie file through
//! `--http-cookies-file` (streamlink 8.2.0, 2026-02-09), not as one `--http-cookie name=value`
//! per cookie: an argument list is readable by every process on the machine, where the file
//! is private and deletes itself — the rule yt-dlp's `--cookies` keeps. The file option also
//! respects each cookie's domain, where `--http-cookie` sends every cookie to every host the
//! plugin contacts. The thumbnail fetch sends the rows whose host matches it, no others.

use std::{ffi::OsString, fmt};

use rd_core::{AuthProfileSelection, CookieRow};
use rd_db::Database;
use rd_media::{CookieError, CookieFile};
use rd_secrets::SecretStore;

/// The cookies one recording or probe goes out with; the file lives as long as this value.
pub struct StreamCookies {
    file: CookieFile,
    rows: Vec<CookieRow>,
}

/// The path and the count, never a name or a value: the rows are the credential.
impl fmt::Debug for StreamCookies {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamCookies")
            .field("file", &self.file.path())
            .field("cookies", &self.rows.len())
            .finish()
    }
}

impl StreamCookies {
    /// The cookies `selection` gives `url`, or `None` when no cookie profile applies.
    pub async fn resolve(
        database: &Database,
        secrets: &SecretStore,
        selection: AuthProfileSelection,
        url: &url::Url,
    ) -> Result<Option<Self>, CookieError> {
        let Some(rows) = rd_media::cookie_rows_for(database, secrets, selection, url).await? else {
            return Ok(None);
        };
        Ok(Some(Self::from_rows(rows)?))
    }

    /// Writes `rows`, already scoped to the channel's site, into the private file.
    pub fn from_rows(rows: Vec<CookieRow>) -> Result<Self, CookieError> {
        Ok(Self {
            file: CookieFile::from_rows(&rows)?,
            rows,
        })
    }

    /// streamlink's options for these cookies: the file, by path.
    pub(crate) fn args(&self) -> [OsString; 2] {
        [
            OsString::from("--http-cookies-file"),
            self.file.path().as_os_str().to_owned(),
        ]
    }

    /// The `Cookie` header for `url`, from the rows that may be sent to its host (secure ones
    /// over HTTPS only); `None` when none may.
    pub(crate) fn header_for(&self, url: &str) -> Option<String> {
        let url = url::Url::parse(url).ok()?;
        let host = url.host_str()?;
        let https = url.scheme() == "https";
        let pairs: Vec<String> = self
            .rows
            .iter()
            .filter(|row| row.matches_host(host) && (https || !row.secure))
            .filter(|row| url.path().starts_with(row.path.as_str()))
            .map(|row| format!("{}={}", row.name, row.value))
            .collect();
        (!pairs.is_empty()).then(|| pairs.join("; "))
    }
}

/// The options for `cookies`, or none.
pub(crate) fn cookie_args(cookies: Option<&StreamCookies>) -> Vec<OsString> {
    cookies
        .map(|cookies| cookies.args().to_vec())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use rd_core::CookieRow;

    use super::{StreamCookies, cookie_args};

    fn row(domain: &str, include_subdomains: bool, secure: bool, name: &str) -> CookieRow {
        CookieRow {
            domain: domain.to_owned(),
            include_subdomains,
            path: "/".to_owned(),
            secure,
            expires: 0,
            name: name.to_owned(),
            value: format!("{name}-secret"),
        }
    }

    fn cookies() -> StreamCookies {
        StreamCookies::from_rows(vec![
            row("twitch.tv", true, true, "auth"),
            row("twitch.tv", true, false, "plain"),
        ])
        .expect("cookie file")
    }

    /// The file by path; no cookie name or value on the command line or in a log line.
    #[test]
    fn streamlink_gets_the_file_and_never_a_value() {
        let cookies = cookies();
        let args: Vec<String> = cookie_args(Some(&cookies))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "--http-cookies-file");
        let contents = std::fs::read_to_string(&args[1]).expect("cookie file");
        assert!(contents.contains("auth-secret"), "{contents}");
        assert!(!args.iter().any(|arg| arg.contains("secret")), "{args:?}");
        let logged = format!("{cookies:?}");
        assert!(!logged.contains("secret"), "{logged}");
        assert!(cookie_args(None).is_empty());

        let path = std::path::PathBuf::from(&args[1]);
        drop(cookies);
        assert!(!path.exists(), "the cookie file outlived the recording");
    }

    /// The thumbnail gets the rows its host may receive, a secure one over HTTPS only.
    #[test]
    fn the_thumbnail_header_keeps_to_the_host() {
        let cookies = cookies();
        assert_eq!(
            cookies
                .header_for("https://static.twitch.tv/thumb.jpg")
                .as_deref(),
            Some("auth=auth-secret; plain=plain-secret")
        );
        assert_eq!(
            cookies.header_for("http://twitch.tv/thumb.jpg").as_deref(),
            Some("plain=plain-secret")
        );
        assert_eq!(cookies.header_for("https://cdn.example/thumb.jpg"), None);
        assert_eq!(cookies.header_for("not a url"), None);
    }
}

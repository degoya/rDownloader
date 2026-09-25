use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use reqwest::cookie::Jar;
use url::Url;

/// Why a cookie's domain may not be stored for a scope (RD-120-49).
///
/// Travels inside the `anyhow::Error` of an import, so a handler can turn it into its own
/// stable code with `downcast_ref`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CookieDomainRefused {
    /// A public suffix such as `com` or `co.uk`: stored with it, the cookie would go to every
    /// site registered below it.
    #[error("cookie domain is a public suffix")]
    PublicSuffix,
    /// Neither the scope's host nor a domain above it.
    #[error("cookie domain is outside the account scope")]
    OutsideScope,
}

/// The hosts an imported cookie may reach: the scope's host, plus its subdomains when the
/// scope includes them — and nothing wider, whatever domain the cookie arrived with.
#[derive(Clone, Debug)]
pub struct CookieScope {
    url: Url,
    host: String,
    include_subdomains: bool,
}

impl CookieScope {
    /// A scope as a profile declares it: `url`'s host, with or without its subdomains.
    pub fn new(url: &Url, include_subdomains: bool) -> Result<Self> {
        let host = url
            .host_str()
            .context("cookie scope has no host")?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty() {
            bail!("cookie scope has no host");
        }
        Ok(Self {
            url: url.clone(),
            host,
            include_subdomains,
        })
    }

    /// A provider's `cookie_scope`: its base domain and every subdomain, which is where a
    /// hoster keeps its API and download servers (`api-v2.ddownload.com`, `fs7.katfile.biz`).
    pub fn provider(url: &Url) -> Result<Self> {
        Self::new(url, true)
    }

    /// The scope's host, lowercased and without a trailing dot.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The one rule for whether a cookie of `domain` may be stored for this scope; every
    /// import and the browser handover (`rd-api`) ask it.
    ///
    /// The domain must be the scope's host or a domain above it, and a domain above it must
    /// not be a public suffix. The host itself is let through even when it is one, as an
    /// intranet `nas` is: RFC 6265 §5.3 step 5 makes such a cookie host-only, and
    /// [`Self::cookie`] does. An accepted domain above the host never widens the scope:
    /// the cookie is stored for the scope's host, not for the domain it came with.
    pub fn admit(&self, domain: &str) -> Result<(), CookieDomainRefused> {
        let domain = domain.trim_start_matches('.').to_ascii_lowercase();
        if domain == self.host {
            return Ok(());
        }
        if domain.is_empty() || !self.host.ends_with(&format!(".{domain}")) {
            return Err(CookieDomainRefused::OutsideScope);
        }
        if is_public_suffix(&domain) {
            return Err(CookieDomainRefused::PublicSuffix);
        }
        Ok(())
    }

    /// Whether an admitted cookie reaches the host's subdomains as well as the host: only
    /// when the scope includes them and its host is not itself a public suffix. The jar
    /// import below and the yt-dlp cookie file (`rd-media`, RD-120-52) both take a cookie's
    /// reach from here.
    #[must_use]
    pub fn reaches_subdomains(&self) -> bool {
        self.include_subdomains && !is_public_suffix(&self.host)
    }

    /// The `Set-Cookie` form of one admitted cookie. Without `Domain=` the jar keeps a cookie
    /// host-only, so the attribute is written only when [`Self::reaches_subdomains`].
    fn cookie(&self, pair: &str, path: &str, secure: bool) -> String {
        let mut cookie = format!("{pair}; Path={path}");
        if self.reaches_subdomains() {
            cookie.push_str("; Domain=");
            cookie.push_str(&self.host);
        }
        if secure {
            cookie.push_str("; Secure");
        }
        cookie
    }
}

/// Whether `domain` is a public suffix per Mozilla's list, compiled in by the `psl` crate.
/// A name the list does not know counts as its own suffix, so every single label — `com`,
/// `lan`, `localhost` — is one.
fn is_public_suffix(domain: &str) -> bool {
    psl::suffix_str(domain) == Some(domain)
}

/// Imports either Netscape cookie-file content or a browser `Cookie` header into a new jar.
pub fn import_cookie_jar(content: &str, scope: &CookieScope) -> Result<Arc<Jar>> {
    let jar = Arc::new(Jar::default());
    import_into(&jar, content, scope)?;
    Ok(jar)
}

/// Imports cookies into an existing jar, so an account and a domain profile can contribute
/// to the same client without one of them replacing the other's jar.
///
/// Every cookie lands in `scope`: header cookies are bound to its host, Netscape rows must
/// pass [`CookieScope::admit`], and one row that does not refuses the whole set.
pub fn import_into(jar: &Jar, content: &str, scope: &CookieScope) -> Result<()> {
    if content.lines().any(|line| line.split('\t').count() >= 7) {
        import_netscape(jar, content, scope)
    } else {
        import_header(jar, content, scope)
    }
}

/// Earliest expiry across the Netscape rows, used to give an imported browser session the
/// same lifetime the browser gave it. Header-format imports carry no expiry at all.
#[must_use]
pub fn earliest_expiry(content: &str) -> Option<DateTime<Utc>> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            let fields = line.split('\t').collect::<Vec<_>>();
            (fields.len() == 7).then(|| fields[4].parse::<i64>().ok())?
        })
        // A zero expiry marks a session cookie, which has no wall-clock lifetime.
        .filter(|seconds| *seconds > 0)
        .filter_map(DateTime::from_timestamp_secs)
        .min()
}

fn import_netscape(jar: &Jar, content: &str, scope: &CookieScope) -> Result<()> {
    let mut imported = 0_usize;
    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
            continue;
        }
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 7 {
            bail!("invalid Netscape cookie row");
        }
        scope.admit(fields[0])?;
        let secure = fields[3].eq_ignore_ascii_case("true");
        let scheme = if secure { "https" } else { "http" };
        let url = Url::parse(&format!("{scheme}://{}/", scope.host))?;
        let pair = format!("{}={}", fields[5], fields[6]);
        jar.add_cookie_str(&scope.cookie(&pair, fields[2], secure), &url);
        imported += 1;
    }
    if imported == 0 {
        bail!("cookie import contains no cookies");
    }
    Ok(())
}

fn import_header(jar: &Jar, content: &str, scope: &CookieScope) -> Result<()> {
    let content = content
        .trim()
        .strip_prefix("Cookie:")
        .unwrap_or(content.trim());
    let mut imported = 0_usize;
    for pair in content
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        let (name, value) = pair.split_once('=').context("invalid Cookie header pair")?;
        if name.is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            bail!("invalid Cookie header value");
        }
        jar.add_cookie_str(
            &scope.cookie(&format!("{name}={value}"), "/", false),
            &scope.url,
        );
        imported += 1;
    }
    if imported == 0 {
        bail!("cookie import contains no cookies");
    }
    Ok(())
}

#[cfg(test)]
#[path = "cookies_tests.rs"]
mod tests;

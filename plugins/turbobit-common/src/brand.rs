//! The parameters that tell the two brands apart.

/// The shape of a file id.
///
/// Turbobit ids are 10 to 12 lowercase letters and digits; HitFile ids are 4 to 7 letters and
/// digits in either case, and the case matters (`0ZGT`, `Uw1TVhP`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdRule {
    pub min: usize,
    pub max: usize,
    pub lowercase_only: bool,
}

impl IdRule {
    /// Whether `candidate` has the shape of a file id — length and alphabet, nothing else.
    #[must_use]
    pub fn accepts(&self, candidate: &str) -> bool {
        let length = candidate.len();
        length >= self.min
            && length <= self.max
            && candidate.bytes().all(|byte| {
                byte.is_ascii_digit()
                    || byte.is_ascii_lowercase()
                    || (!self.lowercase_only && byte.is_ascii_uppercase())
            })
    }
}

/// The plugin's own failure codes, in its own namespace.
///
/// Spelled by the plugin (`turbobit.file_unavailable`, `hitfile.file_unavailable`) and
/// translated by its catalogue; this crate only chooses which one applies. One field per
/// refusal the API is known to give, so a new kind of refusal is a new field here and a new
/// line in four catalogues, never a code invented at the site of the failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Codes {
    /// The URL is not a file link of this brand.
    pub unsupported_link: &'static str,
    /// The URL does not parse.
    pub invalid_link: &'static str,
    /// `/download/folder/<n>`: a folder, which needs a crawler.
    pub folder_not_file: &'static str,
    /// The API answered with something other than the expected JSON; carries `field`.
    pub invalid_response: &'static str,
    /// An HTTP status no branch below explains; carries `status`.
    pub http_error: &'static str,
    /// An `error_name` this crate does not know; carries the sanitised `code`.
    pub api_error: &'static str,
    /// `429`, the operator's `X-RateLimit-Limit: 30` window.
    pub rate_limited: &'static str,
    /// The file is deleted or was never there.
    pub file_unavailable: &'static str,
    /// The file downloads with a premium account only.
    pub premium_only: &'static str,
    /// This IP may not start another free download yet; carries `wait_seconds`.
    pub free_limit_reached: &'static str,
    /// The Turnstile answer was refused twice.
    pub captcha_rejected: &'static str,
    /// `free/start` answered without a usable `downloadUrl`.
    pub no_direct_link: &'static str,
    /// The `downloadUrl` did not parse; carries `error`.
    pub invalid_url: &'static str,
    /// The request carried no account, or the account holds no password.
    pub account_missing: &'static str,
    /// The API answered `401 Unauthenticated` to a call made on the account's session.
    pub not_signed_in: &'static str,
    /// The site refused the stored e-mail address or password.
    pub login_failed: &'static str,
    /// The site refused the sign-in's captcha answer.
    pub login_captcha: &'static str,
    /// The account is locked; carries `until` when the site states it.
    pub account_banned: &'static str,
    /// A premium session that received no direct link: the account's daily limit.
    pub premium_limit_reached: &'static str,
}

/// One brand of the operator's two.
#[derive(Clone, Copy, Debug)]
pub struct Brand {
    /// Shown in English fallback texts: `Turbobit`, `HitFile`.
    pub name: &'static str,
    /// The site the links point at and the direct link redirects through.
    pub site_host: &'static str,
    /// Where the JSON API lives: `app.<site_host>`.
    pub app_host: &'static str,
    /// Every host a link may carry, the main one first. Short domains are only ever rewritten,
    /// never fetched.
    pub match_hosts: &'static [&'static str],
    pub id: IdRule,
    /// Whether a bare `/<id>` path is a file link (HitFile) or not (Turbobit).
    pub bare_id_path: bool,
    /// Whether the canonical link `links/check` accepts ends in `.html` (Turbobit) or must
    /// not (HitFile).
    pub html_suffix: bool,
    /// The `{{secret:…}}` reference of the account password.
    pub password_reference: &'static str,
    pub codes: Codes,
}

impl Brand {
    /// `https://<site>/`.
    #[must_use]
    pub fn site_url(&self) -> String {
        format!("https://{}/", self.site_host)
    }

    /// `https://app.<site>/api/<path>`.
    #[must_use]
    pub fn api_url(&self, path: &str) -> String {
        format!("https://{}/api/{path}", self.app_host)
    }

    /// The link `links/check` accepts for `id`: with `.html` for Turbobit, without for HitFile,
    /// where the `.html` form is answered as `invalid`.
    #[must_use]
    pub fn canonical_link(&self, id: &str) -> String {
        if self.html_suffix {
            format!("https://{}/{id}.html", self.site_host)
        } else {
            format!("https://{}/{id}", self.site_host)
        }
    }

    /// The page the free download's Turnstile sits on.
    #[must_use]
    pub fn free_page(&self, id: &str) -> String {
        format!("https://{}/download/free/{id}", self.site_host)
    }

    /// The page a browser is on when it follows the direct link; the transfer's `Referer`.
    #[must_use]
    pub fn started_page(&self, id: &str) -> String {
        format!("https://{}/download/started/{id}", self.site_host)
    }

    /// The page the login's Turnstile sits on.
    #[must_use]
    pub fn login_page(&self) -> String {
        format!("https://{}/login", self.site_host)
    }

    /// Whether `host` is the site or one of its delivery subdomains — where a direct link may
    /// point and nowhere else.
    #[must_use]
    pub fn owns_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        host == self.site_host
            || host
                .strip_suffix(self.site_host)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }

    /// Whether `host` is one of the hosts a link may carry.
    #[must_use]
    pub fn claims_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.match_hosts.iter().any(|known| *known == host)
    }
}

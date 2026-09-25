//! The stable failure codes of this plugin, translated in `locales/*.json`.
//!
//! One line per refusal the shared flow can report; the English fallback text is formatted by
//! the shared crate with the brand's name, and the catalogue carries the translations.

/// The URL is not a HitFile file link.
pub(crate) const UNSUPPORTED_LINK: &str = "hitfile.unsupported_link";
/// The URL does not parse.
pub(crate) const INVALID_LINK: &str = "hitfile.invalid_link";
/// A folder link, which a resolver cannot take.
pub(crate) const FOLDER_NOT_FILE: &str = "hitfile.folder_not_file";
/// The API answered with something other than the expected JSON; carries `field`.
pub(crate) const INVALID_RESPONSE: &str = "hitfile.invalid_response";
/// An HTTP status nothing else explains; carries `status`.
pub(crate) const HTTP_ERROR: &str = "hitfile.http_error";
/// An `error_name` the plugin does not know; carries the sanitised `code`.
pub(crate) const API_ERROR: &str = "hitfile.api_error";
/// The API's rate limit.
pub(crate) const RATE_LIMITED: &str = "hitfile.rate_limited";
/// The file is deleted or was never there.
pub(crate) const FILE_UNAVAILABLE: &str = "hitfile.file_unavailable";
/// The file downloads with a premium account only.
pub(crate) const PREMIUM_ONLY: &str = "hitfile.premium_only";
/// This IP may not start another free download yet; carries `wait_seconds`.
pub(crate) const FREE_LIMIT_REACHED: &str = "hitfile.free_limit_reached";
/// The captcha answer was refused twice.
pub(crate) const CAPTCHA_REJECTED: &str = "hitfile.captcha_rejected";
/// The site handed out no usable download link.
pub(crate) const NO_DIRECT_LINK: &str = "hitfile.no_direct_link";
/// The download link does not parse; carries `error`.
pub(crate) const INVALID_URL: &str = "hitfile.invalid_url";
/// The account holds no password.
pub(crate) const ACCOUNT_MISSING: &str = "hitfile.account_missing";
/// The API refused a call on the account's session as not signed in.
pub(crate) const NOT_SIGNED_IN: &str = "hitfile.not_signed_in";
/// The site refused the e-mail address or password.
pub(crate) const LOGIN_FAILED: &str = "hitfile.login_failed";
/// The site refused the sign-in's captcha answer.
pub(crate) const LOGIN_CAPTCHA: &str = "hitfile.login_captcha";
/// The account is locked; carries `until` when the site states it.
pub(crate) const ACCOUNT_BANNED: &str = "hitfile.account_banned";
/// A premium session that received no download link.
pub(crate) const PREMIUM_LIMIT_REACHED: &str = "hitfile.premium_limit_reached";

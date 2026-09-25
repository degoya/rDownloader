//! The stable failure codes of this plugin, translated in `locales/*.json`.
//!
//! One line per refusal the shared flow can report; the English fallback text is formatted by
//! the shared crate with the brand's name, and the catalogue carries the translations.

/// The URL is not a Turbobit file link.
pub(crate) const UNSUPPORTED_LINK: &str = "turbobit.unsupported_link";
/// The URL does not parse.
pub(crate) const INVALID_LINK: &str = "turbobit.invalid_link";
/// A folder link, which a resolver cannot take.
pub(crate) const FOLDER_NOT_FILE: &str = "turbobit.folder_not_file";
/// The API answered with something other than the expected JSON; carries `field`.
pub(crate) const INVALID_RESPONSE: &str = "turbobit.invalid_response";
/// An HTTP status nothing else explains; carries `status`.
pub(crate) const HTTP_ERROR: &str = "turbobit.http_error";
/// An `error_name` the plugin does not know; carries the sanitised `code`.
pub(crate) const API_ERROR: &str = "turbobit.api_error";
/// The API's rate limit.
pub(crate) const RATE_LIMITED: &str = "turbobit.rate_limited";
/// The file is deleted or was never there.
pub(crate) const FILE_UNAVAILABLE: &str = "turbobit.file_unavailable";
/// The file downloads with a premium account only.
pub(crate) const PREMIUM_ONLY: &str = "turbobit.premium_only";
/// This IP may not start another free download yet; carries `wait_seconds`.
pub(crate) const FREE_LIMIT_REACHED: &str = "turbobit.free_limit_reached";
/// The captcha answer was refused twice.
pub(crate) const CAPTCHA_REJECTED: &str = "turbobit.captcha_rejected";
/// The site handed out no usable download link.
pub(crate) const NO_DIRECT_LINK: &str = "turbobit.no_direct_link";
/// The download link does not parse; carries `error`.
pub(crate) const INVALID_URL: &str = "turbobit.invalid_url";
/// The account holds no password.
pub(crate) const ACCOUNT_MISSING: &str = "turbobit.account_missing";
/// The API refused a call on the account's session as not signed in.
pub(crate) const NOT_SIGNED_IN: &str = "turbobit.not_signed_in";
/// The site refused the e-mail address or password.
pub(crate) const LOGIN_FAILED: &str = "turbobit.login_failed";
/// The site refused the sign-in's captcha answer.
pub(crate) const LOGIN_CAPTCHA: &str = "turbobit.login_captcha";
/// The account is locked; carries `until` when the site states it.
pub(crate) const ACCOUNT_BANNED: &str = "turbobit.account_banned";
/// A premium session that received no download link.
pub(crate) const PREMIUM_LIMIT_REACHED: &str = "turbobit.premium_limit_reached";

//! The brand, and the five entry points both adapters delegate to.

use plugin_common::{Account, CheckInput, Failure, LinkCheck, PluginHost, ResolveInput, Resolved};
use rd_plugin_turbobit_common::{Brand, Codes, IdRule};

use crate::messages;

/// HitFile, as the shared flow needs to know it. The hosts are the live ones measured on
/// 2026-09-21 (job file `103-21`); the short domains are claimed and rewritten, never fetched.
pub const BRAND: Brand = Brand {
    name: "HitFile",
    site_host: "hitfile.net",
    app_host: "app.hitfile.net",
    match_hosts: &[
        "hitfile.net",
        "www.hitfile.net",
        "new.hitfile.net",
        "hitfile.ru",
        "hil.to",
        "hitf.cc",
        "htfl.net",
        "hitf.to",
    ],
    id: IdRule {
        min: 4,
        max: 7,
        lowercase_only: false,
    },
    bare_id_path: true,
    html_suffix: false,
    password_reference: "hitfile_password",
    codes: Codes {
        unsupported_link: messages::UNSUPPORTED_LINK,
        invalid_link: messages::INVALID_LINK,
        folder_not_file: messages::FOLDER_NOT_FILE,
        invalid_response: messages::INVALID_RESPONSE,
        http_error: messages::HTTP_ERROR,
        api_error: messages::API_ERROR,
        rate_limited: messages::RATE_LIMITED,
        file_unavailable: messages::FILE_UNAVAILABLE,
        premium_only: messages::PREMIUM_ONLY,
        free_limit_reached: messages::FREE_LIMIT_REACHED,
        captcha_rejected: messages::CAPTCHA_REJECTED,
        no_direct_link: messages::NO_DIRECT_LINK,
        invalid_url: messages::INVALID_URL,
        account_missing: messages::ACCOUNT_MISSING,
        not_signed_in: messages::NOT_SIGNED_IN,
        login_failed: messages::LOGIN_FAILED,
        login_captcha: messages::LOGIN_CAPTCHA,
        account_banned: messages::ACCOUNT_BANNED,
        premium_limit_reached: messages::PREMIUM_LIMIT_REACHED,
    },
};

/// Whether this plugin claims `url`.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    rd_plugin_turbobit_common::matches(&BRAND, url)
}

/// The hosts this brand serves; a hoster's catalogue does not depend on the account.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(rd_plugin_turbobit_common::hosters(&BRAND))
}

pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    rd_plugin_turbobit_common::check_account(&BRAND, host, account_id).await
}

pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    rd_plugin_turbobit_common::resolve(&BRAND, host, request).await
}

pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    rd_plugin_turbobit_common::check(&BRAND, host, request).await
}

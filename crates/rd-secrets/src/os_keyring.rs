//! The vault master key's entry in the OS keyring, read without waiting on a prompt nobody sees
//! (RD-1200-02).
//!
//! On macOS the keychain item a previous build created trusts that build's code signature only.
//! An ad-hoc signed upgrade made the keychain ask the user before handing the key out, and a
//! service started by launchd has no one to ask: it waited forever, listening on nothing,
//! logging nothing and holding the data directory's lock. Without a terminal the read now runs
//! with the keychain's user interaction switched off, and the refusal that brings is an error
//! of its own -- never "no entry", which would mint a new key and leave every credential in the
//! vault undecryptable. Linux and Windows read as before.
//!
//! macOS names that refusal two ways (RD-1230-01): `errSecInteractionNotAllowed` when the item
//! would have asked, and `errSecAuthFailed` when its access control rejects the new build
//! outright (`brew upgrade` on macos-15: "The user name or passphrase you entered is not
//! correct."). Without interaction no one entered anything, so both are the refusal. With a
//! terminal `errSecAuthFailed` stays the general error: there a person declined or typed a wrong
//! password, and "no one to ask" would be false.

use anyhow::{Context, Result};

use crate::{KEYRING_SERVICE, KEYRING_USER};

/// `errSecInteractionNotAllowed`: the keychain wanted to ask the user and was not allowed to.
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

/// `errSecAuthFailed`: the item's access control rejected this process, or a password was wrong.
const ERR_SEC_AUTH_FAILED: i32 = -25293;

/// The stable code a start that met [`KeyringInteractionRefused`] ends with.
pub const KEYRING_INTERACTION_REFUSED: &str = "keychain_interaction_refused";

/// The OS keyring holds the vault master key but hands it out only after asking the user, and
/// this process has no one to ask. The vault was not touched.
#[derive(Debug)]
pub struct KeyringInteractionRefused;

impl std::fmt::Display for KeyringInteractionRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{KEYRING_INTERACTION_REFUSED}: the macOS keychain holds the vault master key \
             (item \"{KEYRING_SERVICE}\", account \"{KEYRING_USER}\") but will not hand it to \
             this build without asking, and a background service has no one to ask. Start \
             `rdownloader serve` once in a terminal of the logged-in user and choose \
             \"Always Allow\", or open Keychain Access, find the item \"{KEYRING_SERVICE}\" \
             and allow this rDownloader under Access Control; then start the service again. \
             Nothing in the vault was changed."
        )
    }
}

impl std::error::Error for KeyringInteractionRefused {}

/// The stored master key, base64 as it was written; `None` only when no entry exists.
pub(crate) fn read_master_key(entry: &keyring::Entry) -> Result<Option<String>> {
    // The guard lives until the read below returned.
    #[cfg(target_os = "macos")]
    let no_prompt = no_prompt_without_terminal()?;
    #[cfg(target_os = "macos")]
    let prompts_off = no_prompt.is_some();
    #[cfg(not(target_os = "macos"))]
    let prompts_off = false;
    classify(entry.get_password(), prompts_off, platform_status)
}

/// Switches the keychain's prompts off until the guard drops, unless a terminal is attached:
/// a start from one is the way out [`KeyringInteractionRefused`] names, so it may still ask.
#[cfg(target_os = "macos")]
fn no_prompt_without_terminal()
-> Result<Option<security_framework::os::macos::keychain::KeychainUserInteractionLock>> {
    use std::io::IsTerminal as _;

    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    security_framework::os::macos::keychain::SecKeychain::disable_user_interaction()
        .map(Some)
        .context("switch off the keychain's prompts before reading the vault master key")
}

/// What a keyring read means for the vault. `prompts_off` says the read ran with the keychain's
/// user interaction switched off; `status` is the platform's status code behind an error, where
/// it has one.
fn classify(
    result: keyring::Result<String>,
    prompts_off: bool,
    status: impl Fn(&keyring::Error) -> Option<i32>,
) -> Result<Option<String>> {
    match result {
        Ok(encoded) => Ok(Some(encoded)),
        // The one error that really means "never set, or deleted".
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) if refused(status(&error), prompts_off) => {
            Err(anyhow::Error::new(KeyringInteractionRefused))
        }
        // A locked login session, or a keyring daemon that is not up yet, reports a readable
        // entry as unreadable. Treating that as "no key stored yet" would make the caller mint
        // a fresh master key and overwrite the stored one, leaving every account password,
        // NNTP credential and TOTP seed in the vault permanently undecryptable. Fail startup
        // instead.
        Err(error) => Err(error).context("read vault master key from the OS keyring"),
    }
}

/// Whether `status` is the keychain refusing to hand the key to this build without asking.
fn refused(status: Option<i32>, prompts_off: bool) -> bool {
    match status {
        Some(ERR_SEC_INTERACTION_NOT_ALLOWED) => true,
        Some(ERR_SEC_AUTH_FAILED) => prompts_off,
        _ => false,
    }
}

/// The `OSStatus` behind a keychain error: `apple-native-keyring-store` reports
/// `errSecInteractionNotAllowed` and `errSecAuthFailed` as a platform failure carrying
/// `security-framework`'s error.
#[cfg(target_os = "macos")]
fn platform_status(error: &keyring::Error) -> Option<i32> {
    match error {
        keyring::Error::PlatformFailure(inner) | keyring::Error::NoStorageAccess(inner) => inner
            .downcast_ref::<security_framework::base::Error>()
            .map(|status| status.code()),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_status(_error: &keyring::Error) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        ERR_SEC_AUTH_FAILED, ERR_SEC_INTERACTION_NOT_ALLOWED, KEYRING_INTERACTION_REFUSED,
        KeyringInteractionRefused, classify,
    };

    fn platform_failure() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(std::io::Error::other("keychain")))
    }

    #[test]
    fn a_refused_prompt_is_its_own_error_not_a_missing_entry() {
        let error = classify(Err(platform_failure()), true, |_| {
            Some(ERR_SEC_INTERACTION_NOT_ALLOWED)
        })
        .expect_err("a refused prompt fails the read");
        assert!(error.downcast_ref::<KeyringInteractionRefused>().is_some());
        let message = error.to_string();
        assert!(
            message.starts_with(KEYRING_INTERACTION_REFUSED),
            "{message}"
        );
        assert!(message.contains("Always Allow"), "{message}");
        assert!(message.contains("Keychain Access"), "{message}");
    }

    /// RD-1230-01: after `brew upgrade` the item's access control rejects the new build with
    /// `errSecAuthFailed`. Read without interaction, that is the same refusal.
    #[test]
    fn an_auth_failure_without_prompts_is_the_refusal() {
        for status in [ERR_SEC_INTERACTION_NOT_ALLOWED, ERR_SEC_AUTH_FAILED] {
            let error = classify(Err(platform_failure()), true, |_| Some(status))
                .expect_err("a refusal fails the read");
            assert!(
                error.downcast_ref::<KeyringInteractionRefused>().is_some(),
                "status {status}"
            );
        }
    }

    #[test]
    fn only_a_missing_entry_reads_as_no_key() {
        for prompts_off in [false, true] {
            assert!(
                classify(Err(keyring::Error::NoEntry), prompts_off, |_| None)
                    .expect("no entry")
                    .is_none()
            );
            assert_eq!(
                classify(Ok("a2V5".to_owned()), prompts_off, |_| None).expect("entry"),
                Some("a2V5".to_owned())
            );
        }
        // Any other failure stays the general read error: `errSecAuthFailed` from a read that
        // could ask (a person declined, or typed a wrong password), and an error without a
        // status, or with another one, either way.
        for (status, prompts_off) in [
            (Some(ERR_SEC_AUTH_FAILED), false),
            (None, false),
            (None, true),
            (Some(-25291), true),
        ] {
            let error =
                classify(Err(platform_failure()), prompts_off, |_| status).expect_err("fails");
            assert!(
                error.downcast_ref::<KeyringInteractionRefused>().is_none(),
                "status {status:?}, prompts off {prompts_off}"
            );
            assert!(
                error
                    .to_string()
                    .starts_with("read vault master key from the OS keyring"),
                "{error}"
            );
        }
    }
}

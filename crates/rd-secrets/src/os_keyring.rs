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

use anyhow::{Context, Result};

use crate::{KEYRING_SERVICE, KEYRING_USER};

/// `errSecInteractionNotAllowed`: the keychain wanted to ask the user and was not allowed to.
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

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
             (item \"{KEYRING_SERVICE}\", account \"{KEYRING_USER}\") but asks before it hands \
             it to this build, and a background service has no one to ask. Start \
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
    #[cfg(target_os = "macos")]
    let _no_prompt = no_prompt_without_terminal()?;
    classify(entry.get_password(), platform_status)
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

/// What a keyring read means for the vault. `status` is the platform's status code behind an
/// error, where it has one.
fn classify(
    result: keyring::Result<String>,
    status: impl Fn(&keyring::Error) -> Option<i32>,
) -> Result<Option<String>> {
    match result {
        Ok(encoded) => Ok(Some(encoded)),
        // The one error that really means "never set, or deleted".
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) if status(&error) == Some(ERR_SEC_INTERACTION_NOT_ALLOWED) => {
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

/// The `OSStatus` behind a keychain error: `apple-native-keyring-store` reports
/// `errSecInteractionNotAllowed` as a platform failure carrying `security-framework`'s error.
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
        ERR_SEC_INTERACTION_NOT_ALLOWED, KEYRING_INTERACTION_REFUSED, KeyringInteractionRefused,
        classify,
    };

    fn platform_failure() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(std::io::Error::other("keychain")))
    }

    #[test]
    fn a_refused_prompt_is_its_own_error_not_a_missing_entry() {
        let error = classify(Err(platform_failure()), |_| {
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

    #[test]
    fn only_a_missing_entry_reads_as_no_key() {
        assert!(
            classify(Err(keyring::Error::NoEntry), |_| None)
                .expect("no entry")
                .is_none()
        );
        assert_eq!(
            classify(Ok("a2V5".to_owned()), |_| None).expect("entry"),
            Some("a2V5".to_owned())
        );
        // Any other failure, with another status (`errSecAuthFailed`) or none, stays the
        // general read error.
        for status in [None, Some(-25293)] {
            let error = classify(Err(platform_failure()), |_| status).expect_err("fails");
            assert!(error.downcast_ref::<KeyringInteractionRefused>().is_none());
        }
    }
}

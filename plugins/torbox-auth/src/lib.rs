//! TorBox sign-in: checking the key the person pasted (RD-120-01).
//!
//! The shortest `auth` plugin in the tree, and deliberately so. TorBox has no sign-in flow to
//! run: the API key is on the person's own settings page, they paste it into the account, and
//! there is nothing for a device code or a redirect to obtain. What is left is the question
//! the accounts page actually asks -- *is this key the right one?* -- and answering it needs
//! one request.
//!
//! So `begin` asks `GET /user/me` with the key as a marker and answers `authorized`,
//! `failed`, or `pending` with a wait. `poll` is the same call: the check is idempotent, which
//! is what makes a sign-in started before a restart finish afterwards without remembering
//! anything. Nothing is stored -- `credentials::store-token` is imported by the world and
//! never called, because the credential this flow confirms is the one the person already
//! typed, and writing it back would overwrite their own value with a copy of itself.
//!
//! The sibling plugins, because a manifest carries exactly one `plugin_type`:
//! `plugins/torbox/` is the resolver that carries the `[provider]` row, and
//! `plugins/torbox-jobs/` runs the remote jobs.

pub mod flow;

/// The one refusal this plugin reports, namespaced on its own slug.
///
/// A stable code and not a sentence: the interface translates it, and unlike a provider's
/// refusal there is nothing foreign to quote -- the answer came from TorBox's own status and
/// this plugin decided what it meant.
pub const KEY_INVALID: &str = "torbox_auth.key_invalid";

/// The account holds no key to check.
pub const KEY_MISSING: &str = "torbox_auth.key_missing";

#[cfg(target_arch = "wasm32")]
mod guest;

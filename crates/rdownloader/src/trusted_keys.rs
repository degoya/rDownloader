//! The plugin signing key this build trusts by default.
//!
//! The value itself now lives in `rd_sign::roots`, together with the keys for application
//! updates, the external-tool manifest and plugin repositories — one table, so a rotation is
//! a table entry with an overlap window rather than a constant swap that invalidates every
//! artefact already published. These aliases stay because the CLI and the plugin verifier
//! read them by name.
//!
//! Only public keys are ever compiled in. Generate a pair with `rdownloader plugin keygen`,
//! keep the private PEM outside the repository (CI secret `RDOWNLOADER_PLUGIN_SIGNING_KEY`)
//! and paste the printed base64 public key into `rd_sign::roots::EMBEDDED_KEYS`. An empty
//! entry there disables the default trust entry.

/// Key id referenced by bundled plugin manifests (`key_id`).
pub const RELEASE_KEY_ID: &str = rd_sign::PLUGIN_RELEASE_KEY_ID;

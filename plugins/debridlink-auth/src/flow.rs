//! Reading a device-flow answer.
//!
//! The reading itself — the device code, a poll's outcome, which refusal a provider's `error`
//! is and what of it is safe to repeat — is `plugin_common::device_flow`, shared with every
//! plugin that signs in the same way (RD-191-07). What is this provider's own is the client id
//! below; the endpoints and the form the guest posts are in `guest`. The translation codes a
//! refusal is reported under carry this plugin's slug, `debridlink_auth`, and its catalogue in
//! `locales/` has to carry each of them.

pub use plugin_common::device_flow::{
    DeviceCode, PollOutcome, UNREADABLE, device_code, flow_state, poll, read_flow_state,
    refusal_code, sanitize_error,
};

/// The client id the provider issued for rDownloader. Public by design in a device flow: it
/// identifies the application, not the person, and cannot authorise anything on its own.
pub const CLIENT_ID: &str = "cvE7ck1s0lRJTfWkPGmyDA";

#[cfg(test)]
mod tests {
    use super::{DeviceCode, device_code};

    /// This provider's own answer, escaped slashes and all, reads whole through the shared
    /// reader.
    #[test]
    fn the_providers_device_code_answer_is_read_whole() {
        let started = r#"{
          "device_code": "abc123",
          "user_code": "WXYZ-1234",
          "verification_url": "https:\/\/debrid-link.com\/webapp\/authorize",
          "expires_in": 600,
          "interval": 5
        }"#;
        assert_eq!(
            device_code(started),
            Some(DeviceCode {
                device_code: "abc123".to_owned(),
                user_code: "WXYZ-1234".to_owned(),
                verification_url: "https://debrid-link.com/webapp/authorize".to_owned(),
                expires_in: Some(600),
                interval: Some(5),
            })
        );
    }
}

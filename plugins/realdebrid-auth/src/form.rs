//! The token request's `application/x-www-form-urlencoded` body.
//!
//! Real-Debrid's `/oauth/v2/token` reads `client_id`, `client_secret`, `code` and `grant_type`
//! from the POST body and nowhere else: the same four in the query string are answered with
//! `parameter_missing` (1.5.1 sent them there, and no sign-in ever finished).
//!
//! A body is only half this plugin's to write. The personal client and the refresh material
//! travel as `{{secret:…}}` markers, and the host finds a marker in a form body only as it
//! stands -- percent-encoded here, `{{` would reach it as `%7B%7B` and be sent to the provider
//! as those characters. So a marker is left alone and the host percent-encodes what it fills in;
//! every value this plugin holds itself is encoded here.

use plugin_common::percent_encode as encode;

/// One field of the body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field<'a> {
    /// A value this plugin holds, percent-encoded here.
    Value(&'a str),
    /// A `{{secret:…}}` marker, left as it stands for the host to fill and encode.
    Marker(&'a str),
}

/// The body for `fields`, in the order given.
#[must_use]
pub fn body(fields: &[(&str, Field<'_>)]) -> String {
    fields
        .iter()
        .map(|(name, field)| {
            let value = match field {
                Field::Value(value) => encode(value),
                Field::Marker(marker) => (*marker).to_owned(),
            };
            format!("{}={value}", encode(name))
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sign-in's exchange as it leaves the plugin: the device code and the grant type
    /// encoded, the two markers untouched for the host.
    #[test]
    fn the_sign_in_body_encodes_values_and_keeps_markers() {
        let body = body(&[
            (
                "client_id",
                Field::Marker("{{secret:realdebrid_client_id}}"),
            ),
            (
                "client_secret",
                Field::Marker("{{secret:realdebrid_client_secret}}"),
            ),
            ("code", Field::Value("ABC+def/1=")),
            (
                "grant_type",
                Field::Value("http://oauth.net/grant_type/device/1.0"),
            ),
        ]);
        assert_eq!(
            body,
            "client_id={{secret:realdebrid_client_id}}\
             &client_secret={{secret:realdebrid_client_secret}}\
             &code=ABC%2Bdef%2F1%3D\
             &grant_type=http%3A%2F%2Foauth.net%2Fgrant_type%2Fdevice%2F1.0"
        );
    }

    /// A value holding the separators of the format cannot split into fields of its own.
    #[test]
    fn a_value_cannot_add_a_field() {
        assert_eq!(
            body(&[("code", Field::Value("x&grant_type=other y"))]),
            "code=x%26grant_type%3Dother%20y"
        );
    }
}

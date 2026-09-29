//! A scaffold storage destination. It compiles, packages and passes conformance as it is.
//!
//! It uploads to a server that takes a file in pieces — open an upload, send each chunk at its
//! offset, complete it — which is the shape that makes an upload resumable. Four requests,
//! against the destination address the person configured:
//!
//! | Request | Answer |
//! | --- | --- |
//! | `POST <destination>/uploads?name=<file>&size=<bytes>` | `{"id": "<upload>"}` |
//! | `PUT <destination>/uploads/<upload>?offset=<bytes>` with a chunk | any 2xx |
//! | `POST <destination>/uploads/<upload>/complete` | `{"id": "<file>"}` |
//! | `GET <destination>/files/<file>` (from `verify`) | `{"size": <bytes>}`, or 404 |
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - **Nothing local is deleted until `verify` says the destination holds the object.** That is
//!   why the check is a call of its own rather than something `put` reports: an upload that
//!   returned success and lost the file would otherwise take the only copy with it.
//! - **You read the package through a handle, never a path**, and only the file the job names.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// An upload in progress: the checkpoint, in memory.
///
/// The server's upload id *and* the offset. A stopped upload that remembered only the offset
/// would have to open a second upload and send everything again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    pub id: String,
    pub offset: u64,
}

impl Session {
    /// The checkpoint the host stores: `<id> <offset>`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        format!("{} {}", self.id, self.offset).into_bytes()
    }

    /// A checkpoint read back, or `None` when there is none or it is not one of ours — the
    /// upload then starts over rather than guessing.
    #[must_use]
    pub fn from_bytes(bytes: Option<&[u8]>) -> Option<Self> {
        let text = std::str::from_utf8(bytes?).ok()?;
        let (id, offset) = text.split_once(' ')?;
        Some(Self {
            id: plain_id(id)?.to_owned(),
            offset: offset.parse().ok()?,
        })
    }
}

/// The configured destination as a base address, or `None` when it is not one.
///
/// `https` only: the destination is typed by a person, and a password sent over plain HTTP is
/// a password sent to everybody on the way.
#[must_use]
pub fn base(destination: &str) -> Option<&str> {
    let base = destination.trim().trim_end_matches('/');
    let host = base.strip_prefix("https://")?;
    (!host.is_empty()).then_some(base)
}

/// The id in a server answer — `{"id": "..."}` — if it is a plain one.
///
/// It goes into the next request's path, so an id carrying `/` or `?` is refused rather than
/// followed somewhere the person never configured.
#[must_use]
pub fn answered_id(body: &str) -> Option<String> {
    let value = after_field(body, "id")?.strip_prefix('"')?;
    let (id, _) = value.split_once('"')?;
    plain_id(id).map(str::to_owned)
}

/// The size in a `verify` answer — `{"size": 123}`.
#[must_use]
pub fn answered_size(body: &str) -> Option<u64> {
    let value = after_field(body, "size")?;
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    value[..end].parse().ok()
}

fn plain_id(id: &str) -> Option<&str> {
    let plain = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then_some(id)
}

/// What follows `"name":` in a flat JSON object. Enough for answers this small; a server with
/// richer answers deserves a real reader.
fn after_field<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let rest = &body[body.find(&needle)? + needle.len()..];
    Some(rest.trim_start().strip_prefix(':')?.trim_start())
}

#[cfg(test)]
mod tests {
    use super::{Session, answered_id, answered_size, base};

    #[test]
    fn a_checkpoint_carries_the_upload_and_the_offset() {
        let session = Session {
            id: "u-17".to_owned(),
            offset: 262_144,
        };
        assert_eq!(
            Session::from_bytes(Some(session.to_bytes().as_slice())),
            Some(session)
        );
        assert_eq!(Session::from_bytes(None), None);
        assert_eq!(Session::from_bytes(Some(b"u/../x 5".as_slice())), None);
        assert_eq!(Session::from_bytes(Some(b"u-17".as_slice())), None);
    }

    #[test]
    fn the_destination_is_an_https_address() {
        assert_eq!(
            base(" https://up.example.net/me/ "),
            Some("https://up.example.net/me")
        );
        assert_eq!(base("http://up.example.net"), None);
        assert_eq!(base("https://"), None);
    }

    #[test]
    fn an_id_from_the_server_is_used_only_when_it_is_plain() {
        assert_eq!(answered_id(r#"{"id": "u-17"}"#), Some("u-17".to_owned()));
        assert_eq!(answered_id(r#"{"id":"../admin"}"#), None);
        assert_eq!(answered_id(r#"{"name":"x"}"#), None);
    }

    #[test]
    fn the_verified_size_is_read_as_a_number() {
        assert_eq!(
            answered_size(r#"{"size": 1048576, "name": "a"}"#),
            Some(1_048_576)
        );
        assert_eq!(answered_size(r#"{"size": null}"#), None);
    }
}

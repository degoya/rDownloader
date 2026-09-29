//! A scaffold transfer backend. It compiles, packages and passes conformance as it is.
//!
//! It speaks a deliberately small line protocol over TLS, so that what you read is the
//! contract rather than a protocol's parsing — connect, resume from what the host already has,
//! write through the host's sink, stop on request:
//!
//! | Sent | Answer |
//! | --- | --- |
//! | `HEAD <path>` | `OK <size> [<last-modified>]`, or anything else for "no" |
//! | `GET <path> <offset>` | `OK <size>`, then the file's bytes from `offset` to the end |
//!
//! Addresses look like `{{PLUGIN_SLUG}}://files.example.net:990/path/to/file`; the scheme is
//! `[transfer] schemes` in `manifest.toml`. Replace this module with your protocol.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You move bytes; the application decides where they land and whether they count.** You
//!   never see a path, you never promote a file, and the length is verified without you.
//! - **`sink.committed()` is the truth about what reached the disk.** Resume from it, not from
//!   your checkpoint: the checkpoint is your notes, and it may lag behind after a crash.
//! - **A connection is a number the host owns.** Every socket is closed when the invocation
//!   ends, so a leaked handle costs nothing — but closing early frees a slot.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// The scheme this backend carries, as `manifest.toml` declares it.
pub const SCHEME: &str = "{{PLUGIN_SLUG}}";
/// The port an address without one is fetched from. Must be in `[capabilities.net_stream]`.
pub const DEFAULT_PORT: u16 = 990;

/// Where a file lives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Target {
    /// `<scheme>://host[:port]/path`, or `None` for anything else.
    ///
    /// The path goes into a protocol line, so a space or a line break in it is refused: it
    /// would end the command early and let an address smuggle in a second one.
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
        let (authority, path) = rest.split_once('/')?;
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, DEFAULT_PORT),
        };
        if host.is_empty() || path.is_empty() || path.contains([' ', '\r', '\n']) {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            port,
            path: format!("/{path}"),
        })
    }
}

/// The server's answer to a command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Head {
    Ok {
        size: u64,
        /// RFC 3339, as the server said it; the host compares it verbatim on every resume.
        last_modified: Option<String>,
    },
    /// The server said no.
    Refused,
    /// The server said something this backend does not understand.
    Unreadable,
}

/// Reads one reply line.
#[must_use]
pub fn parse_head(line: &str) -> Head {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("OK") {
        return Head::Refused;
    }
    match parts.next().and_then(|size| size.parse().ok()) {
        Some(size) => Head::Ok {
            size,
            last_modified: parts.next().map(str::to_owned),
        },
        None => Head::Unreadable,
    }
}

/// Takes the first line off `buffer`, without its `\n`, and leaves what came after it.
///
/// What came after it matters: the reply line and the first bytes of the file often arrive in
/// the same read. Dropping them loses the start of every file — the classic line-protocol bug.
pub fn take_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = buffer.iter().position(|byte| *byte == b'\n')?;
    let rest = buffer.split_off(end + 1);
    let mut line = std::mem::replace(buffer, rest);
    line.pop();
    Some(line)
}

/// The checkpoint the host stores. Opaque to the host, so its shape is this backend's own.
#[must_use]
pub fn checkpoint(offset: u64) -> Vec<u8> {
    offset.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PORT, Head, SCHEME, Target, parse_head, take_line};

    #[test]
    fn an_address_names_host_port_and_path() {
        let target = Target::parse(&format!("{SCHEME}://files.example.net:21/a/b.bin"));
        assert_eq!(
            target,
            Some(Target {
                host: "files.example.net".to_owned(),
                port: 21,
                path: "/a/b.bin".to_owned(),
            })
        );
        let default = Target::parse(&format!("{SCHEME}://files.example.net/b.bin"));
        assert_eq!(default.map(|target| target.port), Some(DEFAULT_PORT));
    }

    #[test]
    fn an_address_that_could_smuggle_a_command_is_refused() {
        assert_eq!(Target::parse(&format!("{SCHEME}://h/a\nDELE /b")), None);
        assert_eq!(Target::parse(&format!("{SCHEME}://h/a b")), None);
        assert_eq!(Target::parse(&format!("{SCHEME}://h:99999/a")), None);
        assert_eq!(Target::parse("https://files.example.net/a"), None);
    }

    #[test]
    fn a_reply_is_read_for_size_and_date() {
        assert_eq!(
            parse_head("OK 42 2026-09-28T10:00:00Z"),
            Head::Ok {
                size: 42,
                last_modified: Some("2026-09-28T10:00:00Z".to_owned()),
            }
        );
        assert_eq!(parse_head("NO such file"), Head::Refused);
        assert_eq!(parse_head("OK many"), Head::Unreadable);
    }

    #[test]
    fn bytes_after_the_reply_line_are_kept() {
        let mut buffer = b"OK 5\nhello".to_vec();
        assert_eq!(take_line(&mut buffer), Some(b"OK 5".to_vec()));
        assert_eq!(buffer, b"hello");
        assert_eq!(take_line(&mut buffer), None, "no line break, no line");
    }
}

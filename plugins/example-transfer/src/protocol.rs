//! The backend's line protocol, free of the guest bindings so it is tested on the host
//! (RD-191-07, PLUG-23).
//!
//! What a reply means, what an address names and how a checkpoint is spelled do not need a
//! socket to be right, and before this split nothing checked them outside the contract tests in
//! `rd-plugin-transfer`, which run the whole component against a test server.

/// Where a transfer goes: `example+tcp://host:port/path` or `example+tls://host:port/path`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub path: String,
}

impl Target {
    /// The target an address names, or `None` when it is not one of this backend's.
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let tls = match scheme {
            "example+tcp" => false,
            "example+tls" => true,
            _ => return None,
        };
        let (authority, path) = rest.split_once('/')?;
        let (host, port) = authority.split_once(':')?;
        if host.is_empty() {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            port: port.parse().ok()?,
            tls,
            path: format!("/{path}"),
        })
    }
}

/// Why a reply line was not a usable answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeadError {
    /// The server answered something other than `OK`.
    Refused,
    /// `OK` without a size.
    NoSize,
}

/// `OK <size> [<rfc3339>]`, the whole protocol: the size, and the modification time if sent.
///
/// # Errors
///
/// [`HeadError`] for a refusal or an `OK` without a readable size.
pub fn parse_head(reply: &str) -> Result<(u64, Option<String>), HeadError> {
    let mut parts = reply.split_whitespace();
    if parts.next() != Some("OK") {
        return Err(HeadError::Refused);
    }
    let size = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or(HeadError::NoSize)?;
    Ok((size, parts.next().map(str::to_owned)))
}

/// Takes one reply line out of `buffer`, without its `\n`, and leaves whatever came after it —
/// the first body bytes, when they arrived in the same read — in `buffer`. `None` while no
/// whole line is there yet.
///
/// Keeping the overshoot is the point: dropping it would lose the first chunk of every file,
/// the classic line-protocol bug.
pub fn take_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = buffer.iter().position(|byte| *byte == b'\n')?;
    let rest = buffer.split_off(end + 1);
    let mut line = std::mem::replace(buffer, rest);
    line.pop();
    Some(line)
}

/// The checkpoint is opaque to the host, so its shape is entirely this backend's business:
/// the offset reached, big-endian.
#[must_use]
pub fn checkpoint(offset: u64) -> Vec<u8> {
    offset.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::{HeadError, Target, checkpoint, parse_head, take_line};

    #[test]
    fn both_schemes_are_read_and_nothing_else_is() {
        assert_eq!(
            Target::parse("example+tcp://127.0.0.1:4000/files/a.bin"),
            Some(Target {
                host: "127.0.0.1".to_owned(),
                port: 4000,
                tls: false,
                path: "/files/a.bin".to_owned(),
            })
        );
        assert_eq!(
            Target::parse("example+tls://localhost:443/a").map(|target| target.tls),
            Some(true)
        );
        for foreign in [
            "https://localhost:443/a",
            "example+tcp://localhost/a",
            "example+tcp://localhost:notaport/a",
            "example+tcp://localhost:70000/a",
            "example+tcp://:4000/a",
            "example+tcp://localhost:4000",
            "not an address",
        ] {
            assert_eq!(Target::parse(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn a_head_reply_carries_the_size_and_maybe_a_time() {
        assert_eq!(parse_head("OK 1024"), Ok((1024, None)));
        assert_eq!(
            parse_head("OK 7 2026-10-04T12:00:00Z"),
            Ok((7, Some("2026-10-04T12:00:00Z".to_owned())))
        );
        assert_eq!(parse_head("ERR no such file"), Err(HeadError::Refused));
        assert_eq!(parse_head(""), Err(HeadError::Refused));
        assert_eq!(parse_head("OK"), Err(HeadError::NoSize));
        assert_eq!(parse_head("OK -1"), Err(HeadError::NoSize));
    }

    /// The bytes after the reply line are the start of the file and must survive.
    #[test]
    fn a_reply_line_keeps_the_body_bytes_that_came_with_it() {
        let mut buffer = b"OK 5\nhel".to_vec();
        assert_eq!(take_line(&mut buffer), Some(b"OK 5".to_vec()));
        assert_eq!(buffer, b"hel");

        let mut partial = b"OK 5".to_vec();
        assert_eq!(take_line(&mut partial), None);
        assert_eq!(
            partial, b"OK 5",
            "nothing is consumed before the line is whole"
        );

        let mut empty_line = b"\nrest".to_vec();
        assert_eq!(take_line(&mut empty_line), Some(Vec::new()));
        assert_eq!(empty_line, b"rest");
    }

    #[test]
    fn a_checkpoint_is_the_offset_big_endian() {
        assert_eq!(checkpoint(0), vec![0; 8]);
        assert_eq!(checkpoint(0x0102), vec![0, 0, 0, 0, 0, 0, 1, 2]);
        assert_eq!(
            u64::from_be_bytes(checkpoint(u64::MAX).try_into().expect("eight bytes")),
            u64::MAX
        );
    }
}

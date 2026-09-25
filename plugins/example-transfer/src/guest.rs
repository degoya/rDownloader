//! The component itself: the smallest plugin that exercises the whole contract.
//!
//! It speaks a deliberately trivial line protocol so the test server is a few lines of Tokio
//! rather than a second FTP implementation, and so what the contract tests prove is the
//! contract — connect, resume from a checkpoint, write through the host's sink, stop on
//! request — and not this backend's own parsing.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "transfer-plugin",
});

use exports::rdownloader::plugin::transfer::{Guest, Job, RemoteFile, TransferEnd};
use rdownloader::plugin::{
    host, net, sink,
    types::{Failure, FailureKind},
};

/// Bytes requested per read; small enough that a stop is noticed promptly.
const CHUNK: u32 = 32 * 1024;

struct Component;

impl Guest for Component {
    fn probe(url: String, _credential_ref: Option<String>) -> Result<RemoteFile, Failure> {
        let target = Target::parse(&url)?;
        let mut connection = target.connect()?;
        let head = connection.request(&format!("HEAD {}\n", target.path))?;
        let (size, modified) = parse_head(&head)?;
        Ok(RemoteFile {
            size: Some(size),
            last_modified: modified,
            resumable: true,
        })
    }

    fn run(job: Job) -> TransferEnd {
        match transfer(&job) {
            Ok(end) => end,
            Err(failure) => TransferEnd::Failed(failure),
        }
    }
}

fn transfer(job: &Job) -> Result<TransferEnd, Failure> {
    let target = Target::parse(&job.url)?;
    // The checkpoint is the host's memory of us, not a second source of truth: the sink says
    // how many bytes it actually accepted, and that is where the next read has to start.
    let committed = sink::committed();
    let mut connection = target.connect()?;
    let head = connection.request(&format!("GET {} {}\n", target.path, committed))?;
    let (total, _) = parse_head(&head)?;
    sink::progress(committed, Some(total));

    let mut offset = committed;
    loop {
        if sink::should_stop() {
            sink::sync()?;
            return Ok(TransferEnd::Stopped(checkpoint(offset)));
        }
        let chunk = connection.read(CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        let written = u64::try_from(chunk.len()).unwrap_or_default();
        sink::write_at(offset, &chunk)?;
        offset += written;
        sink::progress(offset, Some(total));
    }
    sink::sync()?;
    if offset < total {
        return Err(failure(
            FailureKind::Transient(None),
            "example.truncated",
            "The server stopped sending before the whole file arrived",
        ));
    }
    Ok(TransferEnd::Complete(Some(checkpoint(offset))))
}

/// The checkpoint is opaque to the host, so its shape is entirely this backend's business.
fn checkpoint(offset: u64) -> Vec<u8> {
    offset.to_be_bytes().to_vec()
}

struct Target {
    host: String,
    port: u16,
    tls: bool,
    path: String,
}

impl Target {
    /// `example+tcp://host:port/path` or `example+tls://host:port/path`.
    fn parse(url: &str) -> Result<Self, Failure> {
        let (scheme, rest) = url.split_once("://").ok_or_else(bad_url)?;
        let tls = match scheme {
            "example+tcp" => false,
            "example+tls" => true,
            _ => return Err(bad_url()),
        };
        let (authority, path) = rest.split_once('/').ok_or_else(bad_url)?;
        let (host, port) = authority.split_once(':').ok_or_else(bad_url)?;
        Ok(Self {
            host: host.to_owned(),
            port: port.parse().map_err(|_| bad_url())?,
            tls,
            path: format!("/{path}"),
        })
    }

    fn connect(&self) -> Result<Connection, Failure> {
        let handle = net::connect(&self.host, self.port, self.tls)?;
        Ok(Connection {
            handle,
            pending: Vec::new(),
        })
    }
}

/// The host's handle for this invocation's socket, plus whatever a reply read overshot into.
struct Connection {
    handle: u32,
    /// Body bytes that arrived in the same read as the reply line. Without this the first
    /// chunk of every file would be read and thrown away — the classic line-protocol bug.
    pending: Vec<u8>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        net::close(self.handle);
    }
}

impl Connection {
    /// Sends one line and reads the single-line reply that answers it.
    fn request(&mut self, line: &str) -> Result<String, Failure> {
        net::write(self.handle, line.as_bytes())?;
        let mut reply = std::mem::take(&mut self.pending);
        let end = loop {
            if let Some(end) = reply.iter().position(|byte| *byte == b'\n') {
                break end;
            }
            let chunk = net::read(self.handle, CHUNK)?;
            if chunk.is_empty() {
                return Err(failure(
                    FailureKind::Transient(None),
                    "example.no_reply",
                    "The server closed the connection without answering",
                ));
            }
            reply.extend_from_slice(&chunk);
        };
        self.pending = reply.split_off(end + 1);
        reply.pop();
        String::from_utf8(reply).map_err(|_| {
            failure(
                FailureKind::Permanent,
                "example.bad_reply",
                "The server reply was not valid UTF-8",
            )
        })
    }

    fn read(&mut self, max: u32) -> Result<Vec<u8>, Failure> {
        if !self.pending.is_empty() {
            let take = self.pending.len().min(max as usize);
            return Ok(self.pending.drain(..take).collect());
        }
        net::read(self.handle, max)
    }
}

/// `OK <size> [<rfc3339>]`, the whole protocol.
fn parse_head(reply: &str) -> Result<(u64, Option<String>), Failure> {
    let mut parts = reply.split_whitespace();
    if parts.next() != Some("OK") {
        host::log("warn", "server refused the request");
        return Err(failure(
            FailureKind::Permanent,
            "example.refused",
            "The server refused the request",
        ));
    }
    let size = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            failure(
                FailureKind::Permanent,
                "example.bad_reply",
                "The server did not report a size",
            )
        })?;
    Ok((size, parts.next().map(str::to_owned)))
}

fn bad_url() -> Failure {
    failure(
        FailureKind::Permanent,
        "example.bad_url",
        "Not an example-transfer URL",
    )
}

fn failure(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

export!(Component);

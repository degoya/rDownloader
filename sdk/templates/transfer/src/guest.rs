//! The component: one connection per call, the file written through the host's sink.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "transfer-plugin",
});

use exports::rdownloader::plugin::transfer::{Guest, Job, RemoteFile, TransferEnd};
use rdownloader::plugin::{
    net, sink,
    types::{Failure, FailureKind},
};

use crate::{Head, Target};

/// Bytes per read. Small enough that a pause is noticed promptly.
const CHUNK: u32 = 32 * 1024;

struct Component;

impl Guest for Component {
    /// Asks the server what is there. Reporting the size lets the application verify the
    /// transfer afterwards; reporting `last-modified` lets it refuse a resume onto a file that
    /// changed underneath.
    fn probe(url: String, _credential_ref: Option<String>) -> Result<RemoteFile, Failure> {
        let target = target(&url)?;
        let mut connection = Connection::open(&target)?;
        let (size, last_modified) = head(&connection.request(&format!("HEAD {}\n", target.path))?)?;
        Ok(RemoteFile {
            size: Some(size),
            last_modified,
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
    let target = target(&job.url)?;
    // Start where the host says the file ends, not where the checkpoint says: the checkpoint
    // is this backend's notes, this is the truth.
    let mut offset = sink::committed();
    let mut connection = Connection::open(&target)?;
    let (total, _) = head(&connection.request(&format!("GET {} {offset}\n", target.path))?)?;
    sink::progress(offset, Some(total));
    loop {
        // Honour this. A backend that ignores it is stopped by its execution budget instead,
        // which loses the chance to write a checkpoint.
        if sink::should_stop() {
            sink::sync()?;
            return Ok(TransferEnd::Stopped(crate::checkpoint(offset)));
        }
        let chunk = connection.read(CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        sink::write_at(offset, &chunk)?;
        offset += chunk.len() as u64;
        sink::progress(offset, Some(total));
    }
    sink::sync()?;
    // A connection that closes early is not a finished file. The host would catch the short
    // length too; saying it here gives the person a reason they can read.
    if offset < total {
        return Err(refuse(
            FailureKind::Transient(None),
            "truncated",
            "the server stopped sending before the whole file arrived",
        ));
    }
    Ok(TransferEnd::Complete(Some(crate::checkpoint(offset))))
}

fn target(url: &str) -> Result<Target, Failure> {
    Target::parse(url).ok_or_else(|| {
        refuse(
            FailureKind::Permanent,
            "bad_url",
            "the address is not one this backend carries",
        )
    })
}

fn head(line: &str) -> Result<(u64, Option<String>), Failure> {
    match crate::parse_head(line) {
        Head::Ok {
            size,
            last_modified,
        } => Ok((size, last_modified)),
        Head::Refused => Err(refuse(
            FailureKind::Permanent,
            "refused",
            "the server refused the request",
        )),
        Head::Unreadable => Err(refuse(
            FailureKind::Permanent,
            "bad_reply",
            "the server sent a reply this backend could not read",
        )),
    }
}

/// The host's handle for this invocation's socket, plus whatever a reply read overshot into.
struct Connection {
    handle: u32,
    pending: Vec<u8>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        net::close(self.handle);
    }
}

impl Connection {
    fn open(target: &Target) -> Result<Self, Failure> {
        // `true` starts TLS at once. A protocol that negotiates it in-band connects plain and
        // calls `net::start_tls` after the greeting.
        let handle = net::connect(&target.host, target.port, true)?;
        Ok(Self {
            handle,
            pending: Vec::new(),
        })
    }

    /// Sends one line and reads the single-line reply that answers it.
    fn request(&mut self, line: &str) -> Result<String, Failure> {
        net::write(self.handle, line.as_bytes())?;
        let reply = loop {
            if let Some(reply) = crate::take_line(&mut self.pending) {
                break reply;
            }
            let chunk = net::read(self.handle, CHUNK)?;
            if chunk.is_empty() {
                return Err(refuse(
                    FailureKind::Transient(None),
                    "no_reply",
                    "the server closed the connection without answering",
                ));
            }
            self.pending.extend_from_slice(&chunk);
        };
        String::from_utf8(reply).map_err(|_| {
            refuse(
                FailureKind::Permanent,
                "bad_reply",
                "the server sent a reply this backend could not read",
            )
        })
    }

    /// The next bytes of the file: first what the reply read already holds, then the socket.
    fn read(&mut self, max: u32) -> Result<Vec<u8>, Failure> {
        if !self.pending.is_empty() {
            let take = self.pending.len().min(max as usize);
            return Ok(self.pending.drain(..take).collect());
        }
        net::read(self.handle, max)
    }
}

/// A failure carrying a stable translation code and nothing the server wrote.
fn refuse(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

export!(Component);

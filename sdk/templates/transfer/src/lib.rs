//! {{PLUGIN_NAME}} transfer backend.
//!
//! This scaffold compiles and packages as it stands. The contract it implements is deliberately
//! lopsided: you move bytes, the application decides where they land and whether they count.
//! You never see a path, you never promote a file, and the length is verified without you.

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

/// Bytes per read. Small enough that a pause is noticed promptly.
const CHUNK: u32 = 32 * 1024;

struct Component;

impl Guest for Component {
    fn probe(_url: String, _credential_ref: Option<String>) -> Result<RemoteFile, Failure> {
        // Ask the server what is there. Reporting the size lets the application verify the
        // transfer afterwards; reporting `last-modified` lets it refuse a resume onto a file
        // that changed underneath.
        Err(failure(
            FailureKind::Unsupported,
            "{{PLUGIN_SLUG}}.not_implemented",
            "probe is not implemented yet",
        ))
    }

    fn run(job: Job) -> TransferEnd {
        match transfer(&job) {
            Ok(end) => end,
            Err(failure) => TransferEnd::Failed(failure),
        }
    }
}

fn transfer(_job: &Job) -> Result<TransferEnd, Failure> {
    // `sink::committed()` is the application's own count of what reached the disk. Start
    // there, not from your checkpoint: the checkpoint is your notes, this is the truth.
    let mut offset = sink::committed();
    let connection = net::connect("files.example.net", 21, false)?;

    loop {
        // Honour this. A backend that ignores it is stopped by its execution budget instead,
        // which loses the chance to write a checkpoint.
        if sink::should_stop() {
            sink::sync()?;
            net::close(connection);
            return Ok(TransferEnd::Stopped(offset.to_be_bytes().to_vec()));
        }
        let chunk = net::read(connection, CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        sink::write_at(offset, &chunk)?;
        offset += chunk.len() as u64;
        sink::progress(offset, None);
    }
    sink::sync()?;
    net::close(connection);
    Ok(TransferEnd::Complete(Some(offset.to_be_bytes().to_vec())))
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

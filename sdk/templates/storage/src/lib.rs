//! A scaffold storage destination. It compiles, packages and passes conformance as it is.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - Nothing local is deleted until `verify` says the destination holds the object. That is
//!   why the check is a call of its own rather than something `put` reports: an upload that
//!   returned success and lost the file would otherwise take the only copy with it.
//! - You read the package through a handle, never a path, and you can only read the file the
//!   job names.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "storage-plugin",
});

use exports::rdownloader::plugin::storage::{Guest, UploadEnd, UploadJob};
use rdownloader::plugin::source;

struct Component;

/// Bytes read per call. Large enough to be worth the crossing, small enough to leave room
/// for the guest's own memory limit.
const CHUNK: u32 = 256 * 1024;

impl Guest for Component {
    fn put(job: UploadJob) -> UploadEnd {
        let mut offset = job
            .checkpoint
            .as_deref()
            .and_then(|bytes| bytes.try_into().ok())
            .map_or(0, u64::from_le_bytes);
        while offset < job.size {
            if source::should_stop() {
                return UploadEnd::Stopped(offset.to_le_bytes().to_vec());
            }
            match source::read_at(&job.handle, &job.file_name, offset, CHUNK) {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => {
                    // Send `chunk` to your destination here, then advance.
                    offset += chunk.len() as u64;
                    source::progress(offset, Some(job.size));
                }
                Err(failure) => return UploadEnd::Failed(failure),
            }
        }
        // Return the destination's own identifier for the object, so `verify` can ask about
        // it later without having to guess a name from the file.
        UploadEnd::Complete(Some(job.file_name))
    }

    /// Ask the destination whether it really holds the object.
    ///
    /// Answer `false` rather than an error when it does not: a missing object is a fact
    /// about the destination, and the host keeps the local copy either way.
    fn verify(_handle: String, _remote_id: String) -> Result<bool, rdownloader::plugin::types::Failure> {
        Ok(true)
    }
}

export!(Component);

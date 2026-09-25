//! A scaffold post-processing step. It compiles, packages and passes conformance as it is.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - You never name a file. `handle` plus the names in `files` is all there is, and both
//!   only mean anything inside this invocation — a path would be a way out of the package.
//! - Stopping is normal. Check `should-stop` and return `stopped` with a checkpoint; the
//!   host stores it and hands it back, so a service restart resumes instead of starting over.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "postprocess-plugin",
});

use exports::rdownloader::plugin::postprocess::{Guest, StepEnd, StepInput};
use rdownloader::plugin::source;

struct Component;

/// Bytes read per call. Large enough to be worth the crossing, small enough to leave room
/// for the guest's own memory limit.
const CHUNK: u32 = 64 * 1024;

impl Guest for Component {
    fn run(input: StepInput) -> StepEnd {
        // Nothing to do is `skipped`, not `failed`: a package this step does not apply to is
        // an ordinary outcome, and reporting it as a failure would stop the pipeline.
        let Some(file) = input.files.first() else {
            return StepEnd::Skipped;
        };
        let total = match source::size_of(&input.handle, file) {
            Ok(size) => size,
            Err(failure) => return StepEnd::Failed(failure),
        };
        // Resume where the last attempt stopped, if there was one.
        let mut offset = input
            .checkpoint
            .as_deref()
            .and_then(|bytes| bytes.try_into().ok())
            .map_or(0, u64::from_le_bytes);
        while offset < total {
            if source::should_stop() {
                return StepEnd::Stopped(offset.to_le_bytes().to_vec());
            }
            match source::read_at(&input.handle, file, offset, CHUNK) {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => {
                    offset += chunk.len() as u64;
                    source::progress(offset, Some(total));
                }
                Err(failure) => return StepEnd::Failed(failure),
            }
        }
        StepEnd::Complete(None)
    }
}

export!(Component);

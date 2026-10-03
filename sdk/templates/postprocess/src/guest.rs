//! The component: a package in, a logged CRC-32 per file out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "postprocess-plugin",
});

use exports::rdownloader::plugin::postprocess::{Guest, StepComplete, StepEnd, StepInput};
use rdownloader::plugin::{host, source};

use crate::{Crc32, Progress};

struct Component;

/// Bytes read per call. Large enough to be worth the crossing, small enough to leave room
/// for the guest's own memory limit.
const CHUNK: u32 = 64 * 1024;

impl Guest for Component {
    fn run(input: StepInput) -> StepEnd {
        // Nothing to do is `skipped`, not `failed`: a package this step does not apply to is
        // an ordinary outcome, and reporting it as a failure would stop the pipeline.
        if input.files.is_empty() {
            return StepEnd::Skipped;
        }
        // Resume where the last attempt stopped, if there was one.
        let mut at = Progress::from_bytes(input.checkpoint.as_deref());
        while let Some(file) = input.files.get(at.file as usize) {
            let total = match source::size_of(&input.handle, file) {
                Ok(size) => size,
                Err(failure) => return StepEnd::Failed(failure),
            };
            while at.offset < total {
                if source::should_stop() {
                    return StepEnd::Stopped(at.to_bytes());
                }
                match source::read_at(&input.handle, file, at.offset, CHUNK) {
                    Ok(chunk) if chunk.is_empty() => break,
                    Ok(chunk) => {
                        at.crc.update(&chunk);
                        at.offset += chunk.len() as u64;
                        source::progress(at.offset, Some(total));
                    }
                    Err(failure) => return StepEnd::Failed(failure),
                }
            }
            host::log("info", &format!("crc32 {:08x} {file}", at.crc.value()));
            at = Progress {
                file: at.file + 1,
                offset: 0,
                crc: Crc32::default(),
            };
        }
        // `warnings` are shown on the step although it passed: a `{{PLUGIN_SLUG}}.*` code from
        // `locales/`, its parameters and an English fallback. A plain pass has none.
        // `input.removed` names the files earlier steps removed, should yours read a list of
        // files that may name one.
        StepEnd::Complete(StepComplete {
            checkpoint: None,
            warnings: Vec::new(),
        })
    }
}

export!(Component);

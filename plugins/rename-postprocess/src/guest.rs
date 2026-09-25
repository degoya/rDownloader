//! The component: rename the package's files to a tidy form.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "postprocess-plugin",
});

use exports::rdownloader::plugin::postprocess::{Guest, StepEnd, StepInput};
use rdownloader::plugin::source;

use crate::rules::{Rules, rename_to};

struct Component;

impl Guest for Component {
    fn run(input: StepInput) -> StepEnd {
        // The checkpoint is how many entries of `files` are done, so resuming needs no
        // bookkeeping beyond a count — the list arrives in the same order every time.
        let done = input
            .checkpoint
            .as_ref()
            .and_then(|bytes| bytes.as_slice().try_into().ok())
            .map_or(0_usize, |bytes: [u8; 4]| u32::from_le_bytes(bytes) as usize);

        let rules = Rules::default();
        let total = input.files.len() as u64;
        let mut renamed = 0_u32;

        for (index, name) in input.files.iter().enumerate().skip(done) {
            if source::should_stop() {
                return StepEnd::Stopped((index as u32).to_le_bytes().to_vec());
            }
            source::progress(index as u64, Some(total));
            let Some(target) = rename_to(name, rules) else {
                continue;
            };
            // A refused rename is not a reason to fail the package: the usual cause is a name
            // that is already taken, which leaves the file exactly as it was.
            if source::rename(&input.handle, name, &target).is_ok() {
                renamed += 1;
            }
        }

        source::progress(total, Some(total));
        if renamed == 0 {
            // Nothing to do is not a success worth reporting; most packages are already tidy.
            return StepEnd::Skipped;
        }
        StepEnd::Complete(None)
    }
}

export!(Component);

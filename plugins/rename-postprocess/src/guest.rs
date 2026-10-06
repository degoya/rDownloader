//! The component: rename the package's files to a tidy form.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_postprocess::{Guest, StepComplete, StepEnd, StepInput, source};

use crate::rules::{Rules, rename_to};

struct Component;

impl Guest for Component {
    fn run(input: StepInput) -> StepEnd {
        // The checkpoint is the name the next file had and how many files were renamed before
        // it (RD-191-06, PLUG-05). A count of entries done was wrong after a restart: the list
        // arrives sorted by name, the renamed files sort elsewhere under their new names, and
        // the count then skipped files nobody had looked at. Resuming at a name skips exactly
        // the files that sort before it; a renamed file that sorts after it is looked at again,
        // and its tidy name needs no renaming.
        let (mut renamed, resume_at) = input
            .checkpoint
            .as_deref()
            .and_then(read_checkpoint)
            .unwrap_or((0, String::new()));

        let rules = Rules::default();
        let total = input.files.len() as u64;

        for (index, name) in input.files.iter().enumerate() {
            if name.as_str() < resume_at.as_str() {
                continue;
            }
            if source::should_stop() {
                return StepEnd::Stopped(write_checkpoint(renamed, name));
            }
            source::progress(index as u64, Some(total));
            // A file in a folder of the package keeps its folder: only the last part is
            // tidied, and the host renames it where it is.
            let base = name
                .rsplit_once('/')
                .map_or(name.as_str(), |(_, base)| base);
            let Some(target) = rename_to(base, rules) else {
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
        StepEnd::Complete(StepComplete {
            checkpoint: None,
            warnings: Vec::new(),
        })
    }
}

/// `renamed` as four little-endian bytes, then the name to resume at.
fn write_checkpoint(renamed: u32, next: &str) -> Vec<u8> {
    let mut bytes = renamed.to_le_bytes().to_vec();
    bytes.extend_from_slice(next.as_bytes());
    bytes
}

fn read_checkpoint(bytes: &[u8]) -> Option<(u32, String)> {
    let (count, name) = bytes.split_first_chunk::<4>()?;
    let name = std::str::from_utf8(name).ok()?;
    Some((u32::from_le_bytes(*count), name.to_owned()))
}

plugin_guest_postprocess::postprocess_plugin!(Component);

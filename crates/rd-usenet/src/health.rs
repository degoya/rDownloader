//! Whether a Usenet set can still be repaired (RD-1100-02).
//!
//! SABnzbd's `fail_hopeless_jobs` and NZBGet's health check ask the same question while the
//! download runs: are more articles gone than the recovery data can stand in for? Both answer
//! it in bytes. This answers it in PAR2 blocks, the unit the repair counts in, with the block
//! arithmetic RD-107-04 introduced: a volume names its block count (`vol031+16.par2`), so the
//! recovery capacity is known before a single volume is fetched.
//!
//! The answer may only ever err towards "keep going". A set given up wrongly is a release the
//! person has to find again; a set kept wrongly costs what SABnzbd would have cost anyway. So
//! every number below is a bound in the safe direction:
//!
//! - **Missing blocks, from below.** PAR2 blocks never span files, and every byte a file lacks
//!   lies in some block of it, so a file missing `m` bytes has lost at least `m / B` blocks,
//!   and at least one. Only articles that every server refused count (`Failed` segments of a
//!   finished file, or a file that came back with nothing at all); an article that is still to
//!   come, or that a backup server delivered, is not missing.
//! - **The block size, from above.** A volume of `n` blocks is `n` blocks plus packet headers
//!   plus the set's critical packets, so its size divided by `n` is at least the block size.
//!   The smallest such quotient is the tightest bound. Both sides are NZB sizes - encoded
//!   article bytes - which inflate the payload and the volumes alike; the few percent in which
//!   the two encodings can differ are taken off the missing bytes first.
//! - **Available blocks, from above.** Every volume counts with every block its name promises,
//!   whether it has arrived, is still queued or was postponed by RD-107-04 - only a volume that
//!   came back with nothing at all counts for nothing.
//!
//! And where the set does not say enough, there is no verdict: recovery data whose block count
//! is unknown, a file still to come under a name that could be anything (an obfuscated post
//! keeps its PAR2 files behind such names, RD-108-24), or a set without any volume to count.
//! Those sets are left to the PAR2 stage, as before.

/// How far one file of the set has got, as far as the arithmetic is concerned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Standing {
    /// Still to come, running, or ended some way that says nothing about its articles.
    Open,
    /// Assembled; `missing_bytes` of its articles were on no server.
    Finished { missing_bytes: u64 },
    /// Not one of its articles was on any server.
    Lost,
}

/// One file of the set: its queue row's name and marking, its NZB size and its standing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SetFile<'a> {
    pub name: &'a str,
    /// The row is marked as PAR2 data, by name or by content (RD-108-23).
    pub recovery: bool,
    /// What the NZB says the file's articles weigh.
    pub nzb_bytes: u64,
    pub standing: Standing,
}

/// A set that has lost more than its recovery data can replace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Shortfall {
    /// The fewest blocks the damage can amount to.
    pub missing_blocks: u64,
    /// The most blocks the set's volumes can still supply.
    pub available_blocks: u64,
}

/// The share of the missing bytes that is not counted, in percent: the room for the payload
/// and the volumes not being inflated by exactly the same encoding overhead.
const ENCODING_TOLERANCE_PERCENT: u64 = 3;

/// The set's shortfall, when it is certain that the set can no longer be repaired.
///
/// `None` means keep going: the set is repairable, undamaged, or does not say enough to tell.
pub(crate) fn shortfall(files: &[SetFile<'_>]) -> Option<Shortfall> {
    let mut available = 0_u64;
    let mut volumes = 0_usize;
    let mut block_bound: Option<u64> = None;
    for file in files {
        if !is_recovery(file) {
            // A file still to come under a name without an extension may well be the set's
            // recovery data, and its blocks are unknown until it has arrived.
            if file.standing == Standing::Open && !rd_collector::looks_like_file_name(file.name) {
                return None;
            }
            continue;
        }
        if rd_core::is_par2_index(file.name) {
            continue;
        }
        // Recovery data that does not name its block count could hold any number of them.
        let blocks = u64::from(rd_core::par2_volume_blocks(file.name)?);
        volumes += 1;
        if blocks > 0 && file.nzb_bytes > 0 {
            let bound = file.nzb_bytes / blocks;
            block_bound = Some(block_bound.map_or(bound, |current| current.min(bound)));
        }
        if file.standing != Standing::Lost {
            available = available.saturating_add(blocks);
        }
    }
    if volumes == 0 {
        return None;
    }
    let block = block_bound.filter(|block| *block > 0)?;
    let mut missing = 0_u64;
    for file in files.iter().filter(|file| !is_recovery(file)) {
        let lost_bytes = match file.standing {
            Standing::Open => continue,
            Standing::Finished { missing_bytes } => missing_bytes,
            Standing::Lost => file.nzb_bytes,
        };
        if lost_bytes == 0 {
            continue;
        }
        let counted = lost_bytes / 100 * (100 - ENCODING_TOLERANCE_PERCENT)
            + lost_bytes % 100 * (100 - ENCODING_TOLERANCE_PERCENT) / 100;
        missing = missing.saturating_add((counted / block).max(1));
    }
    (missing > available).then_some(Shortfall {
        missing_blocks: missing,
        available_blocks: available,
    })
}

fn is_recovery(file: &SetFile<'_>) -> bool {
    file.recovery || rd_core::is_recovery_volume(file.name)
}

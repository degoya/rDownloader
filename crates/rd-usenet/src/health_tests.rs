//! RD-1100-02: the block arithmetic that decides whether a Usenet set can still be repaired.

use crate::health::{SetFile, Shortfall, Standing, shortfall};

fn file(name: &str, nzb_bytes: u64, standing: Standing) -> SetFile<'_> {
    SetFile {
        name,
        recovery: false,
        nzb_bytes,
        standing,
    }
}

fn holes(missing_bytes: u64) -> Standing {
    Standing::Finished { missing_bytes }
}

#[test]
fn more_missing_blocks_than_the_volumes_hold_is_a_shortfall() {
    // A volume of one block weighing 1000 bytes bounds the block size at 1000; 10 000 missing
    // bytes less the encoding tolerance are nine blocks at least.
    let set = [
        file("release.part1.rar", 50_000, holes(10_000)),
        file("release.part2.rar", 50_000, Standing::Open),
        file("release.par2", 64, Standing::Open),
        file("release.vol000+01.par2", 1_000, Standing::Open),
    ];
    assert_eq!(
        shortfall(&set),
        Some(Shortfall {
            missing_blocks: 9,
            available_blocks: 1
        })
    );
}

#[test]
fn a_gap_the_volumes_cover_is_no_shortfall() {
    let set = [
        file("release.part1.rar", 50_000, holes(10_000)),
        file("release.par2", 64, Standing::Open),
        file("release.vol000+20.par2", 20_000, Standing::Open),
    ];
    assert_eq!(shortfall(&set), None);
}

/// Postponed volumes (RD-107-04) and volumes still queued count with every block their names
/// promise; only a volume no server had counts for nothing.
#[test]
fn volumes_still_to_come_count_and_a_lost_one_does_not() {
    let mut set = vec![
        file("release.part1.rar", 50_000, holes(10_000)),
        file("release.par2", 64, Standing::Open),
        file("release.vol000+04.par2", 4_000, Standing::Open),
        file("release.vol004+08.par2", 8_000, Standing::Open),
    ];
    assert_eq!(shortfall(&set), None, "4 + 8 blocks cover 9");
    set[3].standing = Standing::Lost;
    assert_eq!(
        shortfall(&set),
        Some(Shortfall {
            missing_blocks: 9,
            available_blocks: 4
        })
    );
}

#[test]
fn a_lost_payload_file_counts_with_all_of_its_bytes() {
    let set = [
        file("release.part1.rar", 5_000, Standing::Lost),
        file("release.vol000+02.par2", 2_000, Standing::Open),
    ];
    assert_eq!(
        shortfall(&set),
        Some(Shortfall {
            missing_blocks: 4,
            available_blocks: 2
        })
    );
}

/// PAR2 blocks never span files: three files with one small hole each have lost three blocks.
#[test]
fn every_damaged_file_costs_at_least_one_block() {
    let set = [
        file("release.part1.rar", 5_000, holes(10)),
        file("release.part2.rar", 5_000, holes(10)),
        file("release.part3.rar", 5_000, holes(10)),
        file("release.vol000+02.par2", 2_000, Standing::Open),
    ];
    assert_eq!(
        shortfall(&set),
        Some(Shortfall {
            missing_blocks: 3,
            available_blocks: 2
        })
    );
}

/// 2050 encoded bytes would be two blocks of 1000; the tolerance for the two encodings makes
/// them one, which the single block still covers.
#[test]
fn the_encoding_tolerance_keeps_a_borderline_set_going() {
    let set = [
        file("release.part1.rar", 50_000, holes(2_050)),
        file("release.vol000+01.par2", 1_000, Standing::Open),
    ];
    assert_eq!(shortfall(&set), None);
}

#[test]
fn a_set_that_does_not_say_enough_gets_no_verdict() {
    let payload = file("release.part1.rar", 50_000, holes(10_000));
    let volume = file("release.vol000+01.par2", 1_000, Standing::Open);
    // A file still to come under a name that could be anything may be the recovery data of an
    // obfuscated post (RD-108-24).
    let obfuscated = file("a8f7d6e5c4b3", 40_000, Standing::Open);
    assert_eq!(shortfall(&[payload, obfuscated, volume]), None);
    // Recovery data recognised by its content names no block count.
    let by_content = SetFile {
        recovery: true,
        ..file("f7a8b9c0.bin", 40_000, holes(0))
    };
    assert_eq!(shortfall(&[payload, by_content, volume]), None);
    // An index and no volume: nothing to count, so the PAR2 stage decides as before.
    let index = file("release.par2", 64, Standing::Open);
    assert_eq!(shortfall(&[payload, index]), None);
    // And nothing missing is nothing to decide.
    assert_eq!(
        shortfall(&[file("release.part1.rar", 50_000, holes(0)), volume]),
        None
    );
}

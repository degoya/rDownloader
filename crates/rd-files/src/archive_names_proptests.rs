//! Archive volume names as properties (RD-1490-04): the names that decide which RAR, 7-Zip or
//! ZIP volume opens a set and in which order the others follow.
//!
//! Deterministic and short: a fixed seed and no regression file written into the source tree.
//! The case the properties found is a fixed test beside the parser's own
//! (`archive_names::tests::ignores_non_archives`, `.7z.001`).

use proptest::{collection::vec, prelude::*, sample::select, test_runner::RngSeed};

use crate::{ArchiveKind, ArchiveVolume, parse_archive_volume};

/// The pieces volume names are made of, cased both ways, with the separators between them.
const PIECES: &[&str] = &[
    "a", "\u{c4}", "\u{4e2d}", " ", ".", "part", "PART", "rar", "RAR", "r", "z", "7z", "7Z", "zip",
    "0", "1", "00", "01", "001", "999", "+", "-",
];

/// A set's name: any text, dots, spaces and other scripts included, never empty.
const BASE: &str = "\\PC{1,30}";

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 128,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x1490_0004),
        ..ProptestConfig::default()
    }
}

/// What every recognised volume holds, whatever name it came from.
fn holds_its_bounds(name: &str, volume: &ArchiveVolume) -> Result<(), TestCaseError> {
    prop_assert!(!volume.base.is_empty(), "{name:?}");
    prop_assert!(name.starts_with(&volume.base), "{name:?}");
    prop_assert!(!volume.is_first || volume.index == 0, "{name:?}");
    prop_assert!(volume.numbered || volume.is_first, "{name:?}");
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// Any name, control characters included: an answer, never a panic.
    #[test]
    fn an_arbitrary_name_keeps_the_bounds(name in ".{0,60}") {
        if let Some(volume) = parse_archive_volume(&name) {
            holds_its_bounds(&name, &volume)?;
        }
    }

    /// Names put together from the pieces volume names are made of.
    #[test]
    fn a_name_from_volume_pieces_keeps_the_bounds(pieces in vec(select(PIECES), 0..12)) {
        let name = pieces.concat();
        if let Some(volume) = parse_archive_volume(&name) {
            holds_its_bounds(&name, &volume)?;
        }
    }

    /// `name.partNN.rar`: the set's name back and the number as a zero-based index.
    #[test]
    fn a_numbered_rar_part_round_trips(
        base in BASE,
        number in 1_u32..100_000,
        width in 1_usize..6,
    ) {
        let volume = parse_archive_volume(&format!("{base}.part{number:0width$}.rar"));
        prop_assert_eq!(
            volume,
            Some(ArchiveVolume {
                base,
                kind: ArchiveKind::Rar,
                index: number - 1,
                is_first: number == 1,
                numbered: true,
            })
        );
    }

    /// `name.rNN` follows `name.rar`: `.r00` is the second volume.
    #[test]
    fn an_old_style_rar_volume_round_trips(base in BASE, number in 0_u32..1000) {
        let volume = parse_archive_volume(&format!("{base}.r{number:02}"));
        prop_assert_eq!(
            volume,
            Some(ArchiveVolume {
                base,
                kind: ArchiveKind::Rar,
                index: number + 1,
                is_first: false,
                numbered: true,
            })
        );
    }

    /// `name.7z.NNN` and `name.zip.NNN`: the set's name back and a zero-based index.
    #[test]
    fn a_split_volume_round_trips(base in BASE, number in 1_u32..1000, zip in any::<bool>()) {
        let (extension, kind) = if zip {
            ("zip", ArchiveKind::Zip)
        } else {
            ("7z", ArchiveKind::SevenZip)
        };
        let volume = parse_archive_volume(&format!("{base}.{extension}.{number:03}"));
        prop_assert_eq!(
            volume,
            Some(ArchiveVolume {
                base,
                kind,
                index: number - 1,
                is_first: number == 1,
                numbered: true,
            })
        );
    }
}

use super::*;

#[test]
fn the_measured_ten_megabyte_file_has_fourteen_chunks() {
    let offsets = boundaries(10_000_000);
    assert_eq!(offsets.len(), 14);
    assert_eq!(offsets[0], 131_072);
    assert_eq!(offsets[1], 393_216);
    assert_eq!(offsets[7], 4_718_592);
    assert_eq!(*offsets.last().expect("last"), 10_000_000);
    // The last chunk, measured: 38 528 bytes.
    assert_eq!(offsets[13] - offsets[12], 38_528);
}

#[test]
fn the_first_eight_grow_and_the_rest_do_not() {
    let offsets = boundaries(64 * 1024 * 1024);
    for index in 0..RAMP as usize {
        let start = if index == 0 { 0 } else { offsets[index - 1] };
        assert_eq!(offsets[index] - start, STEP * (index as u64 + 1));
    }
    for index in RAMP as usize..offsets.len() - 1 {
        assert_eq!(offsets[index] - offsets[index - 1], PLATEAU);
    }
}

#[test]
fn a_file_smaller_than_one_chunk_has_exactly_one() {
    assert_eq!(boundaries(1), vec![1]);
    assert_eq!(boundaries(STEP), vec![STEP]);
    assert_eq!(boundaries(STEP + 1), vec![STEP, STEP + 1]);
}

#[test]
fn an_empty_file_has_no_chunk_at_all() {
    assert!(boundaries(0).is_empty());
}

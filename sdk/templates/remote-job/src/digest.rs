//! SHA-1 over bytes, and hexadecimal.
//!
//! Written out rather than taken from a crate, for the reason the oauth scaffold writes
//! SHA-256 out: a sandboxed guest pays for every dependency in code size and in surface, and
//! this is sixty lines that compile for `wasm32-unknown-unknown` without WASI and are unit
//! tested on the host target as they are.
//!
//! It is here because `identify` needs it. The content key of a `.torrent` file is the SHA-1
//! of the bencoded value of its top-level `info` key — the same twenty bytes a magnet spells
//! out — and `identify` may not make a request, so the digest has to happen inside the guest.
//!
//! Replace this module if the provider you are writing for keys its jobs by something else.
//! It is a detail of BitTorrent, not of the `remote-job` world.

/// The SHA-1 digest of `bytes` (FIPS 180-4).
///
/// Streamed over the input in 64-byte blocks rather than copying it: a container may be
/// megabytes, and a copy of it is memory this plugin's budget does not have to spend.
#[must_use]
pub fn sha1(bytes: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let (blocks, rest) = bytes.as_chunks::<64>();
    for block in blocks {
        compress(&mut state, block);
    }
    // The padded tail: the rest of the input, a single one bit, zeroes, and the length in
    // bits as a big-endian u64. It is one block unless the rest leaves no room for the
    // length, in which case it is two.
    let mut tail = [0_u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let tail_length = if rest.len() + 9 <= 64 { 64 } else { 128 };
    let bits = u64::try_from(bytes.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    tail[tail_length - 8..tail_length].copy_from_slice(&bits.to_be_bytes());
    for block in tail[..tail_length].as_chunks::<64>().0 {
        compress(&mut state, block);
    }
    let mut digest = [0_u8; 20];
    for (word, out) in state.iter().zip(digest.as_chunks_mut::<4>().0) {
        *out = word.to_be_bytes();
    }
    digest
}

/// One 64-byte block, folded into the running state.
fn compress(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut schedule = [0_u32; 80];
    for (word, source) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*source);
    }
    for index in 16..80 {
        schedule[index] = (schedule[index - 3]
            ^ schedule[index - 8]
            ^ schedule[index - 14]
            ^ schedule[index - 16])
            .rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in schedule.iter().enumerate() {
        let (mixed, constant) = match index {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999_u32),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mixed)
            .wrapping_add(e)
            .wrapping_add(constant)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

/// Bytes as lower-case hexadecimal, which is how an info hash is written everywhere.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write;
        // Writing into a String cannot fail; the result is discarded rather than unwrapped.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::{sha1, to_hex};

    /// FIPS 180-4 test vectors, plus the empty input.
    ///
    /// The third is 56 bytes long on purpose: that is exactly the length at which the padded
    /// length no longer fits into the last block, so it is the one input that exercises the
    /// two-block tail. A SHA-1 that is wrong only there produces a content key nobody else
    /// has, which would make every duplicate guard in the host silently useless.
    #[test]
    fn the_published_vectors_come_out() {
        assert_eq!(
            to_hex(&sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            to_hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let fifty_six = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(fifty_six.len(), 56);
        assert_eq!(
            to_hex(&sha1(fifty_six)),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn a_block_boundary_is_not_a_special_case() {
        // 64 and 119 bytes: the first fills a block exactly, the second is the longest input
        // whose padding still needs the second tail block.
        assert_eq!(
            to_hex(&sha1(&[b'a'; 64])),
            "0098ba824b5c16427bd7a1122a5a442a25ec644d"
        );
        assert_eq!(
            to_hex(&sha1(&[b'a'; 119])),
            "ee971065aaa017e0632a8ca6c77bb3bf8b1dfc56"
        );
    }
}

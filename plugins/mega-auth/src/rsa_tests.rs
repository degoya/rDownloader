//! The RSA half, against a throwaway key.
//!
//! `PRIVK` and `CSID` below were produced on 2026-09-22 from the same throwaway RSA-2048 key
//! `plugins/mega-login-probe` carries -- generated for measurement, never any account's, and
//! protecting nothing. `PRIVK` is the four MPIs in MEGA's own framing, padded to an AES block
//! boundary exactly as `us` sends them; `CSID` is a session identifier encrypted to the
//! matching public key. No credential of any person or provider is in this file.

use super::PrivateKey;
use crate::flow::{b64_decode, session_id};

const PRIVK: &str = "BAD-VFp9uvEC-KJk0paJKgBA0Zuy8s6euh5hYmjRpRcqfTRYGUXjOVOvpBz3ewgmazTHp8bqoptCF8bAwEsnrqNh8neVJDZNGWgnmbZc0GYnPJCDT6TZv5y9h6ZQN4C1DGC8e2yqR0xrif_q_jt4CLLv5DzFtpM24G3eM87UDH6v-wQA9GGMZ2vnYzSImOXqReh-VOMhr0DMQQRWpCRMvGbdK9e3sq6y_NPFpCpdVbs7YrODX8X1PDDemHSOcxGy8qHNSTuDeXO6X1VylBwEGSzlPCjOu97-IEiD1XPBL0myDpyt3av1eyUUwNMdrGirVGItgipAMOK7OBANIgEEuT4SaSsH_RvxFEWXIwMywv4f9rKDc-4tn13PLqWi8vt0DYo9k0tGoNbTYX5izFeWLW8tLQtXX83illY0UVADFIcnGIqAyF_Ir0dZYcacxdsOx1lyvIBFRwbE4-E7MP0SJYcxOq3IfUPN7qCGuA5wi3NQxuVEuno82BukZFHfkJ5KHmRm7WBp7HbO468y5iUmP3GzSgirlXqUxGgRGCmYSlR0Dj8fcmYQeABlwAmtJz5ZQNnvzYD4YaD6JBRwS1P5YsU02U9IMqmIS7nHgZ40ydRa9qN0PSjsjqrPALUE9EpsvKB6WtiETsQYTN_5HZNu5QiqZbTIZQ8NphYxSt9cZvjoYQicEFsEAM9U7Ovc50zXAWYPItgSwrRrRPdZBhw8ZwR6Tz6alMvaO_OQhcP7EVMEaMl1veQEFR7lMR498dzl1pczRyjfJYBy6tjtXoS5YUQ3t3QcAU94RO5wgmHWbkR03azRInumUjVcMOGWY2zUjO8cDy-SDbOTjJs9vW3ja6JeucErTuQPAAAAAAAAAAA";
const CSID: &str = "01AjImGG8A-K76tJAprnjIPn0nJSSce2I3Rw8KjuTVtTctgrmnoynW4JkU30G6WuNPrrmWTCbgbLvrZoC6TcO0AbbQXmutGazltH-P4GhZgJAbJceDFvTaIpGA7XhhIZS9_TZKjrBK6e2eoPFhYU52zuepeVcn_bR9Hg97sPXXV0Wza2m2-ifpuL0LOVLsFPSfgYNZMdXDhjqvzNsQE2XJLzOwBiHYXOKJ-Twc0wiiRNTowHJDwhMeJSmn6xMag5CoVtoU1DOJl9meDGPcNIuwTYXDphwxZ636LwftB5BT8uI_rqD_STH1xRzkwhkvIyTL6y_yBDQOnCpI1-YM9NgA";
/// The identifier that ciphertext stands for.
const EXPECTED_SID: &str = "UkRURVNUU0VTU0lPTklERU5USUZJRVIwMTIzNDU2Nzg5YWJjZGVmZ2hpag";

#[test]
fn the_private_key_block_opens_the_session_identifier() {
    let block = b64_decode(PRIVK).expect("base64");
    // 648 bytes of MPIs, padded to 656 by the AES block the provider wrapped it in.
    assert_eq!(block.len(), 656);
    let key = PrivateKey::parse(&block).expect("four MPIs");
    let plain = key
        .decrypt(&b64_decode(CSID).expect("base64"))
        .expect("decrypts");
    assert_eq!(session_id(&plain).expect("long enough"), EXPECTED_SID);
}

#[test]
fn a_block_that_is_not_four_numbers_is_refused_rather_than_guessed_at() {
    assert!(PrivateKey::parse(&[]).is_none());
    assert!(PrivateKey::parse(&[0x00, 0x08, 0x01]).is_none());
    // A length header that reaches past the block is the shape a truncated answer has.
    assert!(PrivateKey::parse(&[0xff, 0xff, 0x01, 0x02]).is_none());
}

#[test]
fn a_ciphertext_of_the_wrong_shape_is_refused() {
    let key = PrivateKey::parse(&b64_decode(PRIVK).expect("base64")).expect("four MPIs");
    assert!(key.decrypt(&[]).is_none());
}

use super::base32_decode;

/// The pair has to round-trip, or a secret written to the vault cannot be read back and
/// the factor silently stops accepting every code.
#[test]
fn base32_round_trips_through_the_encoder() {
    for length in 0..40 {
        let bytes: Vec<u8> = (0..length).map(|index| (index * 7 + 3) as u8).collect();
        let encoded = rd_authn::totp::base32_encode(&bytes);
        assert_eq!(
            base32_decode(&encoded).as_deref(),
            Some(bytes.as_slice()),
            "length {length} did not survive the round trip"
        );
    }
}

#[test]
fn a_secret_that_is_not_base32_is_refused_rather_than_guessed_at() {
    assert_eq!(base32_decode("not base32!"), None);
}

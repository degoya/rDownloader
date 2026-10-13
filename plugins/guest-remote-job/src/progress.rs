//! A provider's progress figure in the thousandths the contract carries, written once for the
//! remote-job plugins (RD-1280-21). Which of the two a provider states — a fraction from 0.0 to
//! 1.0 or a percentage from 0 to 100 — is the plugin's to know; reading one as the other is a
//! progress bar that finishes at one percent or never leaves zero.

/// A fraction (`0.425`) in thousandths (`425`). `None` when nothing or no finite number was
/// stated; anything outside the range is clamped rather than believed.
#[must_use]
pub fn permille_of_fraction(fraction: Option<f64>) -> Option<u16> {
    scaled(fraction, 1_000.0)
}

/// A percentage (`42.5`) in thousandths (`425`), with the same `None` and clamping as
/// [`permille_of_fraction`].
#[must_use]
pub fn permille_of_percent(percent: Option<f64>) -> Option<u16> {
    scaled(percent, 10.0)
}

fn scaled(value: Option<f64>, thousandths_per_unit: f64) -> Option<u16> {
    let value = value?;
    if !value.is_finite() {
        return None;
    }
    let scaled = (value * thousandths_per_unit).round().clamp(0.0, 1_000.0);
    // The clamp above bounds the value into u16 range before the cast, so nothing is lost.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(scaled as u16)
}

#[cfg(test)]
mod tests {
    use super::{permille_of_fraction, permille_of_percent};

    #[test]
    fn a_fraction_is_read_as_a_fraction() {
        assert_eq!(permille_of_fraction(Some(0.0)), Some(0));
        assert_eq!(permille_of_fraction(Some(0.425)), Some(425));
        assert_eq!(permille_of_fraction(Some(1.0)), Some(1_000));
        assert_eq!(permille_of_fraction(Some(2.5)), Some(1_000), "clamped");
        assert_eq!(permille_of_fraction(Some(-1.0)), Some(0));
    }

    #[test]
    fn a_percentage_is_read_as_a_percentage() {
        assert_eq!(permille_of_percent(Some(0.0)), Some(0));
        assert_eq!(permille_of_percent(Some(37.5)), Some(375));
        assert_eq!(permille_of_percent(Some(100.0)), Some(1_000));
        assert_eq!(permille_of_percent(Some(140.0)), Some(1_000), "clamped");
        assert_eq!(permille_of_percent(Some(-5.0)), Some(0));
    }

    #[test]
    fn nothing_stated_is_no_progress_rather_than_zero() {
        assert_eq!(permille_of_fraction(None), None);
        assert_eq!(permille_of_fraction(Some(f64::NAN)), None);
        assert_eq!(permille_of_percent(None), None);
        assert_eq!(permille_of_percent(Some(f64::INFINITY)), None);
    }
}

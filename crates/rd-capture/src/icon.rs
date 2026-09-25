//! The geometry of the tray icon's activity badge.
//!
//! Plain arithmetic on an RGBA buffer: no `image` type and no `tray-icon` type crosses into this
//! module, because both crates are Windows- and macOS-only and the Linux agent must link neither.
//! What is left is a rule — where the badge sits, how big it is, which pixels it covers — and a
//! rule compiles and is tested on every host while the tray that draws it does not.
//!
//! `status.rs` is the same pattern for the service state, and carries the same gate: the tray's,
//! plus `test`, so the tests run here while a Linux release build compiles no unreachable code.
//!
//! Decoding the shipped PNG and handing the finished buffer to `Icon::from_rgba` stay in
//! `tray.rs`: `image` and `tray-icon` are both in the Linux build's forbidden list, so the
//! boundary is the raw RGBA buffer, not an `RgbaImage`.

/// Bytes per pixel in the buffers this module paints on.
const CHANNELS: usize = 4;

/// The signal teal the rest of the interface uses for activity, as opaque RGBA.
pub(crate) const BADGE: [u8; CHANNELS] = [0x2D, 0xD4, 0xBF, 0xFF];

/// Radius of the activity badge for an icon of this size, in pixels.
///
/// A third of the shorter side, so the badge scales with whatever size the platform asked for,
/// and never below three pixels — a third of a very small icon rounds down to a badge nobody
/// could see.
pub(crate) fn badge_radius(width: u32, height: u32) -> u32 {
    (width.min(height) / 3).max(3)
}

/// Centre of the badge: the far corner of the icon, pulled back inwards by one radius.
///
/// `saturating_sub` rather than `-`: the floor in [`badge_radius`] can exceed the icon's own
/// dimensions, and on such an icon the badge simply starts at the origin instead of wrapping
/// around to an enormous coordinate.
pub(crate) fn badge_centre(width: u32, height: u32, radius: u32) -> (u32, u32) {
    (width.saturating_sub(radius), height.saturating_sub(radius))
}

/// Whether the pixel at `(x, y)` lies within `radius` of `centre`.
///
/// Squared distances in `i64`, so neither the subtraction nor the multiplication can wrap for any
/// icon size a platform will ever hand over.
pub(crate) fn badge_covers(x: u32, y: u32, centre: (u32, u32), radius: u32) -> bool {
    let dx = i64::from(x) - i64::from(centre.0);
    let dy = i64::from(y) - i64::from(centre.1);
    dx * dx + dy * dy <= i64::from(radius) * i64::from(radius)
}

/// Paints the activity badge into an RGBA buffer in place.
///
/// The buffer is `width * height` pixels of four bytes each, row-major. A buffer shorter than
/// that is left alone rather than half-painted: it is not the image this was called for, and
/// producing an icon out of a partly written buffer would be worse than producing none.
pub(crate) fn paint_badge(pixels: &mut [u8], width: u32, height: u32) {
    let Some(expected) = pixel_count(width, height) else {
        return;
    };
    if pixels.len() != expected {
        return;
    }
    let radius = badge_radius(width, height);
    let centre = badge_centre(width, height, radius);
    for y in 0..height {
        for x in 0..width {
            if !badge_covers(x, y, centre, radius) {
                continue;
            }
            // Bounded by the length check above: `y < height` and `x < width`, so the offset is
            // inside the buffer for every iteration.
            let offset = (y as usize * width as usize + x as usize) * CHANNELS;
            pixels[offset..offset + CHANNELS].copy_from_slice(&BADGE);
        }
    }
}

/// Length in bytes of an RGBA buffer of this size, or `None` when it does not fit in a `usize`.
fn pixel_count(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(CHANNELS)
}

#[cfg(test)]
mod tests {
    use super::{BADGE, CHANNELS, badge_centre, badge_covers, badge_radius, paint_badge};

    /// The size of `extension/icons/icon32.png`, which is the artwork the tray actually shows.
    const SHIPPED: u32 = 32;

    fn blank(width: u32, height: u32) -> Vec<u8> {
        vec![0; width as usize * height as usize * CHANNELS]
    }

    fn painted(pixels: &[u8], width: u32, x: u32, y: u32) -> bool {
        let offset = (y as usize * width as usize + x as usize) * CHANNELS;
        pixels[offset..offset + CHANNELS] == BADGE
    }

    /// The badge is anchored in the far corner, not floating in the middle of the mark.
    ///
    /// Stated as "the origin is never touched" because that is the failure that would matter:
    /// a badge covering the top-left would sit on top of the artwork that identifies the
    /// application.
    #[test]
    fn the_badge_sits_in_the_far_corner_and_leaves_the_origin_alone() {
        let mut pixels = blank(SHIPPED, SHIPPED);
        paint_badge(&mut pixels, SHIPPED, SHIPPED);
        assert!(
            !painted(&pixels, SHIPPED, 0, 0),
            "the badge reached the top-left corner of the mark"
        );
        let radius = badge_radius(SHIPPED, SHIPPED);
        let centre = badge_centre(SHIPPED, SHIPPED, radius);
        assert!(
            painted(&pixels, SHIPPED, centre.0 - 1, centre.1 - 1),
            "the badge did not cover its own centre"
        );
    }

    /// Every painted pixel is one the geometry claims, and every claimed pixel is painted.
    ///
    /// The two halves catch opposite mistakes: a loop that paints past the disc, and a bounds
    /// check that quietly drops part of it.
    #[test]
    fn exactly_the_pixels_within_the_radius_are_painted() {
        let mut pixels = blank(SHIPPED, SHIPPED);
        paint_badge(&mut pixels, SHIPPED, SHIPPED);
        let radius = badge_radius(SHIPPED, SHIPPED);
        let centre = badge_centre(SHIPPED, SHIPPED, radius);
        for y in 0..SHIPPED {
            for x in 0..SHIPPED {
                assert_eq!(
                    painted(&pixels, SHIPPED, x, y),
                    badge_covers(x, y, centre, radius),
                    "pixel ({x}, {y}) disagrees with the badge geometry"
                );
            }
        }
    }

    /// A non-square icon catches a transposed index, which a square buffer cannot.
    ///
    /// With width and height swapped in the offset the paint wraps into the wrong rows, and on a
    /// square buffer that still lands inside the disc often enough to look right.
    #[test]
    fn a_non_square_icon_paints_the_same_corner() {
        let (width, height) = (8, 24);
        let mut pixels = blank(width, height);
        paint_badge(&mut pixels, width, height);
        let radius = badge_radius(width, height);
        let centre = badge_centre(width, height, radius);
        for y in 0..height {
            for x in 0..width {
                assert_eq!(
                    painted(&pixels, width, x, y),
                    badge_covers(x, y, centre, radius),
                    "pixel ({x}, {y}) of a {width}x{height} icon is wrong"
                );
            }
        }
        assert!(
            !painted(&pixels, width, 0, 0),
            "the badge reached the origin of a tall icon"
        );
    }

    /// The badge stays visible on a small icon, and stays a badge rather than a repaint.
    ///
    /// The floor in `badge_radius` exists for the first half; the second is what keeps the floor
    /// from turning the mark into a plain teal square at the sizes Windows asks for.
    #[test]
    fn a_small_icon_still_shows_a_badge_and_still_shows_the_mark() {
        for size in [16_u32, 20, 24, 32] {
            let mut pixels = blank(size, size);
            paint_badge(&mut pixels, size, size);
            let lit = (0..size)
                .flat_map(|y| (0..size).map(move |x| (x, y)))
                .filter(|(x, y)| painted(&pixels, size, *x, *y))
                .count();
            assert!(lit > 0, "a {size}x{size} icon shows no badge at all");
            let total = size as usize * size as usize;
            assert!(
                lit < total / 2,
                "the badge covers {lit} of {total} pixels at {size}x{size}; \
                 that is a repaint, not a badge"
            );
        }
    }

    /// No icon size makes the paint run off the end of the buffer.
    ///
    /// The degenerate sizes are the point: at 1x1 and 2x2 the three-pixel floor is larger than
    /// the icon, so the centre saturates to the origin, and `saturating_sub` is what keeps that
    /// from becoming a coordinate near `u32::MAX`.
    #[test]
    fn no_icon_size_paints_outside_its_own_buffer() {
        for (width, height) in [(0, 0), (1, 1), (2, 2), (3, 7), (7, 3), (64, 64)] {
            let mut pixels = blank(width, height);
            let length = pixels.len();
            paint_badge(&mut pixels, width, height);
            assert_eq!(
                pixels.len(),
                length,
                "painting a {width}x{height} icon changed the buffer length"
            );
        }
    }

    /// The badge is opaque, so it reads against whatever the artwork puts underneath it.
    #[test]
    fn the_badge_is_opaque() {
        assert_eq!(
            BADGE[3], 0xFF,
            "a translucent badge would sink into the mark"
        );
    }

    /// A buffer that is not the image the caller claims is left untouched.
    ///
    /// Half-painting it would hand `Icon::from_rgba` a buffer whose contents no longer match any
    /// icon, and that is harder to notice than an icon without a badge.
    #[test]
    fn a_buffer_of_the_wrong_length_is_left_alone() {
        let mut pixels = blank(SHIPPED, SHIPPED);
        pixels.truncate(pixels.len() - CHANNELS);
        let before = pixels.clone();
        paint_badge(&mut pixels, SHIPPED, SHIPPED);
        assert_eq!(pixels, before, "a short buffer was painted anyway");
    }
}

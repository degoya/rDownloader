//! The tray's marks: the shipped artwork, the same with the activity badge, and both greyed out
//! while clipboard watching is paused (RD-1180-01). Where the badge sits and how the grey is made
//! is `crate::icon`, which compiles and is tested on every host; this is the decoding and the
//! `Icon::from_rgba` around it, which need the platform crates. The menu's title line takes the
//! same artwork at 16 px (RD-1260-08).

use anyhow::{Context, Result};
use tray_icon::{Icon, menu::Icon as MenuIcon};

use crate::{icon as badge, tray_state::IconKind};

// The browser extension artwork doubles as the tray icon. Reaching across the
// crate boundary with `include_bytes!` would break `cargo package`; this crate
// is never published, so duplicating the PNG is not worth it.
const ICON_PNG: &[u8] = include_bytes!("../../../extension/icons/icon32.png");

/// The four marks, kept for the whole run: the tray is handed one or another whenever
/// transfers start or stop and whenever clipboard watching is paused or resumed.
pub(super) struct Marks {
    idle: Icon,
    /// The same mark with a badge, shown while something is transferring.
    busy: Icon,
    /// Both greyed out, while clipboard watching is paused.
    paused_idle: Icon,
    paused_busy: Icon,
    /// The title line's logo, or `None` when it could not be made: the title then stands
    /// without it rather than taking the tray down.
    logo: Option<MenuIcon>,
}

impl Marks {
    pub(super) fn prepare() -> Result<Self> {
        let image = decode_image()?;
        let mut paused = image.clone();
        badge::dim(&mut paused.rgba);
        Ok(Self {
            logo: logo_icon(&image),
            busy: busy_icon(&image)?,
            idle: to_icon(image)?,
            paused_busy: busy_icon(&paused)?,
            paused_idle: to_icon(paused)?,
        })
    }

    /// The icon handle for a mark the state named.
    pub(super) fn get(&self, kind: IconKind) -> Icon {
        match kind {
            IconKind::Idle => self.idle.clone(),
            IconKind::Busy => self.busy.clone(),
            IconKind::PausedIdle => self.paused_idle.clone(),
            IconKind::PausedBusy => self.paused_busy.clone(),
        }
    }

    /// The menu's 16 px logo for the title line (RD-1260-08).
    pub(super) fn logo(&self) -> Option<MenuIcon> {
        self.logo.clone()
    }
}

/// The decoded icon: 8-bit RGBA, row by row, as [`Icon::from_rgba`] takes it.
#[derive(Clone)]
struct Pixels {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// Decodes with `png` directly: the one asset is an 8-bit RGBA PNG, and `image` would add a
/// decoder framework on top of the `png` crate it uses for exactly this (RD-140-13).
fn decode_image() -> Result<Pixels> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().context("read the tray icon header")?;
    let size = reader
        .output_buffer_size()
        .context("the tray icon does not fit in memory")?;
    let mut rgba = vec![0; size];
    let frame = reader
        .next_frame(&mut rgba)
        .context("decode the tray icon")?;
    anyhow::ensure!(
        frame.color_type == png::ColorType::Rgba && frame.bit_depth == png::BitDepth::Eight,
        "the tray icon is not 8-bit RGBA"
    );
    rgba.truncate(frame.buffer_size());
    Ok(Pixels {
        rgba,
        width: frame.width,
        height: frame.height,
    })
}

fn to_icon(image: Pixels) -> Result<Icon> {
    Icon::from_rgba(image.rgba, image.width, image.height).context("build the tray icon")
}

/// The idle mark with a filled corner badge, for "something is transferring".
///
/// Derived from the same image rather than shipped as a second file: one asset cannot drift from
/// the other, and `web/public/favicon.svg` stays the single source every icon is generated from.
///
/// Where the badge sits and which pixels it covers is [`crate::icon`], which knows nothing about
/// `png` or `tray-icon` and is measured on Linux. What is left here is the pair of conversions
/// those two crates own.
fn busy_icon(base: &Pixels) -> Result<Icon> {
    let mut pixels = base.rgba.clone();
    badge::paint_badge(&mut pixels, base.width, base.height);
    Icon::from_rgba(pixels, base.width, base.height).context("build the busy tray icon")
}

/// The mark at half its size for the menu's title line (RD-1260-08): the shipped 32 px artwork
/// halved by [`crate::icon::halve`], not a second file.
fn logo_icon(image: &Pixels) -> Option<MenuIcon> {
    let halved = badge::halve(&image.rgba, image.width, image.height)?;
    match MenuIcon::from_rgba(halved, image.width / 2, image.height / 2) {
        Ok(icon) => Some(icon),
        Err(error) => {
            tracing::warn!(%error, "the tray menu's logo could not be built");
            None
        }
    }
}

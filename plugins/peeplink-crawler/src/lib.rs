//! PEEPLink and AlfaLink entry pages in the LinkGrabber (RD-110-17).
//!
//! The one service of eight that survived the measurement of 2026-09-21. Seven others are
//! dead, parked, a maintenance page, or behind a Cloudflare managed challenge; this one
//! answers a plain `GET` with the hoster links in clear text inside its `<article>`, refuses
//! an unknown identifier with `404`, and served byte-identical pages over three runs without
//! a cookie jar. Nothing stands in front of it: the reCAPTCHA, hCaptcha and QapTcha markers
//! those pages carry sit in the login and register popups, and this plugin never signs in.
//!
//! It is a plugin rather than a site rule because the reading is not the whole job: two
//! domains serve two different shapes — `peeplink.in` writes each link as an `<a href>`,
//! `alfalink.to` writes it as bare text inside `<article class="articless">` — there is a
//! password branch with its own `POST`, and four refusals have to be told apart.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`entry`] reads a page. `guest` is the thin wrapper
//! around the two and exists only on `wasm32`.

pub mod entry;
pub mod messages;
pub mod target;

#[cfg(target_arch = "wasm32")]
mod guest;

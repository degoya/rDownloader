//! The host abstraction a hoster's protocol logic is written against.
//!
//! Every bundled hoster used to exist twice: once natively against `rd_plugin_api::ResolverHost`
//! and once as a WebAssembly guest against the WIT imports. The two are genuinely different —
//! one is async and speaks `rd_core::Failure`, the other is synchronous and speaks a generated
//! type — so the logic between them was copied rather than shared, and every fix had to be made
//! twice or silently was not.
//!
//! This crate is the third thing both can be: a set of plain types and one trait, depending on
//! neither side. The protocol logic is written once against [`PluginHost`], and each build
//! supplies a thin adapter. `xfs-common` already does this for the *pure* parts, the parsing
//! and the markers; what was left over is the part that talks to a host, which is what this
//! covers.
//!
//! The trait uses `async fn` directly rather than boxed futures, so the guest pays nothing for
//! it: its adapter's futures are ready on the first poll, and [`block_on`] drives them without
//! a runtime, a waker or an allocation.

#![forbid(unsafe_code)]

mod host;
pub mod label;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
/// PKCE and the small JSON reader every OAuth plugin needs.
///
/// It lived four times over — in `example-oauth`, `dropbox-oauth`, `google-drive-oauth` and
/// `onedrive-oauth`, byte for byte — which meant a correction to a security primitive had to be
/// made four times or was silently made once. It depends on nothing, so it adds no import to a
/// guest that takes it.
pub mod pkce;
mod poll;
mod types;

pub use host::PluginHost;
pub use label::{Label, LabelPart};
pub use poll::block_on;
pub use types::{
    Account, CaptchaAnswer, CaptchaChallenge, CaptchaSolution, CheckInput, ClickPoint,
    CutcaptchaChallenge, Failure, FailureKind, Header, HttpRequest, HttpResponse, ImageChallenge,
    LinkCheck, LinkStatus, ResolveInput, Resolved, WidgetChallenge,
};

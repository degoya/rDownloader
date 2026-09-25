//! LinkSnappy resolver: stateless multihoster JSON API (`linksnappy.com/api`), username + account
//! password. JD's own plugin authenticates once via `/api/AUTHENTICATE` and reuses the resulting
//! cookie session for every later call; this plugin cannot do that (the host never hands a
//! freshly-authenticated session's cookies back for reuse within one resolver invocation, and
//! every plugin in this workspace treats each invocation as stateless). It instead follows
//! pyLoad's independently-implemented `LinksnappyCom.py`, which proves the API also accepts
//! `username`/`password` embedded directly in a single stateless `genLinks` call with no prior
//! login step — see `api.rs`'s module-level IMPL-VERIFY note for the full comparison against JD,
//! `resolveurl`'s Kodi plugin and pyLoad.
//!
//! `check_account`'s premium detection is a second, related simplification: JD compares the
//! numeric `expire` epoch LinkSnappy returns against the current wall-clock time
//! (`validUntil > System.currentTimeMillis()`). Neither this crate's WASM guest target (the WIT
//! host interface in `crates/rd-plugin-api/wit/rdownloader.wit` exposes no clock function at all)
//! nor, deliberately, `native.rs` (to keep native/guest parity — see `plugin-common.md`'s "Both
//! implementations must be thin adapters ... so error codes and messages are byte-identical")
//! perform that comparison; `api::is_premium` instead treats any non-`"expired"` `expire` value as
//! an active plan. This can report `premium: true` for an account whose numeric `expire` has, in
//! reality, already passed; `resolve()`'s live `linksnappy.account_expired` handling (triggered by
//! LinkSnappy's own "Your Account has Expired" API message) is the authoritative signal for that
//! case, so `check_account`'s label is informational, not load-bearing for whether downloads work.

pub(crate) mod api;
mod messages;
mod resolver;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
pub use native::LinkSnappyResolver;

//! What the Premiumize plugins read the same way (RD-120-23).
//!
//! Four plugins run on one Premiumize account: the resolver `plugins/premiumize/`, the sign-in
//! `plugins/premiumize-auth/`, the folder crawler `plugins/premiumize-crawler/` and the
//! transfers `plugins/premiumize-transfers/`. The last two read the same listings — a finished
//! transfer's folder is exactly what the crawler walks — so `listing` lives here rather than in
//! both, and `status` and `container` beside it, because a second reader of the same envelope
//! is how the first divergence starts.
//!
//! `plugins/premiumize/` deliberately still carries its own copy of the envelope vocabulary.
//! Moving it would mean editing the resolver, which RD-120-23 put out of scope, and the two
//! are not yet duplicated code so much as one table written twice — worth consolidating in a
//! job that is allowed to touch the resolver, not in this one.
//!
//! `cache` is the first thing the resolver shares (RD-130-11): `cache/check` is asked by the
//! resolver about hoster links and by the transfers about magnets, and one reading of its
//! index-aligned arrays is what keeps "held" and "known" meaning the same in both.
//!
//! Nothing in this crate makes a request or touches a credential: it is pure reading, which
//! is what lets `cargo test` cover it on the host target without a WebAssembly toolchain.

pub mod cache;
pub mod container;
pub mod listing;
pub mod status;

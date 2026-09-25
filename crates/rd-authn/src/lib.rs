//! Authentication mechanics: who the caller is, where they are, and how often they may ask.
//!
//! A crate of its own rather than more modules in `rd-api`, for a reason that is about the
//! build rather than the design. Lockout curves, forwarded-header parsing and, later, TOTP
//! drift windows and WebAuthn ceremonies are security-critical logic whose tests should be
//! cheap to run — and every `rd-api` integration binary links the whole dependency graph, so
//! a test placed there is one that gets run less often. Here `cargo nextest run -p rd-authn`
//! finishes in under a second.
//!
//! `rd-api` keeps the axum middleware and handlers; this crate keeps the decisions they make.

pub mod cidr;
pub mod client_ip;
pub mod proxy;
pub mod recovery;
pub mod throttle;
pub mod totp;
pub mod webauthn;

pub use cidr::{Cidr, CidrError, loopback_ranges};
pub use client_ip::{ClientAddress, resolve as resolve_client_address};
pub use proxy::{CookieSecurity, ProxyConfig, ProxyConfigError};
pub use recovery::{CODE_COUNT as RECOVERY_CODE_COUNT, RecoveryCode};
pub use throttle::{Decision, LoginThrottle, ThrottleSettings};
pub use webauthn::{CeremonyStore, RelyingPartyError, relying_party};

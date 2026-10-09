//! Application updates (RD-180-01): is there a newer rDownloader, can it be trusted, and what
//! does the person running this installation do about it.
//!
//! * [`manifest`] — the signed update manifest, one per channel, published as a release asset
//!   and verified against the compiled-in `Role::Release` root: signature, schema, channel,
//!   freshness and content, in that order.
//! * [`check`] — fetching the manifests of a channel from GitHub and verifying them against the
//!   replay floor the caller persists.
//! * [`offer`] — which verified version, if any, is offered: never a downgrade, never a beta on
//!   the stable channel, and the artifact that fits this platform and installation.
//! * [`install_kind`] — how this installation came to be (portable archive, installer, package
//!   manager, container) and therefore whether it updates itself or shows a command.
//! * [`download`] — streaming an artifact to disk and refusing it unless its size and SHA-256
//!   match the manifest.
//! * [`install`] — installing a downloaded artifact and taking it back (RD-180-02): the journal,
//!   the portable switch, the processes of an update and what a start does with an interrupted
//!   one.
//! * [`agent`] — the capture agent's own update when it is installed without the service
//!   (RD-1210-03): whether it updates itself, what it reports, and its install with roll-back.
//!
//! No database, no queue, no plugin host: the updater of RD-180-02 runs this before the new
//! version is known to start, and the service keeps its own state (the floors, the last result)
//! in its settings.

#![warn(unreachable_pub)]

pub mod agent;
pub mod check;
pub mod download;
pub mod fetch;
pub mod install;
pub mod install_kind;
pub mod manifest;
pub mod offer;
pub mod settings;

pub use check::{CheckReport, Floors, Sources, check};
pub use download::{download_verified, download_verified_with, verified_file};
pub use fetch::{Fetcher, HttpFetcher, MemoryFetcher};
pub use install_kind::{INSTALL_KIND_ENV, INSTALL_KIND_FILE, InstallKind, UpdateAction};
pub use manifest::{Artifact, Channel, UpdateError, UpdateManifest};
pub use offer::{Offer, Target, is_newer, newest_agent_offer, newest_offer, parse_version};
pub use settings::UpdateSettings;

pub use rd_sign::{SigningKey, TrustStore};

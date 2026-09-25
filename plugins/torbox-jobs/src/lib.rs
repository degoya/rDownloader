//! TorBox torrents, Usenet downloads and web downloads as jobs at the provider (RD-120-01).
//!
//! The second `remote-job` plugin, and the first that carries all three shapes of
//! `job-source`. TorBox runs three kinds of job -- a torrent, an NZB, an ordinary link it
//! fetches for itself -- behind three sets of endpoints that answer in the same envelope and
//! move through the same states. So they are one plugin and **one state machine**: the kind
//! chooses the path, and nothing else about the sequence changes.
//!
//! The siblings, because a manifest carries exactly one `plugin_type`:
//!
//! - `plugins/torbox/` is the resolver. It carries the `[provider]` row every TorBox account
//!   hangs off, and it turns the stable address this plugin hands back into the short-lived
//!   one the bytes actually come from -- again on every attempt, which is what makes a resume
//!   after a pause work rather than fail on an expired ticket.
//! - `plugins/torbox-auth/` checks the pasted API key against the account and reports what
//!   the plan is worth.
//!
//! Three things are worth saying out loud, because they are where TorBox differs from the
//! Real-Debrid plugin this one is modelled on:
//!
//! - **No file-selection step.** TorBox downloads the whole job and offers every file
//!   afterwards; there is no call that says "these three and not the others". So this plugin
//!   never answers `awaiting-choice`, and the person picks in the LinkGrabber, where the
//!   finished addresses land as one package. `choose` is exported because the world requires
//!   it and refuses under its own code, because it cannot be reached.
//! - **The finished address carries no credential.** `requestdl` needs the account's API key
//!   as a query parameter, and this plugin never sees it. What it hands back is the stable
//!   `requestdl` address *without* the key, which is exactly the address the resolver sibling
//!   claims -- so the key is added by the host, at resolve time, every time.
//! - **The content key carries the kind.** `torrent:<sha1>`, `usenet:<md5>`, `web:<md5>`;
//!   [`source`] says why.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`source`] derives the kind and the key, [`api`] holds the response shapes, the state
//! mapping, the request bodies and the failure classification. `guest` is the thin wrapper
//! around the two and exists only on `wasm32`.

pub mod api;
pub mod messages;
pub mod source;

#[cfg(target_arch = "wasm32")]
mod guest;

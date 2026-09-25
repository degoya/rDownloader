//! Managed external tools (RD-102-02): downloading, verifying, activating and rolling back
//! yt-dlp, gallery-dl, Streamlink and FFmpeg.
//!
//! A leaf crate rather than part of `rd-core`, for the same reason `rd-authn` is one:
//! everything in the workspace depends on `rd-core`, and this needs HTTP, ZIP and the
//! signature primitives. Putting it there would make a change to the tool store rebuild the
//! world and would drag reqwest into every crate that only wanted a settings struct.
//!
//! ## What holds the feature together
//!
//! * **A signed manifest decides what may be installed.** URL, SHA-256 and size all come from
//!   a document signed by the compiled-in tool-manifest root, checked for replay against a
//!   sequence this installation persists. See [`manifest`].
//! * **The hash decides whether it is activated.** Bytes stream into a staging directory and
//!   are hashed as they arrive; a mismatch removes the staging directory and nothing else
//!   ever sees it. See [`download`].
//! * **Activation is one rename and one pointer file.** No symlink, because Windows makes
//!   those a privilege. See [`store`] and [`activation`].
//! * **A running job keeps its binary.** Resolving a managed tool takes a lease; a leased
//!   version is never removed. See [`lease`].
//! * **A version that is too old or known bad blocks only what it breaks.** The signed
//!   manifest carries the policy alongside the builds; an unreadable policy degrades to the
//!   compiled-in base rules, and an unreadable *version* never blocks at all. See [`compat`]
//!   and [`version`].
//! * **Running one is the same everywhere.** Locating, leasing, gating and spawning an
//!   external tool is one path, so a Windows console-window flag or a `kill_on_drop` cannot
//!   be present in two runners and missing from the third. See [`process`].
//! * **A system tool is never touched.** The store owns `<data>/tools/**` and nothing else,
//!   and an explicitly configured path still wins over every managed version.

pub mod activation;
pub mod compat;
pub mod download;
mod error;
pub mod lease;
pub mod manifest;
pub mod platform;
pub mod process;
mod service;
pub mod store;
pub mod version;

pub use compat::{Assessment, Capability, CompatRule, CompatRules, Verdict};
pub use error::ToolError;
pub use lease::LeaseRegistry;
pub use manifest::{
    ArchiveFormat, MANAGED_TOOLS, TOOL_MANIFEST_DOMAIN, TOOL_MANIFEST_SCHEMA_VERSION, ToolEntry,
    ToolManifest, is_managed_tool,
};
pub use process::{
    PROGRESS_INTERVAL, PreparedTool, ProgressThrottle, Stdout, ToolProcess, prepare, run_to_output,
};
pub use service::{ManagedToolService, ManagedToolStatus};
pub use store::{TOOLS_DIR_NAME, ToolStore};
pub use version::{DetectedVersion, ToolVersion};

/// The tool store directory under a data directory.
#[must_use]
pub fn store_root(data_directory: &std::path::Path) -> std::path::PathBuf {
    data_directory.join(TOOLS_DIR_NAME)
}

//! MEGA folders (RD-103-02).
//!
//! The half of MEGA that is a list rather than a file. A folder link's fragment carries the
//! share key; one `a=f` call answers with every node behind it, and each node's name and key
//! are encrypted under that share key. This crawler opens them and names the files, so the
//! LinkGrabber shows structure, names and sizes before anything is queued.
//!
//! Every file it names keeps the folder form of the address -- `…/folder/<handle>#<key>/file/
//! <node>` -- because a node's key lives in the listing and not in any address MEGA defines.
//! The stream-transform plugin beside this one reads that form and fetches the key the same
//! way.
//!
//! A folder of the signed-in account (`https://mega.nz/fm/<handle>`, RD-120-30) is listed
//! with the session and walked in `account`: its keys are wrapped under the master key, which
//! only the host holds, so each is unwrapped by the host one node at a time. The files it names
//! carry no key at all (`https://mega.nz/fm/file/<node>`).

pub mod account;
pub mod messages;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;

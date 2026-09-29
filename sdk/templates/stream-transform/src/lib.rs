//! A scaffold stream transform. It compiles, packages and passes conformance as it is.
//!
//! For a provider that encrypts on the client and keeps the key out of its own reach: the key
//! rides in the link's fragment, the storage server only ever sees ciphertext, and a plain
//! download would be a file of the right length full of rubbish. `resolve` answers with the
//! address *and* a description of how its bytes become the file; the host applies it on its
//! own write path.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **The plugin describes, the host computes.** You name a primitive the host implements
//!   (`aes-128-ctr` today) and its parameters. You never see a byte of the file, and a name the
//!   host does not know is refused rather than guessed at.
//! - **The key is a secret from the moment you return it.** The host vaults it and keeps it
//!   out of every row, log line and answer. Your half of that promise: the key never goes
//!   into a request — the provider is the one party that must never learn it.
//! - **Nothing survives between calls.** A download that resumes asks `resolve` again and
//!   gets a fresh address and a fresh description, so there is nothing to remember.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives outside the component. `cargo test` in a fresh scaffold runs
//! [`link`] and [`reply`] on the host target; `guest` exists only on `wasm32`.

pub mod link;
pub mod reply;

#[cfg(target_arch = "wasm32")]
mod guest;

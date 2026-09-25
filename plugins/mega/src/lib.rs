//! MEGA, the first provider of the twelfth world (RD-103-02, ADR 0011).
//!
//! MEGA encrypts every file on the client and never holds the key: it rides in the fragment of
//! the link somebody was given. So a MEGA address resolves to two things, not one -- where the
//! ciphertext is, and how it becomes the file. This plugin answers both, and computes neither:
//! the AES-128-CTR keystream and the chunk MACs run on the host's write path, where the bytes
//! already are, which is the whole reason ADR 0011 exists.
//!
//! What is in this crate is therefore small: read the address, ask MEGA's command endpoint,
//! open the attribute block, and hand over the key schedule. [`plan`] holds all of it and is
//! tested on the host target; [`guest`] is the translation into the WIT vocabulary.

pub mod messages;
pub mod plan;

#[cfg(target_arch = "wasm32")]
mod guest;

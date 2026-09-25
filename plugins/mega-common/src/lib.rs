//! What both MEGA plugins need to know, and neither of them may own twice.
//!
//! MEGA encrypts every file on the client: the provider stores ciphertext and never holds the
//! key, which rides in the link fragment the person already has. Resolving such a link is
//! therefore two separate pieces of work, and this crate holds the half that is pure
//! arithmetic on values the caller already has -- addresses, keys, attribute blocks, chunk
//! boundaries, the API's request and answer shapes. Neither half fetches anything.
//!
//! What is deliberately absent is the payload cipher. `docs/adr/0011-*` put AES-128-CTR on the
//! host's write path (`crates/rd-http/src/transform.rs`), so a plugin describes the transform
//! and never performs it. The AES in here touches 64-byte attribute blocks and 16-byte node
//! keys, nothing that grows with a file.

pub mod address;
pub mod api;
pub mod chunks;
pub mod crypto;

pub use address::Target;

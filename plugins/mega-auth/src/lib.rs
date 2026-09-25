//! MEGA account sign-in, with the password never entering the sandbox (RD-120-20).
//!
//! MEGA's `us` call does not take a password. It takes `uh` -- the second half of
//! PBKDF2(password, salt, 100 000) -- and the first half unwraps the account's master key,
//! which unwraps the RSA private key, which decrypts the session identifier. Every stage
//! needs the password or something derived from it, and `interface auth` says in its own
//! header that a plugin never sees a credential. Under `rdownloader:plugin@0.6.0` the
//! sign-in was therefore not buildable at any fuel price; `docs/roadmap/jobs/120-11-mega.md`
//! measured that and section 2 of it states the conclusion.
//!
//! The contract answers it with `interface key-derivation`: the guest names the credential
//! and the stages, the host runs them, and only the last stage's output comes back. This
//! plugin makes three such calls and nothing more:
//!
//! | call | chain | what comes back |
//! | --- | --- | --- |
//! | 1 | PBKDF2, then bytes 16..32 | `uh`, which `us` carries |
//! | 2 | PBKDF2, bytes 0..16, AES-128-ECB over `k` | the master key |
//! | 3 | the same, then AES-128-ECB over `privk` | the RSA private key block |
//!
//! Three calls rather than one on purpose. Asking once for all thirty-two derived bytes
//! would be cheaper and would work -- and would put the password key, the one value that is
//! a direct function of the password, into the sandbox. The whole point of the primitive is
//! that it need not go there, so this plugin does not ask for it.
//!
//! What the plugin does compute itself is the RSA private operation on `csid`. That is not
//! a credential: it is ciphertext MEGA sent, under a key MEGA also sent, and it cost
//! 189 444 831 fuel when it was measured -- under a tenth of a default budget.
//!
//! [`flow`] and [`rsa`] are pure and tested on the host target; `guest` is the component
//! wrapper and exists only on `wasm32`.

pub mod flow;
pub mod rsa;

#[cfg(target_arch = "wasm32")]
mod guest;

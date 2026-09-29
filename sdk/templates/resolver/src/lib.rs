//! {{PLUGIN_NAME}} resolver.
//!
//! This scaffold compiles and packages as it stands, so the first failure you see is about
//! your own code and not about the setup.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You are asked before you are handed a link.** `match-url` reaches nothing, and the host
//!   also checks the link against `match_domains` in `manifest.toml`; a resolver that claims
//!   links belonging to nobody fails conformance.
//! - **You never see a credential.** `{{secret:<reference>}}` in a header or query value is
//!   expanded by the host on the way out, towards the domains the manifest grants, and only
//!   for the account the call is made for.
//! - **An account check states findings, not hopes.** `premium` is `true` only where the
//!   check actually read a subscription.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// Hosts this plugin claims. Keep in step with `match_domains`.
pub const HOSTS: &[&str] = &["{{PLUGIN_SLUG}}.example"];

/// Whether `url` is a link of this provider: an `https` or `http` address on one of [`HOSTS`]
/// or a sub-domain of one.
#[must_use]
pub fn claims(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && url
                .host_str()
                .is_some_and(|host| HOSTS.iter().any(|claimed| host_matches(host, claimed)))
    })
}

/// Exact host, or a sub-domain of it — never a host that merely ends in the same letters.
fn host_matches(host: &str, claimed: &str) -> bool {
    host == claimed || host.ends_with(&format!(".{claimed}"))
}

#[cfg(test)]
mod tests {
    use super::{HOSTS, claims};

    #[test]
    fn a_link_on_the_provider_or_a_sub_domain_is_claimed() {
        let host = HOSTS[0];
        assert!(claims(&format!("https://{host}/f/abc")));
        assert!(claims(&format!("https://www.{host}/f/abc")));
    }

    #[test]
    fn a_link_belonging_to_nobody_is_not_claimed() {
        // The two addresses conformance asks about: claiming either fails the package.
        assert!(!claims("https://conformance.invalid/some/file.bin"));
        assert!(!claims("https://cdn.example.org/a/b/c.zip"));
        // A host that only ends in the same letters is somebody else's.
        assert!(!claims(&format!("https://not{}/f/abc", HOSTS[0])));
        assert!(!claims("not a link"));
    }
}

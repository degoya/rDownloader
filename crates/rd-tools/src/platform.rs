//! Which manifest entries belong to the machine this is running on.
//!
//! The target triple is taken from the compiler rather than from `std::env::consts` pieces
//! glued together, so a cross-build names the target it was built for and not the host that
//! built it.

/// The Rust target triple this build was compiled for.
///
/// Manifest entries carry the same string, so an entry either matches this build or is not
/// for it. Nothing infers a "close enough" platform: running a glibc build on a musl system
/// fails in ways a download manager should not be arranging.
#[must_use]
pub fn current() -> &'static str {
    CURRENT
}

const CURRENT: &str = env!("RD_TOOLS_TARGET");

#[cfg(test)]
mod tests {
    /// A triple with no separators would silently match nothing.
    #[test]
    fn the_target_triple_looks_like_one() {
        assert!(super::current().contains('-'), "{}", super::current());
    }
}

//! Pairs of links a person stated are *not* mirrors of each other (RD-110-34).
//!
//! A group built from a shared file name alone is a proposal, and RD-110-18 left no way to
//! contradict one: whoever could see that two links are a different cut of the same release
//! could only pin one of them, never take the group apart. This type is what a contradiction
//! becomes once it has been stated — a set of unordered pairs, checked by
//! [`crate::group_mirrors`] before any source is allowed to form a group out of them.
//!
//! It is deliberately a fact about *links*, not about a group key or a source. A group key is
//! derived and changes the moment a size arrives; a source is re-decided on every regroup. A
//! decision anchored to either would evaporate exactly where it has to hold, which is what
//! makes this the same shape as the rule that two links with the same address are never
//! mirrors of each other: a property of the pair, checked before the sources get their turn.

use std::collections::BTreeSet;

/// The pairs of one package's links that must not be grouped together again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MirrorSeparations {
    /// Each pair with the smaller identifier first, so a pair is stored and asked for once.
    pairs: BTreeSet<(String, String)>,
}

impl MirrorSeparations {
    /// Builds the set from stored pairs, in either order.
    #[must_use]
    pub fn new(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            pairs: pairs
                .into_iter()
                .map(|(left, right)| Self::key(&left, &right))
                .collect(),
        }
    }

    /// Whether nothing has been separated, so the check can be skipped entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Whether a person stated that these two links are not the same file.
    #[must_use]
    pub fn separated(&self, left: &str, right: &str) -> bool {
        self.pairs.contains(&Self::key(left, right))
    }

    fn key(left: &str, right: &str) -> (String, String) {
        if left <= right {
            (left.to_owned(), right.to_owned())
        } else {
            (right.to_owned(), left.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MirrorSeparations;

    /// A separation is a statement about a pair, not about an order of two arguments.
    #[test]
    fn a_separation_reads_the_same_in_both_directions() {
        let separations = MirrorSeparations::new([("b".to_owned(), "a".to_owned())]);
        assert!(separations.separated("a", "b"));
        assert!(separations.separated("b", "a"));
        assert!(!separations.separated("a", "c"));
        assert!(!separations.is_empty());
        assert!(MirrorSeparations::default().is_empty());
    }
}

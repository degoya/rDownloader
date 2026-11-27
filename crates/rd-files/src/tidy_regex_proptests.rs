//! The package-name regex rules as properties (RD-1490-04): a pattern somebody saved is
//! checked the way it later runs, never blanks a name, and answers in bounded time.
//!
//! Deterministic and short: a fixed seed and no regression file written into the source tree.

use std::time::{Duration, Instant};

use proptest::{collection::vec, prelude::*, sample::select, test_runner::RngSeed};
use rd_core::{MAX_PACKAGE_NAME_REGEX_RULES, PackageNameRegex};

use crate::{
    package_name_regex, tidy_regex::apply_package_name_regex, validate_package_name_regex,
};

/// Far above what a linear engine needs for a package name, far below what a backtracking one
/// takes on the patterns below (`2^4096` steps for `(a+)+$`).
const BUDGET: Duration = Duration::from_secs(5);

/// The pieces of a pattern: syntax that nests, repeats and alternates, and broken syntax.
/// Counted repetitions stay small, so a case stays a millisecond case.
const PIECES: &[&str] = &[
    "a", "b", "c", ".", "*", "+", "?", "*?", "|", "(", ")", "(?:", "(?i)", "(?P<n>", "[a-c]",
    "[^a]", "[0-9]", "^", "$", "\\b", "{2}", "{1,3}", "{0,}", "\\", "[", "]", "{", "}", "\\.", "-",
    "_",
];

/// The patterns that make a backtracking engine take exponential time.
const CATASTROPHIC: &[&str] = &["(a+)+$", "(a|aa)+$", "^(a|a?)+$", "^(a+)+b", "(.*a){12}$"];

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 128,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x1490_0004),
        ..ProptestConfig::default()
    }
}

fn pattern() -> impl Strategy<Value = String> {
    vec(select(PIECES), 0..24).prop_map(|pieces| pieces.concat())
}

/// A replacement: text, group references and broken ones.
fn replacement() -> impl Strategy<Value = String> {
    "[a-c._ $0-9{}n]{0,12}"
}

fn pairs(count: std::ops::Range<usize>) -> impl Strategy<Value = Vec<PackageNameRegex>> {
    vec((pattern(), replacement()), count).prop_map(|pairs| {
        pairs
            .into_iter()
            .map(|(pattern, replacement)| PackageNameRegex {
                pattern,
                replacement,
            })
            .collect()
    })
}

/// A long run of `a` that ends in a character no pattern above expects.
fn adversarial(length: usize) -> String {
    let mut haystack = "a".repeat(length);
    haystack.push('!');
    haystack
}

proptest! {
    #![proptest_config(config())]

    /// A list is saved exactly when every pattern in it compiles the way the rules run it.
    #[test]
    fn saving_agrees_with_running(pairs in pairs(0..MAX_PACKAGE_NAME_REGEX_RULES + 1)) {
        let compiles = pairs.iter().all(|pair| package_name_regex(&pair.pattern).is_ok());
        prop_assert_eq!(validate_package_name_regex(&pairs).is_ok(), compiles);
    }

    /// Whatever the pairs do, a name that was not blank does not come out blank, and the pairs
    /// answer within the budget, on a long name made to stall a backtracking engine too.
    #[test]
    fn the_pairs_never_blank_a_name_and_answer_in_bounded_time(
        name in "\\PC{0,80}",
        pairs in pairs(0..4),
    ) {
        let long = adversarial(255);
        let started = Instant::now();
        let renamed = apply_package_name_regex(&name, &pairs);
        let _ = apply_package_name_regex(&long, &pairs);
        prop_assert!(started.elapsed() < BUDGET, "took {:?}", started.elapsed());
        if !name.trim().is_empty() {
            prop_assert!(!renamed.trim().is_empty(), "{name:?} became {renamed:?}");
        }
    }
}

/// The classic catastrophic patterns finish on a long name: the engine is a linear one, and a
/// change to a backtracking engine fails here instead of stalling the intake.
#[test]
fn a_catastrophic_pattern_answers_in_bounded_time() {
    let haystack = adversarial(4096);
    for pattern in CATASTROPHIC {
        let regex =
            package_name_regex(pattern).unwrap_or_else(|error| panic!("{pattern}: {error}"));
        let started = Instant::now();
        assert!(!regex.is_match(&haystack), "{pattern}");
        assert_eq!(regex.replace_all(&haystack, "x"), haystack, "{pattern}");
        assert!(
            started.elapsed() < BUDGET,
            "{pattern} took {:?}",
            started.elapsed()
        );
    }
}

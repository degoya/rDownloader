//! The LinkFilter name patterns as properties (RD-1490-04): a glob matches the name it was
//! written from, and a regular expression somebody saved answers in bounded time.
//!
//! Deterministic and short: a fixed seed and no regression file written into the source tree.

use std::time::{Duration, Instant};

use proptest::{collection::vec, prelude::*, sample::select, test_runner::RngSeed};
use rd_core::LinkFilterNameSyntax;

use crate::compile_name_pattern;

/// Far above what a linear engine needs for a file name, far below what a backtracking one
/// takes on the patterns below (`2^4096` steps for `(a+)+$`).
const BUDGET: Duration = Duration::from_secs(5);

/// The pieces of a pattern: syntax that nests, repeats and alternates, and broken syntax.
/// Counted repetitions stay small, so a case stays a millisecond case.
const PIECES: &[&str] = &[
    "a", "b", "c", ".", "*", "+", "?", "*?", "|", "(", ")", "(?:", "(?i)", "[a-c]", "[^a]",
    "[0-9]", "^", "$", "\\b", "{2}", "{1,3}", "{0,}", "\\", "[", "]", "{", "}", "\\.", "-",
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

/// A long run of `a` that ends in a character no pattern above expects.
fn adversarial(length: usize) -> String {
    let mut haystack = "a".repeat(length);
    haystack.push('!');
    haystack
}

proptest! {
    #![proptest_config(config())]

    /// A glob without wildcards is the name itself, whatever the name holds; with them, it
    /// still matches the name it was written from, case ignored.
    #[test]
    fn a_glob_matches_the_name_it_was_written_from(name in "\\PC{0,40}") {
        let glob = compile_name_pattern(&name, LinkFilterNameSyntax::Glob);
        prop_assert!(glob.is_some_and(|glob| glob.is_match(&name)), "{name:?}");
    }

    /// `*.ext` matches every stem, the extension in any case.
    #[test]
    fn a_star_glob_matches_every_stem(stem in "\\PC{0,40}", extension in "[a-z0-9]{1,5}") {
        let glob = compile_name_pattern(&format!("*.{extension}"), LinkFilterNameSyntax::Glob);
        let name = format!("{stem}.{}", extension.to_ascii_uppercase());
        prop_assert!(glob.is_some_and(|glob| glob.is_match(&name)), "{name:?}");
    }

    /// Any pattern compiles or is refused, never a panic, and one that compiles answers on a
    /// file name, an adversarial one included, within the budget.
    #[test]
    fn a_saved_pattern_answers_in_bounded_time(
        pattern in pattern(),
        name in "[a-c0-9. ]{0,255}",
    ) {
        let _ = compile_name_pattern(&pattern, LinkFilterNameSyntax::Glob);
        if let Some(regex) = compile_name_pattern(&pattern, LinkFilterNameSyntax::Regex) {
            let adversarial = adversarial(255);
            let started = Instant::now();
            let _ = regex.is_match(&name);
            let _ = regex.is_match(&adversarial);
            prop_assert!(started.elapsed() < BUDGET, "{pattern:?} took {:?}", started.elapsed());
        }
    }
}

/// The classic catastrophic patterns finish on a long name: the engine is a linear one, and a
/// change to a backtracking engine fails here instead of stalling the LinkGrabber.
#[test]
fn a_catastrophic_pattern_answers_in_bounded_time() {
    let haystack = adversarial(4096);
    for pattern in CATASTROPHIC {
        let regex = compile_name_pattern(pattern, LinkFilterNameSyntax::Regex)
            .unwrap_or_else(|| panic!("{pattern} compiles"));
        let started = Instant::now();
        assert!(!regex.is_match(&haystack), "{pattern}");
        assert!(
            started.elapsed() < BUDGET,
            "{pattern} took {:?}",
            started.elapsed()
        );
    }
}

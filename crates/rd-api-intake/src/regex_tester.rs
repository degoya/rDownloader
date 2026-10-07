//! Live regex tester backing the visual regex editor for category rules.
//!
//! Evaluates a candidate `name_regex` against sample file names with the same engine and
//! semantics the collector uses (`regex` crate, unanchored first match). An uncompilable
//! pattern is a regular `valid: false` response, not an HTTP error, so the editor can show
//! live feedback while typing.
//!
//! With a `replacement` the tester also answers what replacing every match makes of each sample
//! — the package-name regex rules' find → replace (RD-1140-05), compiled by the same
//! `rd_files::package_name_regex` those rules run with.
//!
//! A valid pattern also comes back as its structure, the tree the editor draws as a diagram
//! (RD-1140-06, `regex_tester_structure.rs`).

use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiError;

#[path = "regex_tester_structure.rs"]
mod structure;

pub use structure::{RegexFlag, RegexFlagName, RegexNode, RegexNodeKind};

const MAX_PATTERN_BYTES: usize = 2048;
const MAX_SAMPLES: usize = 50;
const MAX_SAMPLE_BYTES: usize = 512;

#[derive(Deserialize, ToSchema)]
pub struct TestRegexRequest {
    pub pattern: String,
    pub samples: Vec<String>,
    /// Replaces every match, `$1`/`${name}` naming groups; each result then carries `replaced`.
    #[serde(default)]
    pub replacement: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct TestRegexResponse {
    pub valid: bool,
    /// Compiler error text when the pattern is invalid.
    pub error: Option<String>,
    /// One entry per sample in request order; empty when the pattern is invalid.
    pub results: Vec<TestRegexSampleResult>,
    /// The pattern's structure as the service parses it, for the editor's diagram; absent when
    /// the pattern is invalid or too large to draw.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structure: Option<RegexNode>,
    /// Why a valid pattern has no structure, as a stable code
    /// (`category_rule.regex_structure_limits`: deeper or larger than a diagram shows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structure_error: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct TestRegexSampleResult {
    pub matched: bool,
    /// First match offsets in UTF-16 code units, ready for JS `String.prototype.slice`.
    pub start: Option<u32>,
    pub end: Option<u32>,
    /// The sample with every match replaced, when the request carried a replacement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
}

fn utf16_offset(text: &str, byte_offset: usize) -> u32 {
    text[..byte_offset].encode_utf16().count() as u32
}

fn evaluate_test_regex(
    pattern: &str,
    samples: &[String],
    replacement: Option<&str>,
) -> TestRegexResponse {
    // Without a replacement this is the routing rules' engine as before; with one, the
    // package-name rules' (the same crate, capped in compiled size).
    let compiled = match replacement {
        None => regex::Regex::new(pattern),
        Some(_) => rd_files::package_name_regex(pattern),
    };
    let regex = match compiled {
        Ok(regex) => regex,
        Err(error) => {
            return TestRegexResponse {
                valid: false,
                error: Some(error.to_string()),
                results: Vec::new(),
                structure: None,
                structure_error: None,
            };
        }
    };
    let results = samples
        .iter()
        .map(|sample| {
            let replaced = replacement.map(|with| regex.replace_all(sample, with).into_owned());
            match regex.find(sample) {
                Some(found) => TestRegexSampleResult {
                    matched: true,
                    start: Some(utf16_offset(sample, found.start())),
                    end: Some(utf16_offset(sample, found.end())),
                    replaced,
                },
                None => TestRegexSampleResult {
                    matched: false,
                    start: None,
                    end: None,
                    replaced,
                },
            }
        })
        .collect();
    let (structure, structure_error) = structure::pattern_structure(pattern);
    TestRegexResponse {
        valid: true,
        error: None,
        results,
        structure,
        structure_error,
    }
}

#[utoipa::path(post, path = "/api/v1/category-rules/test-regex", tag = "configuration", request_body = TestRegexRequest, responses((status = 200, body = TestRegexResponse)))]
pub async fn test_category_rule_regex(
    Json(request): Json<TestRegexRequest>,
) -> Result<Json<TestRegexResponse>, ApiError> {
    if request.pattern.len() > MAX_PATTERN_BYTES
        || request
            .replacement
            .as_ref()
            .is_some_and(|replacement| replacement.len() > MAX_PATTERN_BYTES)
        || request.samples.len() > MAX_SAMPLES
        || request
            .samples
            .iter()
            .any(|sample| sample.len() > MAX_SAMPLE_BYTES)
    {
        return Err(ApiError::bad_request(
            "category_rule.test_regex_limits",
            "Pattern or samples exceed the allowed size",
        ));
    }
    Ok(Json(evaluate_test_regex(
        &request.pattern,
        &request.samples,
        request.replacement.as_deref(),
    )))
}

#[cfg(test)]
mod tests {
    use super::evaluate_test_regex;

    fn samples(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn matches_and_misses_report_offsets() {
        let response = evaluate_test_regex(
            "1080p",
            &samples(&["Movie.1080p.mkv", "Show.720p.mp4"]),
            None,
        );
        assert!(response.valid);
        assert_eq!(response.error, None);
        assert_eq!(response.results.len(), 2);
        assert!(response.results[0].matched);
        assert_eq!(response.results[0].start, Some(6));
        assert_eq!(response.results[0].end, Some(11));
        assert!(!response.results[1].matched);
        assert_eq!(response.results[1].start, None);
    }

    #[test]
    fn case_insensitive_inline_flag_is_supported() {
        let response = evaluate_test_regex("(?i)show", &samples(&["SHOW.S01E01.mkv"]), None);
        assert!(response.valid);
        assert!(response.results[0].matched);
        assert_eq!(response.results[0].start, Some(0));
        assert_eq!(response.results[0].end, Some(4));
    }

    #[test]
    fn invalid_pattern_reports_error_without_results() {
        let response = evaluate_test_regex("(?=broken", &samples(&["anything"]), None);
        assert!(!response.valid);
        assert!(response.error.is_some());
        assert!(response.results.is_empty());
        assert!(response.structure.is_none());
    }

    #[test]
    fn a_valid_pattern_comes_back_with_its_structure() {
        let response = evaluate_test_regex(r"^\d+$", &samples(&["42"]), None);
        let structure = response.structure.expect("a structure");
        assert_eq!(structure.kind, super::RegexNodeKind::Sequence);
        assert_eq!(structure.children.len(), 3);
        assert_eq!(response.structure_error, None);
        // With a replacement too: the package-name rules' patterns are drawn the same way.
        let replacing = evaluate_test_regex("_", &samples(&["a_b"]), Some("."));
        assert_eq!(
            replacing.structure.map(|node| node.kind),
            Some(super::RegexNodeKind::Literal)
        );
    }

    #[test]
    fn offsets_are_utf16_code_units_not_bytes() {
        // "Café1." is 6 chars but 7 bytes; UTF-16 offsets must ignore the extra byte.
        let response = evaluate_test_regex("1080p", &samples(&["Café1.1080p.mkv"]), None);
        assert_eq!(response.results[0].start, Some(6));
        assert_eq!(response.results[0].end, Some(11));
        // An astral-plane emoji occupies two UTF-16 code units.
        let response = evaluate_test_regex("clip", &samples(&["🎬clip.mkv"]), None);
        assert_eq!(response.results[0].start, Some(2));
        assert_eq!(response.results[0].end, Some(6));
    }

    #[test]
    fn empty_samples_yield_empty_results() {
        let response = evaluate_test_regex(".*", &[], None);
        assert!(response.valid);
        assert!(response.results.is_empty());
    }

    #[test]
    fn a_replacement_answers_what_every_sample_becomes() {
        let response = evaluate_test_regex(
            r"_(v\d[\d.]*)_",
            &samples(&["Game_Update_v2.0.2_NSW", "Plain.Name"]),
            Some(" ${1} "),
        );
        assert!(response.valid);
        assert_eq!(
            response.results[0].replaced.as_deref(),
            Some("Game_Update v2.0.2 NSW")
        );
        // A sample without a match comes back as it was.
        assert_eq!(response.results[1].replaced.as_deref(), Some("Plain.Name"));
        assert_eq!(
            evaluate_test_regex("x", &samples(&["x"]), None).results[0].replaced,
            None
        );
    }
}

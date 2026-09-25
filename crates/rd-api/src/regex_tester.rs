//! Live regex tester backing the visual regex editor for category rules.
//!
//! Evaluates a candidate `name_regex` against sample file names with the same engine and
//! semantics the collector uses (`regex` crate, unanchored first match). An uncompilable
//! pattern is a regular `valid: false` response, not an HTTP error, so the editor can show
//! live feedback while typing.

use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ApiError;

const MAX_PATTERN_BYTES: usize = 2048;
const MAX_SAMPLES: usize = 50;
const MAX_SAMPLE_BYTES: usize = 512;

#[derive(Deserialize, ToSchema)]
pub struct TestRegexRequest {
    pub pattern: String,
    pub samples: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct TestRegexResponse {
    pub valid: bool,
    /// Compiler error text when the pattern is invalid.
    pub error: Option<String>,
    /// One entry per sample in request order; empty when the pattern is invalid.
    pub results: Vec<TestRegexSampleResult>,
}

#[derive(Serialize, ToSchema)]
pub struct TestRegexSampleResult {
    pub matched: bool,
    /// First match offsets in UTF-16 code units, ready for JS `String.prototype.slice`.
    pub start: Option<u32>,
    pub end: Option<u32>,
}

fn utf16_offset(text: &str, byte_offset: usize) -> u32 {
    text[..byte_offset].encode_utf16().count() as u32
}

fn evaluate_test_regex(pattern: &str, samples: &[String]) -> TestRegexResponse {
    let regex = match regex::Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => {
            return TestRegexResponse {
                valid: false,
                error: Some(error.to_string()),
                results: Vec::new(),
            };
        }
    };
    let results = samples
        .iter()
        .map(|sample| match regex.find(sample) {
            Some(found) => TestRegexSampleResult {
                matched: true,
                start: Some(utf16_offset(sample, found.start())),
                end: Some(utf16_offset(sample, found.end())),
            },
            None => TestRegexSampleResult {
                matched: false,
                start: None,
                end: None,
            },
        })
        .collect();
    TestRegexResponse {
        valid: true,
        error: None,
        results,
    }
}

#[utoipa::path(post, path = "/api/v1/category-rules/test-regex", tag = "configuration", request_body = TestRegexRequest, responses((status = 200, body = TestRegexResponse)))]
pub async fn test_category_rule_regex(
    Json(request): Json<TestRegexRequest>,
) -> Result<Json<TestRegexResponse>, ApiError> {
    if request.pattern.len() > MAX_PATTERN_BYTES
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
        let response =
            evaluate_test_regex("1080p", &samples(&["Movie.1080p.mkv", "Show.720p.mp4"]));
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
        let response = evaluate_test_regex("(?i)show", &samples(&["SHOW.S01E01.mkv"]));
        assert!(response.valid);
        assert!(response.results[0].matched);
        assert_eq!(response.results[0].start, Some(0));
        assert_eq!(response.results[0].end, Some(4));
    }

    #[test]
    fn invalid_pattern_reports_error_without_results() {
        let response = evaluate_test_regex("(?=broken", &samples(&["anything"]));
        assert!(!response.valid);
        assert!(response.error.is_some());
        assert!(response.results.is_empty());
    }

    #[test]
    fn offsets_are_utf16_code_units_not_bytes() {
        // "Café1." is 6 chars but 7 bytes; UTF-16 offsets must ignore the extra byte.
        let response = evaluate_test_regex("1080p", &samples(&["Café1.1080p.mkv"]));
        assert_eq!(response.results[0].start, Some(6));
        assert_eq!(response.results[0].end, Some(11));
        // An astral-plane emoji occupies two UTF-16 code units.
        let response = evaluate_test_regex("clip", &samples(&["🎬clip.mkv"]));
        assert_eq!(response.results[0].start, Some(2));
        assert_eq!(response.results[0].end, Some(6));
    }

    #[test]
    fn empty_samples_yield_empty_results() {
        let response = evaluate_test_regex(".*", &[]);
        assert!(response.valid);
        assert!(response.results.is_empty());
    }
}

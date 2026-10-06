//! The component: a YouTube link in, a summary of its sponsor segments out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

use plugin_guest_enricher::{
    EnrichField, EnrichSubject, Guest,
    http::{self, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::segments;

struct Component;

impl Guest for Component {
    fn enrich(subject: EnrichSubject) -> Result<Vec<EnrichField>, Failure> {
        // Not a YouTube video: answer with nothing rather than asking SponsorBlock about an
        // address it cannot know. An empty list is a normal answer, not a failure.
        let Some(video) = segments::video_id(&subject.url) else {
            return Ok(Vec::new());
        };
        let response = http::http_request(
            "GET",
            "https://sponsor.ajay.app/api/skipSegments",
            &[
                RequestQuery {
                    name: "videoID".to_owned(),
                    value_template: video,
                },
                // Asking for exactly the categories that get reported: a wider query would
                // hand back segments nothing here reads.
                RequestQuery {
                    name: "categories".to_owned(),
                    value_template: format!(
                        "[{}]",
                        segments::CATEGORIES
                            .iter()
                            .map(|category| format!("\"{category}\""))
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                },
            ],
            &[],
            &[],
        )?;
        // 404 is SponsorBlock's answer for "nobody has submitted anything", which is an
        // answer and not an error: most videos have no segments.
        if response.status == 404 {
            return Ok(Vec::new());
        }
        if !(200..300).contains(&response.status) {
            return Err(Failure {
                category: FailureKind::Transient(None),
                message: format!("SponsorBlock answered {}", response.status),
                code: Some("sponsorblock_enricher.lookup_failed".to_owned()),
                params: Vec::new(),
            });
        }
        let body = String::from_utf8_lossy(&response.body);
        let mut fields = Vec::new();
        let mut total = 0.0;
        for category in segments::CATEGORIES {
            let seconds = segments::totals(&body, category);
            if seconds <= 0.0 {
                continue;
            }
            total += seconds;
            fields.push(EnrichField {
                name: format!("sponsorblock.{category}"),
                value: segments::human_duration(seconds),
            });
        }
        // A single number is what somebody deciding whether to download actually reads; the
        // per-category ones are there for whoever wants the detail.
        if total > 0.0 {
            fields.insert(
                0,
                EnrichField {
                    name: "sponsorblock.skippable".to_owned(),
                    value: segments::human_duration(total),
                },
            );
        }
        Ok(fields)
    }
}

plugin_guest_enricher::enricher_plugin!(Component);

//! The request one worker sends for its chunk, and the checks its response has to pass before
//! a byte of it is believed.

use reqwest::{StatusCode, header};

use crate::ChunkSpec;

use super::{HttpDownloadError, failure::status_failure, worker::Worker};

impl Worker {
    /// The method, body, agent, bound headers and range of this chunk's request.
    pub(super) fn build_request(&self, chunk: &ChunkSpec) -> reqwest::RequestBuilder {
        let mut builder = match self.method {
            rd_core::ReplayMethod::Get => self.client.get(self.url.clone()),
            rd_core::ReplayMethod::Post => {
                let mut post = self.client.post(self.url.clone());
                if let Some(payload) = &self.body {
                    post = post
                        .header(header::CONTENT_TYPE, payload.content_type.as_str())
                        .body(payload.bytes.clone());
                }
                post
            }
        };
        // Per request rather than on the pooled client: the captured agent usually has to
        // match what the origin saw at capture time, but it must not bleed into unrelated
        // transfers that happen to share a client.
        if let Some(agent) = &self.captured_user_agent
            && let Ok(value) = header::HeaderValue::from_str(agent)
        {
            builder = builder.header(header::USER_AGENT, value);
        }
        for (name, value) in self.headers.iter() {
            if let Ok(name) = header::HeaderName::from_bytes(name.as_bytes())
                && let Ok(value) = header::HeaderValue::from_str(value)
            {
                builder = builder.header(name, value);
            }
        }
        if self.require_range {
            let range = match chunk.end {
                Some(end) => format!("bytes={}-{}", chunk.committed, end.saturating_sub(1)),
                None => format!("bytes={}-", chunk.committed),
            };
            builder = builder.header(header::RANGE, range);
            if let Some(validator) = &self.validator {
                builder = builder.header(header::IF_RANGE, validator);
            }
        }
        builder
    }

    /// Refuses a response from an unapproved origin, a changed remote and an error status.
    pub(super) fn check_response(
        &self,
        response: &reqwest::Response,
    ) -> Result<(), HttpDownloadError> {
        // Belt and braces. The client's redirect policy already refuses an unapproved hop,
        // but only the final URL proves that no header and no body reached a foreign
        // origin, and a stopped redirect arrives here as an ordinary 3xx response.
        if !self.approved_origins.is_empty() {
            if !crate::redirect::is_approved(&self.approved_origins, response.url()) {
                return Err(crate::redirect::not_allowed(response.url()));
            }
            if response.status().is_redirection() {
                let target = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| response.url().join(value).ok())
                    .unwrap_or_else(|| response.url().clone());
                return Err(crate::redirect::not_allowed(&target));
            }
        }
        if response.status() == StatusCode::PRECONDITION_FAILED
            || response.status() == StatusCode::RANGE_NOT_SATISFIABLE
        {
            return Err(HttpDownloadError::RemoteChanged);
        }
        if !response.status().is_success() {
            return Err(status_failure(response.status(), response.headers()));
        }
        Ok(())
    }
}

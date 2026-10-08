//! Where a response's body belongs in the part file, and whether it may be continued at all.

use reqwest::{StatusCode, header};

use crate::ChunkSpec;

use super::{
    HttpDownloadError,
    failure::{header_text, not_a_file},
    worker::Worker,
};

/// Whether this chunk layout is responsible for every byte of the file.
///
/// Ascending and gap-free from zero to the end. A layout with a hole in it is not one this
/// engine produces, but it is one a caller could hand over, and a whole-file integrity check
/// made on such a set would be a check of something that was never downloaded.
pub(super) fn layout_covers_file(chunks: &[ChunkSpec], total: Option<u64>) -> bool {
    let mut ordered: Vec<&ChunkSpec> = chunks.iter().collect();
    ordered.sort_by_key(|chunk| chunk.start);
    let mut reach = 0_u64;
    for chunk in ordered {
        if chunk.start > reach {
            return false;
        }
        // An open-ended chunk runs to the end of whatever the server sends.
        let Some(end) = chunk.end else {
            return true;
        };
        reach = reach.max(end);
    }
    total.is_some_and(|total| reach >= total)
}

impl Worker {
    /// The byte offset this response's body is written from.
    pub(super) fn start_position(
        &self,
        chunk: &ChunkSpec,
        response: &reqwest::Response,
    ) -> Result<u64, HttpDownloadError> {
        // A success that is not `206` is not automatically a refusal: it may be the whole
        // file, the requested part described by a header instead of the status, a changed
        // remote, or a page that is not the file at all. They are told apart below.
        let mut position = chunk.committed;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            // `206` says a part is coming; only `Content-Range` says *which* part. Trusting
            // the status alone let a server answer `Range: bytes=8388608-` with the head of
            // the file and have it written at offset 8 MiB, checkpointed and completed --
            // silently corrupt, and worst with several chunks in flight.
            self.check_partial_range(chunk, response)?;
        } else if self.require_range {
            position = self.unranged_start(chunk, response)?;
        }
        Ok(position)
    }

    /// Refuses a `206` whose `Content-Range` is not the range this chunk asked for.
    ///
    /// RFC 9110 requires the header on a single-range `206`, so its absence is as much a
    /// reason to refuse as a mismatch: without it there is nothing that says where the body
    /// belongs, and the only alternative is to guess.
    fn check_partial_range(
        &self,
        chunk: &ChunkSpec,
        response: &reqwest::Response,
    ) -> Result<(), HttpDownloadError> {
        let range = header_text(response, &header::CONTENT_RANGE).unwrap_or_default();
        match crate::probe::parse_content_range_start(&range) {
            Some(start) if start == chunk.committed => self.check_total(chunk, &range),
            _ => Err(HttpDownloadError::RangeIgnored),
        }
    }

    /// Refuses a described range of an entity whose length is not this transfer's (TR-01).
    ///
    /// The start says where the body goes, the total says which file it belongs to. Only the
    /// start used to be compared, so a file that had changed length behind the same address
    /// handed its tail to the bytes of the old one. A validator answers that question too,
    /// through `If-Range`; without one, the length is the only evidence left, and bytes on disk
    /// are continued only when it is confirmed equal.
    fn check_total(&self, chunk: &ChunkSpec, content_range: &str) -> Result<(), HttpDownloadError> {
        match (
            crate::probe::parse_content_range_total(content_range),
            self.total_bytes,
        ) {
            (Some(named), Some(expected)) if named != expected => {
                Err(HttpDownloadError::RemoteChanged)
            }
            (Some(_), Some(_)) => Ok(()),
            _ if self.validator.is_none() && chunk.committed > chunk.start => {
                Err(HttpDownloadError::RemoteChanged)
            }
            _ => Ok(()),
        }
    }

    /// Byte offset the body of a non-`206` success starts at, or why it cannot be used.
    ///
    /// RFC 9110 gives a server several legitimate answers to a conditional range request
    /// and the status code alone tells apart none of them. Treating every non-`206` as a
    /// refusal ended perfectly good downloads: a small file that arrives complete, and the
    /// `200` a server *must* send once an `If-Range` validator no longer matches, were both
    /// reported as "server ignored a required range request".
    fn unranged_start(
        &self,
        chunk: &ChunkSpec,
        response: &reqwest::Response,
    ) -> Result<u64, HttpDownloadError> {
        // The header decides before the status does: a server that answers `200` while
        // describing the range it sends is serving that range.
        if let Some(range) = header_text(response, &header::CONTENT_RANGE)
            && let Some(start) = crate::probe::parse_content_range_start(&range)
        {
            return if start == chunk.committed {
                self.check_total(chunk, &range).map(|()| start)
            } else {
                Err(HttpDownloadError::RangeIgnored)
            };
        }
        // Neither a range nor a file: a throttle notice or a landing page. Worth another
        // attempt in a minute rather than a dead download -- the same judgement the probe
        // already makes, on the same headers.
        let content_type = header_text(response, &header::CONTENT_TYPE);
        if !crate::probe::looks_downloadable(
            header_text(response, &header::CONTENT_DISPOSITION).as_deref(),
            content_type.as_deref(),
            response.content_length(),
        ) {
            return Err(not_a_file(content_type));
        }
        // Bytes are already on disk and the server is sending the entity from its first
        // byte. `If-Range` went out with the request, so this is how RFC 9110 spells "your
        // validator is stale" -- and the partial file must not be continued with it.
        if chunk.committed > chunk.start {
            return Err(HttpDownloadError::RemoteChanged);
        }
        // A complete body is usable exactly when this chunk is the complete file.
        if self.covers_whole_file {
            Ok(chunk.start)
        } else {
            Err(HttpDownloadError::RangeIgnored)
        }
    }
}

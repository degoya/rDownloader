//! The bounds of list-shaped requests (audit 1.9.1): how many ids one bulk request may carry
//! (API-11), and the optional page window of the growing list routes (API-15).
//!
//! Both used to be decided per handler: `MAX_BULK` twice, the literal `500` in four more
//! places, so one route could drift from the others without anyone noticing; the web client's
//! `BULK_LIMIT` batches against this one number.

use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue},
};

use crate::{ApiError, dto::PageQuery, error_codes::bulk_range};

/// The most ids one bulk request may carry; every bulk route refuses more.
///
/// The web client sends a larger selection in batches of this size
/// (`web/src/utils/bulkBatches.ts`).
pub const MAX_BULK: usize = 500;

/// `400` unless a bulk request names between 1 and [`MAX_BULK`] ids.
///
/// # Errors
///
/// [`bulk_range`] with [`MAX_BULK`] for an empty or a longer list.
pub fn validate_bulk(count: usize) -> Result<(), ApiError> {
    if count == 0 || count > MAX_BULK {
        return Err(bulk_range(MAX_BULK));
    }
    Ok(())
}

/// The largest page a list route hands out in one answer.
pub const MAX_PAGE_LIMIT: u32 = 1_000;

/// The header that carries the length of the whole list when a page was asked for.
pub const TOTAL_COUNT_HEADER: HeaderName = HeaderName::from_static("x-total-count");

/// `400` for a page size outside `1..=MAX_PAGE_LIMIT`.
#[must_use]
pub fn page_limit_range() -> ApiError {
    ApiError::bad_request(
        "request.page_limit",
        format!("The page size must be between 1 and {MAX_PAGE_LIMIT}"),
    )
    .with_param("max", MAX_PAGE_LIMIT)
}

/// The page window a request asks for, read from its query string.
///
/// An extractor of its own rather than `Query<PageQuery>`: axum answers a `limit` or `offset`
/// that is not a number with an uncoded plain-text `400`, and every REST error here carries a
/// stable code. A malformed value is refused like one out of range, as `request.page_limit`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Page(pub PageQuery);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Page {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<PageQuery>::try_from_uri(&parts.uri)
            .map(|axum::extract::Query(query)| Self(query))
            .map_err(|_| page_limit_range())
    }
}

/// A checked page window: rows to skip, then rows to keep at most.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageWindow {
    pub offset: usize,
    pub limit: usize,
}

impl PageQuery {
    /// The window this query asks for, or `None` when it asks for none (the whole list).
    ///
    /// Checked before the list is read, so a refused request costs no table load.
    ///
    /// # Errors
    ///
    /// [`page_limit_range`] for a `limit` of 0 or above [`MAX_PAGE_LIMIT`].
    pub fn window(&self) -> Result<Option<PageWindow>, ApiError> {
        if self.limit.is_none() && self.offset.is_none() {
            return Ok(None);
        }
        let limit = match self.limit {
            Some(limit) if limit == 0 || limit > MAX_PAGE_LIMIT => {
                return Err(page_limit_range());
            }
            Some(limit) => usize::try_from(limit).unwrap_or(usize::MAX),
            // An offset alone skips that many rows and returns the rest.
            None => usize::MAX,
        };
        let offset = usize::try_from(self.offset.unwrap_or(0)).unwrap_or(usize::MAX);
        Ok(Some(PageWindow { offset, limit }))
    }
}

/// Cuts the window out of an already ordered list.
///
/// Without a window the list and the answer stay exactly what they were before paging
/// existed: no header, every row. With one, the rows keep the list's own order and
/// [`TOTAL_COUNT_HEADER`] names how many there are in all; an offset past the end is an
/// empty page, not an error.
///
/// The rows are still read whole from the database and sliced here: the answer is bounded,
/// the load is not. Paging in SQL is a follow-up for `rd-db`.
pub fn paged<T>(window: Option<PageWindow>, rows: Vec<T>) -> (HeaderMap, Json<Vec<T>>) {
    let Some(window) = window else {
        return (HeaderMap::new(), Json(rows));
    };
    let mut headers = HeaderMap::new();
    headers.insert(TOTAL_COUNT_HEADER, HeaderValue::from(rows.len()));
    let page = rows
        .into_iter()
        .skip(window.offset)
        .take(window.limit)
        .collect();
    (headers, Json(page))
}

/// The header of a page the database cut itself (RD-1100-04): the same [`TOTAL_COUNT_HEADER`]
/// [`paged`] sends, sent under the same rule — only when a window was asked for.
#[must_use]
pub fn total_header(window: Option<PageWindow>, total: u64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if window.is_some() {
        headers.insert(TOTAL_COUNT_HEADER, HeaderValue::from(total));
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::{MAX_BULK, MAX_PAGE_LIMIT, PageWindow, TOTAL_COUNT_HEADER, paged, validate_bulk};
    use crate::dto::PageQuery;

    fn query(limit: Option<u32>, offset: Option<u32>) -> PageQuery {
        PageQuery { limit, offset }
    }

    #[test]
    fn no_parameters_leave_the_list_and_the_headers_alone() {
        let window = query(None, None).window().expect("no window");
        assert_eq!(window, None);
        let (headers, rows) = paged(window, vec![1, 2, 3]);
        assert!(headers.is_empty());
        assert_eq!(rows.0, vec![1, 2, 3]);
    }

    #[test]
    fn a_window_keeps_the_order_and_names_the_total() {
        let window = query(Some(2), Some(1)).window().expect("window");
        let (headers, rows) = paged(window, vec![1, 2, 3, 4]);
        assert_eq!(rows.0, vec![2, 3]);
        assert_eq!(
            headers
                .get(TOTAL_COUNT_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("4")
        );
    }

    #[test]
    fn an_offset_alone_returns_the_rest_and_past_the_end_nothing() {
        let rest = paged(
            query(None, Some(2)).window().expect("window"),
            vec![1, 2, 3],
        );
        assert_eq!(rest.1.0, vec![3]);
        let beyond = paged(
            query(Some(5), Some(9)).window().expect("window"),
            vec![1, 2],
        );
        assert!(beyond.1.0.is_empty());
    }

    #[test]
    fn a_limit_outside_the_range_is_refused() {
        for limit in [0, MAX_PAGE_LIMIT + 1] {
            let error = query(Some(limit), None).window().expect_err("refused");
            assert_eq!(error.code(), "request.page_limit");
        }
        assert_eq!(
            query(Some(MAX_PAGE_LIMIT), None).window().expect("window"),
            Some(PageWindow {
                offset: 0,
                limit: MAX_PAGE_LIMIT as usize
            })
        );
    }

    #[test]
    fn a_bulk_request_holds_one_to_the_maximum() {
        assert_eq!(
            validate_bulk(0).expect_err("empty").code(),
            "request.bulk_range"
        );
        assert!(validate_bulk(1).is_ok());
        assert!(validate_bulk(MAX_BULK).is_ok());
        assert!(validate_bulk(MAX_BULK + 1).is_err());
    }
}

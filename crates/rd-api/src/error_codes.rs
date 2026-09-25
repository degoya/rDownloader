//! Shared error codes and constructors for messages that several handlers emit.
//!
//! Convention: `<domain>.<subject>_<condition>` in snake_case, one dot between domain
//! and subject (`package.not_found`, `auth.invalid_credentials`). Codes are stable API:
//! the web client maps them to translated texts; unknown codes fall back to the
//! English `message`.

use rd_db::StoreErrorKind;

use crate::ApiError;

pub const INTERNAL_ERROR: &str = "internal.error";

/// `404` for a package id that does not exist.
#[must_use]
pub fn package_not_found() -> ApiError {
    ApiError::not_found("package.not_found", "Package not found")
}

/// `404` for a download id that does not exist.
#[must_use]
pub fn download_not_found() -> ApiError {
    ApiError::not_found("download.not_found", "Download not found")
}

/// `404` for an NNTP server id that does not exist.
#[must_use]
pub fn usenet_server_not_found() -> ApiError {
    ApiError::not_found("usenet.server_not_found", "NNTP server not found")
}

/// `404` for a provider account id that does not exist.
#[must_use]
pub fn account_not_found() -> ApiError {
    ApiError::not_found("account.not_found", "Provider account not found")
}

/// `401` for wrong credentials.
#[must_use]
pub fn invalid_credentials() -> ApiError {
    ApiError::unauthorized("auth.invalid_credentials", "Invalid credentials")
}

/// `400` for bulk requests outside `1..=max` ids.
#[must_use]
pub fn bulk_range(max: usize) -> ApiError {
    ApiError::bad_request(
        "request.bulk_range",
        format!("Provide between 1 and {max} ids"),
    )
    .with_param("max", max)
}

/// `400` for a reorder whose id list is not exactly the members of one package.
///
/// Both reorder endpoints hand out the positions 1..n from the list they are given. A list that
/// omits a member, repeats one, or names a row of another package therefore writes an order
/// nobody asked for — or, where the `UPDATE` is fenced by `package_id`, writes nothing at all
/// and reports success. Rejecting the request is the only answer that stays honest.
#[must_use]
pub fn reorder_ids_mismatch(expected: usize, provided: usize) -> ApiError {
    ApiError::bad_request(
        "request.reorder_ids_mismatch",
        "Provide exactly the ids of this package, each of them once",
    )
    .with_param("expected", expected)
    .with_param("provided", provided)
}

/// Checks a reorder list against the members it is supposed to reorder.
///
/// Accepts only a permutation: same length, no duplicates, no id from anywhere else.
pub fn validate_reorder<T: Copy + Eq + std::hash::Hash>(
    members: &[T],
    ids: &[T],
) -> Result<(), ApiError> {
    let listed: std::collections::HashSet<T> = ids.iter().copied().collect();
    if listed.len() != ids.len()
        || listed.len() != members.len()
        || !members.iter().all(|id| listed.contains(id))
    {
        return Err(reorder_ids_mismatch(members.len(), ids.len()));
    }
    Ok(())
}

/// Parses a typed id from its string form, as `request.invalid_id`.
///
/// Every path parameter that arrives as a `String` needs this, and five handlers had written
/// their own copy of it — each free to drift into a different code for the same mistake, which
/// the web client then cannot translate. One parser keeps the code stable across all of them.
pub fn parse_id<T>(value: &str) -> Result<T, ApiError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error: T::Err| ApiError::bad_request("request.invalid_id", error.to_string()))
}

/// Maps a store's "no such row" onto a coded `404`, and anything else onto the generic `500`.
///
/// The reason comes from [`rd_db::store_kind`], not from the sentence the store rendered. Reading
/// the sentence is what this used to do, and a reworded bail in `rd-db` then turned a documented
/// `404` into a `500` with nothing to compile-check.
#[must_use]
pub fn store_not_found(
    error: &anyhow::Error,
    code: &'static str,
    message: &'static str,
) -> ApiError {
    if rd_db::store_kind(error) == Some(StoreErrorKind::NotFound) {
        ApiError::not_found(code, message)
    } else {
        ApiError::from(anyhow::anyhow!(error.to_string()))
    }
}

/// The same, for a store that also refuses on one other ground.
///
/// `conflict_kind` is the reason that store reports for the refusal — [`StoreErrorKind::InUse`]
/// where a reference blocks the removal, [`StoreErrorKind::Busy`] where the collector is working
/// on the row. It is a parameter rather than a constant because which refusal an endpoint expects
/// belongs to the endpoint; passing the wrong one now names a variant that exists, instead of a
/// phrase that may not.
#[must_use]
pub fn store_error(
    error: &anyhow::Error,
    not_found_code: &'static str,
    not_found_message: &'static str,
    conflict_kind: StoreErrorKind,
    conflict_code: &'static str,
    conflict_message: &'static str,
) -> ApiError {
    match rd_db::store_kind(error) {
        Some(StoreErrorKind::NotFound) => ApiError::not_found(not_found_code, not_found_message),
        Some(kind) if kind == conflict_kind => ApiError::conflict(conflict_code, conflict_message),
        // Spelled out rather than written `_`, and this is the one place in the crate where that
        // matters: a new `StoreErrorKind` fails to compile here until somebody has decided
        // whether it is a refusal an endpoint names or a fault that stays a `500`. An `_` would
        // decide it silently, which is how the prose matching went wrong in the first place.
        Some(
            StoreErrorKind::InUse
            | StoreErrorKind::Busy
            | StoreErrorKind::Duplicate
            | StoreErrorKind::WrongState
            | StoreErrorKind::UnknownMediaVariant
            | StoreErrorKind::NoMediaMetadata
            | StoreErrorKind::NoEnqueueableLinks,
        )
        | None => ApiError::from(anyhow::anyhow!(error.to_string())),
    }
}

/// Enqueueing a capture that sends credentials before anyone approved it.
pub const REPLAY_CONSENT_REQUIRED: &str = "replay.consent_required";
/// The captured request changed after consent was granted.
pub const REPLAY_TEMPLATE_CHANGED: &str = "replay.template_changed";
/// The capture cannot be reproduced at all; the `reason` parameter says why.
pub const REPLAY_NOT_REPLAYABLE: &str = "replay.not_replayable";
/// The candidate carries no captured request to preview or approve.
pub const REPLAY_TEMPLATE_MISSING: &str = "replay.template_missing";
/// Consent named an origin the server did not derive from the capture.
pub const REPLAY_ORIGIN_NOT_APPROVED: &str = "replay.origin_not_approved";
/// A media link would be queued without a variant selection, a row that can only fail
/// (RD-120-50).
pub const MEDIA_SELECTION_MISSING: &str = "collector.media_selection_missing";

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use rd_db::StoreErrorKind;

    #[test]
    fn a_reorder_list_has_to_be_a_permutation_of_the_members() {
        let members = [1, 2, 3];
        assert!(super::validate_reorder(&members, &[3, 1, 2]).is_ok());
        // Incomplete, duplicated, foreign, and longer than the package — every one of these
        // used to be accepted and silently produce an order the caller never asked for.
        for ids in [vec![1, 2], vec![1, 1, 2], vec![1, 2, 9], vec![1, 2, 3, 4]] {
            let error = super::validate_reorder(&members, &ids).expect_err("rejected");
            assert_eq!(error.code(), "request.reorder_ids_mismatch");
        }
    }

    /// A store refusal arrives as the documented status and code, and the English does not matter.
    ///
    /// This is the regression the substring matching left wide open: `rd-api` picked `404` against
    /// `409` against `500` by searching the rendered message, so rewording a bail in `rd-db` turned
    /// a documented response into an internal error with nothing to compile-check. No test caught
    /// it, because every test asserted the code the mapping produced rather than the sentence it
    /// depended on.
    ///
    /// The messages here are deliberately nonsense. A mapping that went back to reading them would
    /// fail this test instead of quietly answering `500`, and the last case proves the point from
    /// the other side: the exact English the old matcher looked for, with no reason attached, is
    /// an internal error.
    #[test]
    fn a_store_refusal_keeps_its_documented_status_and_code() {
        use axum::response::IntoResponse as _;

        let missing = anyhow::Error::new(rd_db::StoreError::not_found("mlqaa xyzzy"));
        let error = super::store_not_found(&missing, "subscription.not_found", "Subscription");
        assert_eq!(error.code(), "subscription.not_found");
        assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);

        let referenced = anyhow::Error::new(rd_db::StoreError::in_use("mlqaa xyzzy"));
        for (error, code, status) in [
            (&missing, "category.not_found", StatusCode::NOT_FOUND),
            (&referenced, "category.in_use", StatusCode::CONFLICT),
        ] {
            let mapped = super::store_error(
                error,
                "category.not_found",
                "Category not found",
                StoreErrorKind::InUse,
                "category.in_use",
                "The category is still used",
            );
            assert_eq!(mapped.code(), code);
            assert_eq!(mapped.into_response().status(), status);
        }

        // A reason this endpoint does not expect is not silently promoted to its conflict.
        let busy = anyhow::Error::new(rd_db::StoreError::busy("mlqaa xyzzy"));
        let mapped = super::store_error(
            &busy,
            "category.not_found",
            "Category not found",
            StoreErrorKind::InUse,
            "category.in_use",
            "The category is still used",
        );
        assert_eq!(mapped.code(), super::INTERNAL_ERROR);
        assert_eq!(
            mapped.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let prose = anyhow::anyhow!("category not found, still used");
        let mapped = super::store_not_found(&prose, "category.not_found", "Category not found");
        assert_eq!(mapped.code(), super::INTERNAL_ERROR);
        assert_eq!(
            mapped.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn code_convention_is_followed() {
        let pattern = regex::Regex::new(r"^[a-z]+\.[a-z0-9_]+$").expect("regex");
        for error in [
            super::package_not_found(),
            super::download_not_found(),
            super::usenet_server_not_found(),
            super::account_not_found(),
            super::invalid_credentials(),
            super::bulk_range(500),
            super::reorder_ids_mismatch(3, 2),
            super::parse_id::<rd_core::DownloadId>("not-a-uuid").expect_err("rejected"),
        ] {
            assert!(pattern.is_match(error.code()), "{}", error.code());
        }
        for code in [
            super::INTERNAL_ERROR,
            super::REPLAY_CONSENT_REQUIRED,
            super::REPLAY_TEMPLATE_CHANGED,
            super::REPLAY_NOT_REPLAYABLE,
            super::REPLAY_TEMPLATE_MISSING,
            super::REPLAY_ORIGIN_NOT_APPROVED,
        ] {
            assert!(pattern.is_match(code), "{code}");
        }
    }
}

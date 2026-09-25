//! The typed reason a store refused, carried as a cause on the `anyhow` error.
//!
//! Every store function in this crate returns `anyhow::Result` and is called from six crates, so
//! giving them all a `thiserror` signature would rewrite call sites far outside this one. What
//! `rd-api` actually needs is narrower than a full error type: the *reason* a store said no, so it
//! can pick the documented HTTP status and the stable error code the web client translates.
//! [`StoreError`] carries that reason, and because `anyhow` keeps causes downcastable, a caller
//! asks [`store_kind`] instead of reading the sentence.
//!
//! The failure this prevents: `rd-api` used to choose `404` against `409` against `500` by
//! searching the rendered message for `"not found"` or `"still used"`. Rewording a bail in this
//! crate therefore turned a documented `404` into a `500` — nothing to compile-check, and no test
//! that would notice, because the tests assert the code the mapping produced rather than the
//! sentence it matched.
//!
//! Messages stay exactly as they were. They are still what a log or a `500` body shows; they are
//! simply no longer load-bearing.

/// Why a store refused, at the granularity the REST layer has to distinguish.
///
/// Each variant exists because some handler answers it differently from a generic `500`. Which
/// *code* a refusal gets is still the handler's business — two endpoints both report
/// [`Self::NotFound`] and name different subjects — so this enum names the reason, never the
/// response.
///
/// It is deliberately **not** `#[non_exhaustive]`: adding a reason here should stop the build in
/// `rd_api::error_codes` until somebody decides what it means over HTTP. That decision is exactly
/// what the prose matching used to make silently, and wrongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreErrorKind {
    /// No row with that id.
    NotFound,
    /// Removal refused because another row still references this one.
    InUse,
    /// The collector or the enqueue pipeline is holding the row right now.
    Busy,
    /// A uniqueness constraint already holds a row like the one asked for.
    Duplicate,
    /// The row exists, but its state does not allow this operation.
    WrongState,
    /// A media variant was named that this link does not offer.
    UnknownMediaVariant,
    /// The link carries no media metadata, so there are no variants to choose between.
    NoMediaMetadata,
    /// The package holds nothing that could be enqueued.
    NoEnqueueableLinks,
}

/// A refusal from a store, with the message it has always carried.
///
/// Constructed through the named constructors rather than a literal, so the reason and the
/// sentence are written next to each other and cannot drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    kind: StoreErrorKind,
    message: String,
}

impl StoreError {
    /// Builds a refusal with an explicit reason.
    pub fn new(kind: StoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// The reason, for a caller deciding how to report it.
    #[must_use]
    pub fn kind(&self) -> StoreErrorKind {
        self.kind
    }

    /// No row with that id.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::NotFound, message)
    }

    /// Something else still references the row.
    pub fn in_use(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::InUse, message)
    }

    /// The collector or the enqueue pipeline holds the row.
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Busy, message)
    }

    /// A uniqueness constraint already holds a row like this one.
    pub fn duplicate(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Duplicate, message)
    }

    /// The row's state forbids the operation.
    pub fn wrong_state(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::WrongState, message)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StoreError {}

/// The reason behind an error, if a store put one there.
///
/// Walks the whole `anyhow` cause chain, so it finds the reason whether the store built the error
/// outright (`bail!`, `ensure!`) or attached it to a driver error with `.context(...)`.
#[must_use]
pub fn store_kind(error: &anyhow::Error) -> Option<StoreErrorKind> {
    error.downcast_ref::<StoreError>().map(StoreError::kind)
}

/// Tags SQLite's uniqueness refusal, and leaves every other driver error untouched.
///
/// A duplicate is the one constraint a REST caller can realistically trip, and it is a `409`, not
/// a `500`. `rd-api` used to recognise it by searching the driver's message for `"UNIQUE"` — which
/// is SQLite's wording, not ours, and changes when the driver does. Anything that is not a
/// uniqueness violation passes through unwrapped, so its `500` still reports the driver's own
/// message rather than this one.
pub(crate) fn tag_duplicate(error: sqlx::Error, message: &'static str) -> anyhow::Error {
    let duplicate = error
        .as_database_error()
        .is_some_and(|database| matches!(database.kind(), sqlx::error::ErrorKind::UniqueViolation));
    if duplicate {
        return anyhow::Error::new(error).context(StoreError::duplicate(message));
    }
    anyhow::Error::new(error)
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::{StoreError, StoreErrorKind, store_kind};

    #[test]
    fn a_reason_survives_being_bailed_and_being_attached_as_context() {
        let bailed = anyhow::Error::new(StoreError::not_found("category not found"));
        assert_eq!(store_kind(&bailed), Some(StoreErrorKind::NotFound));
        assert_eq!(bailed.to_string(), "category not found");

        // The `.context(...)` form keeps the driver error as the cause, and the rendered message
        // is still only the store's sentence — the same thing a caller saw before.
        let attached = Err::<(), _>(std::io::Error::other("disk"))
            .context(StoreError::in_use("category is still used"))
            .expect_err("error");
        assert_eq!(store_kind(&attached), Some(StoreErrorKind::InUse));
        assert_eq!(attached.to_string(), "category is still used");
    }

    #[test]
    fn an_untagged_error_has_no_reason() {
        let plain = anyhow::anyhow!("category not found");
        assert_eq!(store_kind(&plain), None);
    }
}

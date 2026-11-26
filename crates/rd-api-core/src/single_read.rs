//! Single reads, `GET <collection>/{id}` (RD-1480-10, API-6): what they share.
//!
//! A single read answers the row its list answers -- the same shape, the same scope, and nothing
//! the list leaves out -- so it is built on that list. A configuration table is a handful of rows
//! and its list is one query, so the handler asks the list and keeps one row; the tables that grow
//! (downloads, packages, the LinkGrabber, NZB imports) read their row alone.

use crate::extract::Json;

use crate::ApiError;

/// The row `wanted` names out of a list's rows, or `missing`'s `404`.
///
/// # Errors
///
/// `missing` when no row is the wanted one.
pub fn one_of<T>(
    rows: Vec<T>,
    wanted: impl Fn(&T) -> bool,
    missing: impl FnOnce() -> ApiError,
) -> Result<Json<T>, ApiError> {
    rows.into_iter()
        .find(|row| wanted(row))
        .map(Json)
        .ok_or_else(missing)
}

#[cfg(test)]
mod tests {
    use super::one_of;
    use crate::error_codes::package_not_found;

    #[test]
    fn the_wanted_row_or_the_coded_refusal() {
        let found = one_of(vec![1, 2, 3], |row| *row == 2, package_not_found).expect("the row");
        assert_eq!(found.0, 2);
        let refused =
            one_of(vec![1, 2, 3], |row| *row == 9, package_not_found).expect_err("no such row");
        assert_eq!(refused.code(), "package.not_found");
    }
}

//! The XFS base script's own form values, bound once (RD-1120-10, PL-2).
//!
//! `ddownload`, `katfile`, `filejoker` and `xfs-generic` each bound [`crate::page`] and
//! [`crate::free`] to the same four constants in a page module of their own, and none of them
//! overrides one: JD's `XFileSharingProBasic.findFormDownload1Free` finds the first free form by
//! `op` = `download1` and fills `method_free` with `"Free Download"` when the page leaves it
//! empty, the second form is found by `op` = `download2`, and the premium submission carries
//! `"Premium Download"` (`plugins/ddownload/src/page.rs` holds the full IMPL-VERIFY record). A
//! plugin re-exports what it uses, so its call sites keep their `page::` names.

/// The `op` value of the first free form.
pub const OP_DOWNLOAD1: &str = "download1";
/// The `op` value of the second free form, which is also the premium form.
pub const OP_DOWNLOAD2: &str = "download2";
/// The free button's label when the page carries none.
pub const FREE_BUTTON: &str = "Free Download";
/// The premium button's label.
pub const PREMIUM_BUTTON: &str = "Premium Download";

/// Hidden form fields of the `download1` form, the first step of the free flow.
#[must_use]
pub fn download1_form(html: &str) -> Option<Vec<(String, String)>> {
    crate::page::download_form(html, OP_DOWNLOAD1)
}

/// Hidden form fields of the `download2` form: the second free step, and the premium form.
#[must_use]
pub fn download2_form(html: &str) -> Option<Vec<(String, String)>> {
    crate::page::download_form(html, OP_DOWNLOAD2)
}

/// The free submission with the script's own button label (keeps `method_free`, drops the
/// premium marker) — the counterpart of [`premium_form`].
#[must_use]
pub fn free_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    crate::free::free_form(fields, FREE_BUTTON)
}

/// The premium submission JDownloader sends for the raw `download2` fields.
#[must_use]
pub fn premium_form(fields: &[(String, String)]) -> Vec<(String, String)> {
    crate::page::premium_form(fields, PREMIUM_BUTTON)
}

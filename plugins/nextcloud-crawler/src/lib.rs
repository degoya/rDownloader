//! Public Nextcloud and ownCloud folder shares in the LinkGrabber (RD-107-05).
//!
//! The service JDownloader does not cover at all: its 857 decrypters contain nothing for
//! Nextcloud, ownCloud, Seafile or WebDAV. rDownloader can, because a share has a documented
//! public DAV endpoint that needs no key and no account — `/public.php/dav/files/<token>`
//! from Nextcloud 29 on, `/public.php/webdav/` before that and on ownCloud.
//!
//! Three host decisions from RD-107-05 make this plugin possible, and it exists partly to
//! prove them: a crawler may send `PROPFIND` without being handed the writing methods, a
//! crawler whose manifest says `*` is narrowed to the host of the address it was given, and a
//! crawler that claims by the shape of a path — `/s/<token>` is not a Nextcloud-specific
//! shape — can say "not mine after all" and have the address carried on.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`propfind`] reads an answer, [`walk`] bounds the tree.
//! `guest` is the thin wrapper around the three and exists only on `wasm32`.

pub mod messages;
pub mod propfind;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;

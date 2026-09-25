//! Resolving one FTP link into the listing the LinkGrabber reviews.

use anyhow::Result;
use rd_core::{
    ByteCount, Failure, RemoteCredential, RemoteEntry, RemoteListing, RemoteTarget,
    is_safe_relative_path,
};

use crate::{client::Connection, error};

/// What a link turned out to be.
pub enum Probed {
    /// The listing, plus the login that reached it.
    Resolved(Box<RemoteListing>),
    /// The server could be reached but the link could not be used.
    Failed(Failure),
}

/// Probes `target` with `credential`.
///
/// A single file and a directory are both returned as a [`RemoteListing`]; the difference
/// is `single_file`, which the review UI uses to skip the tree for a one-file link.
pub async fn probe(connection: &mut Connection, target: &RemoteTarget) -> Result<Probed> {
    let supports_resume = match connection.features().await {
        Ok(features) => crate::client::advertises_rest(&features),
        // A server without FEAT is old but usable; it just cannot promise REST up front.
        Err(_) => false,
    };
    let path = normalize(&target.path);

    // SIZE only succeeds for a plain file, which makes it the cheapest file/directory
    // test that does not depend on the server's listing dialect.
    if let Ok(size) = connection.size(&path).await {
        let name = file_name(&path);
        if !is_safe_relative_path(&name) {
            return Ok(Probed::Failed(Failure::coded(
                rd_core::FailureKind::Permanent,
                error::UNSAFE_PATH,
                "The remote file name cannot be stored safely",
            )));
        }
        let modified = connection
            .modified_at(&path)
            .await
            .ok()
            .map(|naive| naive.and_utc());
        return Ok(Probed::Resolved(Box::new(RemoteListing {
            root: parent(&path),
            single_file: true,
            entries: vec![RemoteEntry {
                path: name,
                is_dir: false,
                size: ByteCount::new(size as u64).ok(),
                modified,
                etag: None,
            }],
            truncated: None,
            supports_resume,
        })));
    }

    // Not a file: it is either a directory or nothing at all, and CWD tells them apart.
    if let Err(failure) = connection.cwd(&path).await {
        return Ok(Probed::Failed(error::classify(&failure)));
    }
    // Ask the server what it actually resolved the path to, so a relative link or a
    // symlinked directory is walked under its real name.
    let root = connection.pwd().await.unwrap_or(path);
    let mut listing = crate::listing::walk(connection, &root).await?;
    listing.supports_resume = supports_resume;
    Ok(Probed::Resolved(Box::new(listing)))
}

/// The credential that should be used for `target`, or a coded failure explaining that
/// none is configured.
pub fn require_credential(
    credential: Option<RemoteCredential>,
    target: &RemoteTarget,
) -> Result<RemoteCredential, Failure> {
    credential.ok_or_else(|| error::no_credential(&target.host))
}

/// Collapses `//` and a trailing slash, and maps an empty path to the server root.
fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    trimmed.to_owned()
}

fn file_name(path: &str) -> String {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => path[..index].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{file_name, normalize, parent};

    #[test]
    fn paths_normalize_to_a_server_absolute_form() {
        assert_eq!(normalize("/pub/a.bin"), "/pub/a.bin");
        assert_eq!(normalize("/pub/"), "/pub");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize(""), "/");
    }

    #[test]
    fn file_and_parent_split_the_path() {
        assert_eq!(file_name("/pub/dir/a.bin"), "a.bin");
        assert_eq!(parent("/pub/dir/a.bin"), "/pub/dir");
        // A file directly under the root keeps the root as its parent.
        assert_eq!(parent("/a.bin"), "/");
        assert_eq!(file_name("/a.bin"), "a.bin");
    }
}

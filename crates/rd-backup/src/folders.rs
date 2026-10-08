//! The folders a backup or a restore keeps plain copies in.

use std::path::Path;

/// Creates `folder` for the service account alone and makes an existing one so (security review
/// 2026-09-30, finding 8): it holds plain copies of the database, readable by every account on
/// the machine under the default umask. The staging of a full backup, the verification's scratch
/// and the restore's uploads, fetches and work folders are such folders as well (RD-1190-22): in
/// a data directory that already exists open -- a Docker volume -- `create_dir_all` left them so.
///
/// # Errors
///
/// When the folder cannot be created or narrowed.
pub async fn private_folder(folder: &Path) -> std::io::Result<()> {
    let folder = folder.to_path_buf();
    tokio::task::spawn_blocking(move || {
        rd_files::create_private_dir_all(&folder)?;
        rd_files::restrict_to_owner(&folder)
    })
    .await
    .map_err(std::io::Error::other)?
}

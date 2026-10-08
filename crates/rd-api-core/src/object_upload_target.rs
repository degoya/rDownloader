//! An `object-storage:<profile>/<bucket>/<prefix>` upload target, checked against the profiles
//! when it is saved (RD-1190-20).
//!
//! A token with `api:config` writes upload targets; the profiles and their credentials belong to
//! `api:secrets`. Before this check a target could name any profile id with any bucket, and a
//! profile bound to one bucket signed uploads into another. Now the target has to name a profile
//! that exists, and a bound profile only its own bucket. The upload checks the binding again
//! when it runs (`rd_object_storage`'s `split_destination`), for a profile changed later.

use rd_core::ObjectStorageProfileId;

use crate::ApiError;

const PREFIX: &str = "object-storage:";

/// Refuses an object storage target whose profile does not exist, whose bucket the profile's
/// binding excludes, or that names no valid bucket. Every other remote, and none, passes.
pub async fn check_object_upload_target(
    database: &rd_db::Database,
    remote: Option<&str>,
) -> Result<(), ApiError> {
    let Some(rest) = remote.and_then(|remote| remote.trim().strip_prefix(PREFIX)) else {
        return Ok(());
    };
    let (profile_id, destination) = rest.split_once('/').unwrap_or((rest, ""));
    let unknown = || {
        ApiError::bad_request(
            "object_storage.upload_target_profile_unknown",
            "The upload target names no existing object storage profile",
        )
    };
    let id = profile_id
        .trim()
        .parse::<ObjectStorageProfileId>()
        .map_err(|_| unknown())?;
    let profile = database
        .object_storage_profile(id)
        .await?
        .ok_or_else(unknown)?;
    let named = destination.trim().trim_matches('/');
    let named = named.split('/').next().unwrap_or_default();
    if !named.is_empty() && !profile.serves_bucket(named) {
        return Err(ApiError::bad_request(
            "object_storage.upload_target_bucket_refused",
            "The profile is bound to another bucket than the upload target names",
        )
        .with_param("bucket", profile.bucket.clone().unwrap_or_default()));
    }
    let bucket = if named.is_empty() {
        profile.bucket.as_deref()
    } else {
        Some(named)
    };
    if !bucket.is_some_and(|bucket| profile.provider.is_valid_bucket(bucket)) {
        return Err(ApiError::bad_request(
            "object_storage.bucket_invalid",
            "The bucket name is not valid",
        ));
    }
    Ok(())
}

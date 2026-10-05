//! Database facade methods for object storage profiles and uploads (RD-150-04).

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{ObjectStorageProfile, ObjectStorageProfileId};

use crate::{
    Database,
    commands::ObjectStorageCommand,
    object_storage_store::{self, NewObjectStorageProfile, ObjectUpload, ObjectUploadPart},
    writer,
};

impl Database {
    /// Every object storage profile, including disabled ones, by name.
    pub async fn list_object_storage_profiles(&self) -> Result<Vec<ObjectStorageProfile>> {
        object_storage_store::list(&self.readers).await
    }

    pub async fn object_storage_profile(
        &self,
        id: ObjectStorageProfileId,
    ) -> Result<Option<ObjectStorageProfile>> {
        object_storage_store::get(&self.readers, id).await
    }

    pub async fn create_object_storage_profile(
        &self,
        input: NewObjectStorageProfile,
    ) -> Result<ObjectStorageProfile> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::CreateObjectStorageProfile {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Updates a profile and returns it with the secret references that fell out of use.
    pub async fn update_object_storage_profile(
        &self,
        id: ObjectStorageProfileId,
        input: NewObjectStorageProfile,
    ) -> Result<(ObjectStorageProfile, Vec<String>)> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::UpdateObjectStorageProfile {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Deletes a profile and its upload records; returns the orphaned secret references.
    pub async fn delete_object_storage_profile(
        &self,
        id: ObjectStorageProfileId,
    ) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::DeleteObjectStorageProfile { id, reply }
        })
        .await
    }

    /// The upload recorded for one destination object.
    pub async fn object_upload(
        &self,
        profile_id: ObjectStorageProfileId,
        bucket: &str,
        object_key: &str,
    ) -> Result<Option<ObjectUpload>> {
        object_storage_store::upload(&self.readers, profile_id, bucket, object_key).await
    }

    /// Recorded uploads of one profile, or of every profile started before `before`.
    pub async fn object_uploads(
        &self,
        profile_id: Option<ObjectStorageProfileId>,
        before: Option<DateTime<Utc>>,
    ) -> Result<Vec<ObjectUpload>> {
        object_storage_store::uploads(&self.readers, profile_id, before).await
    }

    pub async fn begin_object_upload(&self, upload: ObjectUpload) -> Result<()> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::BeginObjectUpload {
                upload: Box::new(upload),
                reply,
            }
        })
        .await
    }

    pub async fn record_object_upload_part(
        &self,
        id: String,
        part: ObjectUploadPart,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::RecordObjectUploadPart { id, part, reply }
        })
        .await
    }

    pub async fn complete_object_upload(&self, id: String) -> Result<()> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::CompleteObjectUpload { id, reply }
        })
        .await
    }

    /// Forgets one upload record by id, or every record of an owner.
    pub async fn forget_object_uploads(
        &self,
        id: Option<String>,
        owner: Option<String>,
    ) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            ObjectStorageCommand::ForgetObjectUploads { id, owner, reply }
        })
        .await
    }
}

//! The commands of `writer/object_storage.rs`.

use super::Reply;

/// The commands `Writer::handle_object_storage` applies.
pub(crate) enum ObjectStorageCommand {
    /// Object storage profiles (RD-150-04).
    CreateObjectStorageProfile {
        input: Box<crate::NewObjectStorageProfile>,
        reply: Reply<rd_core::ObjectStorageProfile>,
    },
    UpdateObjectStorageProfile {
        id: rd_core::ObjectStorageProfileId,
        input: Box<crate::NewObjectStorageProfile>,
        /// The profile plus the secret references it stopped using.
        reply: Reply<(rd_core::ObjectStorageProfile, Vec<String>)>,
    },
    DeleteObjectStorageProfile {
        id: rd_core::ObjectStorageProfileId,
        /// Secret references orphaned by the deletion.
        reply: Reply<Vec<String>>,
    },
    /// Records a multipart upload about to start, replacing an older one for the same object.
    BeginObjectUpload {
        upload: Box<crate::ObjectUpload>,
        reply: Reply<()>,
    },
    RecordObjectUploadPart {
        id: String,
        part: crate::ObjectUploadPart,
        reply: Reply<()>,
    },
    CompleteObjectUpload {
        id: String,
        reply: Reply<()>,
    },
    /// Forgets one upload record by id, or every record of an owner.
    ForgetObjectUploads {
        id: Option<String>,
        owner: Option<String>,
        reply: Reply<u64>,
    },
}

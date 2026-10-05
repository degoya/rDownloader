//! The writer half of `object_storage_store` (RD-150-04).

use super::{Writer, publish_config, send};
use crate::commands::ObjectStorageCommand;

impl Writer {
    /// Applies the object storage profile and upload commands.
    pub(super) async fn handle_object_storage(&mut self, command: ObjectStorageCommand) {
        match command {
            ObjectStorageCommand::CreateObjectStorageProfile { input, reply } => {
                let result =
                    crate::object_storage_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            ObjectStorageCommand::UpdateObjectStorageProfile { id, input, reply } => {
                let result = crate::object_storage_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(profile, orphaned, event)| ((profile, orphaned), event));
                publish_config(reply, result, &self.events);
            }
            ObjectStorageCommand::DeleteObjectStorageProfile { id, reply } => {
                let result = crate::object_storage_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            // The upload records are bookkeeping of a running step, not configuration: nobody
            // re-reads anything when one changes, so they raise no event.
            ObjectStorageCommand::BeginObjectUpload { upload, reply } => {
                let result =
                    crate::object_storage_store::begin_upload(&mut self.connection, *upload).await;
                send(reply, result);
            }
            ObjectStorageCommand::RecordObjectUploadPart { id, part, reply } => {
                let result =
                    crate::object_storage_store::record_part(&mut self.connection, &id, part).await;
                send(reply, result);
            }
            ObjectStorageCommand::CompleteObjectUpload { id, reply } => {
                let result =
                    crate::object_storage_store::complete_upload(&mut self.connection, &id).await;
                send(reply, result);
            }
            ObjectStorageCommand::ForgetObjectUploads { id, owner, reply } => {
                let result = crate::object_storage_store::forget_uploads(
                    &mut self.connection,
                    id.as_deref(),
                    owner.as_deref(),
                )
                .await;
                send(reply, result);
            }
        }
    }
}

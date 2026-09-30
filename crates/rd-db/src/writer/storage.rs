//! The writer half of `collision_store` and `storage_ops_store` (RD-150-01, RD-150-02).

use super::{Writer, send};
use crate::{collision_store, commands::WriterCommand, storage_ops_store};

impl Writer {
    /// Applies the collision, content-index and storage-operation commands.
    pub(super) async fn handle_storage(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::SetCollisionPolicy {
                scope_kind,
                scope_id,
                policy,
                reply,
            } => {
                let result = collision_store::set_collision_policy(
                    &mut self.connection,
                    scope_kind,
                    &scope_id,
                    policy,
                )
                .await;
                send(reply, result);
            }
            WriterCommand::OpenCollisionPrompt { prompt, reply } => {
                let result =
                    collision_store::open_collision_prompt(&mut self.connection, prompt).await;
                send(reply, result);
            }
            WriterCommand::DecideCollisionPrompt {
                download_id,
                decision,
                reply,
            } => {
                let result = collision_store::decide_collision_prompt(
                    &mut self.connection,
                    download_id,
                    decision,
                )
                .await;
                send(reply, result);
            }
            WriterCommand::ClearCollisionPrompt { download_id, reply } => {
                let result =
                    collision_store::clear_collision_prompt(&mut self.connection, download_id)
                        .await;
                send(reply, result);
            }
            WriterCommand::IndexContent {
                download_id,
                algorithm,
                digest,
                size_bytes,
                path,
                reply,
            } => {
                let result = collision_store::index_content(
                    &mut self.connection,
                    download_id,
                    &algorithm,
                    &digest,
                    size_bytes,
                    &path,
                )
                .await;
                send(reply, result);
            }
            WriterCommand::MoveIndexedContent {
                download_id,
                path,
                reply,
            } => {
                let result =
                    collision_store::move_indexed_content(&mut self.connection, download_id, &path)
                        .await;
                send(reply, result);
            }
            WriterCommand::MarkIndexedContent { changes, reply } => {
                let result =
                    collision_store::mark_indexed_content(&mut self.connection, changes).await;
                send(reply, result);
            }
            WriterCommand::ForgetIndexedPath {
                path,
                except,
                reply,
            } => {
                let result =
                    collision_store::forget_indexed_path(&mut self.connection, &path, except).await;
                send(reply, result);
            }
            WriterCommand::ClearContentIndex { reply } => {
                let result = collision_store::clear_content_index(&mut self.connection).await;
                send(reply, result);
            }
            WriterCommand::StartStorageOperation { operation, reply } => {
                let result =
                    storage_ops_store::start_storage_operation(&mut self.connection, operation)
                        .await;
                send(reply, result);
            }
            WriterCommand::FinishStorageOperation { id, outcome, reply } => {
                let result =
                    storage_ops_store::finish_storage_operation(&mut self.connection, id, outcome)
                        .await;
                send(reply, result);
            }
            WriterCommand::InterruptStorageOperations { reply } => {
                let result =
                    storage_ops_store::interrupt_running_storage_operations(&mut self.connection)
                        .await;
                send(reply, result);
            }
            WriterCommand::ClearStorageOperations { reply } => {
                let result =
                    storage_ops_store::clear_storage_operations(&mut self.connection).await;
                send(reply, result);
            }
            // Routed here by `Writer::run` only for the variants above; see `handle_plugins`.
            _ => {}
        }
    }
}

//! The commands of `writer/plugins.rs`.

use rd_core::DownloadId;

use super::Reply;

/// The commands `Writer::handle_plugins` applies.
pub(crate) enum PluginsCommand {
    /// Writes the row that stands for one remote job, before the provider is asked for
    /// anything (RD-107-06). Refused when the account already has one for that content.
    ClaimRemoteJob {
        input: Box<crate::remote_job_store::ClaimRemoteJob>,
        reply: Reply<rd_core::RemoteJob>,
    },
    /// Records what one submit, poll or answer changed about a remote job (RD-107-06).
    AdvanceRemoteJob {
        id: rd_core::RemoteJobId,
        input: Box<crate::remote_job_store::AdvanceRemoteJob>,
        reply: Reply<rd_core::RemoteJob>,
    },
    /// Removes one remote job's row and nothing at the provider (RD-108-04).
    DeleteRemoteJob {
        id: rd_core::RemoteJobId,
        reply: Reply<bool>,
    },
    SavePluginTransfer {
        id: DownloadId,
        plugin_id: String,
        plugin_version: String,
        checkpoint: Option<Vec<u8>>,
        reply: Reply<crate::PluginTransfer>,
    },
    ClearPluginTransfer {
        id: DownloadId,
        reply: Reply<()>,
    },
    RecordPluginExecution {
        entry: Box<crate::NewPluginExecution>,
        reply: Reply<()>,
    },
    TrustPluginKey {
        input: crate::NewPluginTrustedKey,
        reply: Reply<crate::PluginTrustedKey>,
    },
    /// Withdraws one exact plugin package by its content digest.
    RevokePluginDigest {
        input: crate::NewPluginDigestRevocation,
        reply: Reply<crate::PluginDigestRevocation>,
    },
    /// Takes such a withdrawal back.
    UnrevokePluginDigest {
        digest: String,
        reply: Reply<bool>,
    },
    /// Replaces one plugin's version choice (RD-140-02).
    SavePluginVersionChoice {
        input: crate::NewPluginVersionChoice,
        reply: Reply<crate::PluginVersionChoice>,
    },
    RecordManagedTool {
        input: crate::NewManagedTool,
        reply: Reply<crate::ManagedToolRecord>,
    },
    ForgetManagedTool {
        name: String,
        version: String,
        reply: Reply<bool>,
    },
    AcceptToolManifest {
        sequence: i64,
        issued_at: String,
        reply: Reply<()>,
    },
    RevokePluginKey {
        key_id: String,
        reply: Reply<bool>,
    },
}

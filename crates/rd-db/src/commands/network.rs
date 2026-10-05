//! The commands of `writer/network.rs`.

use super::{ReleasedAccountRefs, Reply};
use crate::{
    network_store::{NewAccount, NewProxyProfile, UpdateAccount},
    usenet_store::{NewUsenetServer, UpdateUsenetServer, UsenetQuotaInput},
    usenet_traffic_store::UsenetQuotaReached,
};

/// The commands `Writer::handle_network` applies.
pub(crate) enum NetworkCommand {
    CreateAccount {
        input: NewAccount,
        reply: Reply<rd_core::Account>,
    },
    UpdateAccount {
        id: rd_core::AccountId,
        input: UpdateAccount,
        reply: Reply<rd_core::Account>,
    },
    DeleteAccount {
        id: rd_core::AccountId,
        /// The account's own references, and the ones its sign-in held.
        reply: Reply<ReleasedAccountRefs>,
    },
    CreateProxyProfile {
        input: NewProxyProfile,
        reply: Reply<rd_core::ProxyProfile>,
    },
    UpdateProxyProfile {
        id: rd_core::ProxyProfileId,
        input: NewProxyProfile,
        reply: Reply<rd_core::ProxyProfile>,
    },
    DeleteProxyProfile {
        id: rd_core::ProxyProfileId,
        reply: Reply<Option<String>>,
    },
    CreateUsenetServer {
        input: NewUsenetServer,
        reply: Reply<rd_core::UsenetServer>,
    },
    UpdateUsenetServer {
        id: rd_core::UsenetServerId,
        input: UpdateUsenetServer,
        reply: Reply<rd_core::UsenetServer>,
    },
    DeleteUsenetServer {
        id: rd_core::UsenetServerId,
        reply: Reply<Option<String>>,
    },
    CreateRemoteCredential {
        input: Box<crate::remote_store::NewRemoteCredential>,
        reply: Reply<rd_core::RemoteCredential>,
    },
    UpdateRemoteCredential {
        id: rd_core::RemoteCredentialId,
        input: Box<crate::remote_store::UpdateRemoteCredential>,
        /// The credential plus the secret references it stopped using.
        reply: Reply<(rd_core::RemoteCredential, Vec<String>)>,
    },
    DeleteRemoteCredential {
        id: rd_core::RemoteCredentialId,
        /// Secret references orphaned by the deletion.
        reply: Reply<Vec<String>>,
    },
    TrustSshHostKey {
        key: Box<rd_core::SshHostKey>,
        reply: Reply<()>,
    },
    ForgetSshHostKey {
        host: String,
        port: u16,
        algorithm: String,
        reply: Reply<()>,
    },
    SetCandidateListing {
        id: rd_core::CandidateId,
        listing: Box<rd_core::RemoteListing>,
        credential_id: Option<rd_core::RemoteCredentialId>,
        reply: Reply<()>,
    },
    SetCandidateListingPlan {
        id: rd_core::CandidateId,
        plan: rd_core::RemoteListingPlan,
        reply: Reply<rd_core::ResolvedRemoteListing>,
    },
    /// Sets, changes or removes one server's quota (RD-1100-05).
    SetUsenetQuota {
        id: rd_core::UsenetServerId,
        input: UsenetQuotaInput,
        reply: Reply<rd_core::UsenetServer>,
    },
    /// One flush of the bytes each server delivered (RD-1100-05).
    RecordUsenetTraffic {
        counts: Vec<(rd_core::UsenetServerId, u64)>,
        now: chrono::DateTime<chrono::Utc>,
        reply: Reply<Vec<UsenetQuotaReached>>,
    },
}

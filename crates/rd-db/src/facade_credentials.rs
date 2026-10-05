//! Database facade for authentication profiles, remote credentials and SSH host keys.

use anyhow::Result;

use crate::{
    Database,
    commands::{AuthCommand, NetworkCommand},
    writer,
};

impl Database {
    /// Lists every auth profile, including disabled and expired ones.
    pub async fn list_auth_profiles(&self) -> Result<Vec<rd_core::AuthProfile>> {
        crate::auth_profile_store::list(&self.readers).await
    }

    pub async fn auth_profile(
        &self,
        id: rd_core::AuthProfileId,
    ) -> Result<Option<rd_core::AuthProfile>> {
        crate::auth_profile_store::get(&self.readers, id).await
    }

    /// Most specific enabled, unexpired profile whose scope covers `url`.
    pub async fn match_auth_profile(&self, url: &url::Url) -> Result<Option<rd_core::AuthProfile>> {
        crate::auth_profile_store::match_for_url(&self.readers, url).await
    }

    /// Points one download at a profile, at none, or back at scope matching.
    pub async fn set_download_auth_profile(
        &self,
        id: rd_core::DownloadId,
        selection: rd_core::AuthProfileSelection,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| AuthCommand::SetDownloadAuthProfile {
            id,
            selection,
            reply,
        })
        .await
    }

    pub async fn create_auth_profile(
        &self,
        input: crate::auth_profile_store::NewAuthProfile,
    ) -> Result<rd_core::AuthProfile> {
        writer::request(&self.writer, |reply| AuthCommand::CreateAuthProfile {
            input,
            reply,
        })
        .await
    }

    /// Updates a profile and returns it together with the secret references that fell out
    /// of use, so the caller can remove them from the secret store.
    pub async fn update_auth_profile(
        &self,
        id: rd_core::AuthProfileId,
        input: crate::auth_profile_store::UpdateAuthProfile,
    ) -> Result<(rd_core::AuthProfile, Vec<String>)> {
        writer::request(&self.writer, |reply| AuthCommand::UpdateAuthProfile {
            id,
            input,
            reply,
        })
        .await
    }

    /// Enables or disables a profile; this is how a captured browser session is approved.
    pub async fn set_auth_profile_enabled(
        &self,
        id: rd_core::AuthProfileId,
        enabled: bool,
    ) -> Result<rd_core::AuthProfile> {
        writer::request(&self.writer, |reply| AuthCommand::SetAuthProfileEnabled {
            id,
            enabled,
            reply,
        })
        .await
    }

    /// Deletes a profile and returns the secret references it orphaned.
    pub async fn delete_auth_profile(&self, id: rd_core::AuthProfileId) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| AuthCommand::DeleteAuthProfile {
            id,
            reply,
        })
        .await
    }

    /// Lists every stored FTP/SFTP login, including disabled ones.
    pub async fn list_remote_credentials(&self) -> Result<Vec<rd_core::RemoteCredential>> {
        crate::remote_store::list(&self.readers).await
    }

    pub async fn remote_credential(
        &self,
        id: rd_core::RemoteCredentialId,
    ) -> Result<Option<rd_core::RemoteCredential>> {
        crate::remote_store::get(&self.readers, id).await
    }

    /// Most specific enabled login that can reach `target`.
    pub async fn match_remote_credential(
        &self,
        target: &rd_core::RemoteTarget,
    ) -> Result<Option<rd_core::RemoteCredential>> {
        crate::remote_store::match_for_target(&self.readers, target).await
    }

    pub async fn create_remote_credential(
        &self,
        input: crate::remote_store::NewRemoteCredential,
    ) -> Result<rd_core::RemoteCredential> {
        writer::request(&self.writer, |reply| {
            NetworkCommand::CreateRemoteCredential {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Updates a login and returns it together with the secret references that fell out of
    /// use, so the caller can remove them from the secret store.
    pub async fn update_remote_credential(
        &self,
        id: rd_core::RemoteCredentialId,
        input: crate::remote_store::UpdateRemoteCredential,
    ) -> Result<(rd_core::RemoteCredential, Vec<String>)> {
        writer::request(&self.writer, |reply| {
            NetworkCommand::UpdateRemoteCredential {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Deletes a login and returns the secret references it orphaned.
    pub async fn delete_remote_credential(
        &self,
        id: rd_core::RemoteCredentialId,
    ) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| {
            NetworkCommand::DeleteRemoteCredential { id, reply }
        })
        .await
    }

    /// What the trust store says about a server key that was just offered.
    pub async fn ssh_host_key_verdict(
        &self,
        host: &str,
        port: u16,
        algorithm: &str,
        fingerprint: &str,
    ) -> Result<crate::remote_store::HostKeyVerdict> {
        crate::remote_store::host_key_verdict(&self.readers, host, port, algorithm, fingerprint)
            .await
    }

    pub async fn list_ssh_host_keys(&self) -> Result<Vec<rd_core::SshHostKey>> {
        crate::remote_store::list_host_keys(&self.readers).await
    }

    /// Records a server key as trusted. Overwrites an existing entry, so the caller must
    /// have made a *changed* key an explicit decision before getting here.
    pub async fn trust_ssh_host_key(&self, key: rd_core::SshHostKey) -> Result<()> {
        writer::request(&self.writer, |reply| NetworkCommand::TrustSshHostKey {
            key: Box::new(key),
            reply,
        })
        .await
    }

    pub async fn forget_ssh_host_key(
        &self,
        host: String,
        port: u16,
        algorithm: String,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| NetworkCommand::ForgetSshHostKey {
            host,
            port,
            algorithm,
            reply,
        })
        .await
    }
}

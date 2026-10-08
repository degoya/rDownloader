//! The auth and OAuth providers the installed plugins offer, loaded once and swapped on an
//! install.

use super::*;

impl AuthFlowService {
    /// Hands over the providers the start built from its one plugin registry (RD-130-06).
    ///
    /// Without this the first `GET /api/v1/providers` loaded a registry of its own —
    /// re-verifying every installed package and compiling it — and the accounts page waited
    /// 25 s for a list the start had just had in hand. A set that is already there stays; the
    /// start hands them over once, before anything has asked, so a second set is a wiring
    /// mistake (API-06).
    pub fn preload(
        &self,
        providers: rd_plugin_ext::AuthProviders,
        oauth: rd_plugin_ext::OAuthProviders,
    ) {
        let providers_first = self
            .inner
            .providers
            .set(RwLock::new(Arc::new(providers)))
            .is_ok();
        let oauth_first = self.inner.oauth.set(RwLock::new(Arc::new(oauth))).is_ok();
        debug_assert!(
            providers_first && oauth_first,
            "the auth providers are preloaded once, before the first lookup"
        );
    }

    async fn auth_cell(&self) -> &RwLock<Arc<rd_plugin_ext::AuthProviders>> {
        self.inner
            .providers
            .get_or_init(|| async {
                RwLock::new(Arc::new(
                    match rd_plugin_ext::AuthProviders::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(providers) => providers,
                        Err(error) => {
                            tracing::warn!(%error, "could not load authentication plugins");
                            rd_plugin_ext::AuthProviders::none()
                        }
                    },
                ))
            })
            .await
    }

    async fn oauth_cell(&self) -> &RwLock<Arc<rd_plugin_ext::OAuthProviders>> {
        self.inner
            .oauth
            .get_or_init(|| async {
                RwLock::new(Arc::new(
                    match rd_plugin_ext::OAuthProviders::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(providers) => providers,
                        Err(error) => {
                            tracing::warn!(%error, "could not load oauth plugins");
                            rd_plugin_ext::OAuthProviders::none()
                        }
                    },
                ))
            })
            .await
    }

    /// The installed authentication providers, compiled on first use.
    pub async fn providers(&self) -> Arc<rd_plugin_ext::AuthProviders> {
        let cell = self.auth_cell().await;
        Arc::clone(&*cell.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// The installed OAuth providers, compiled on first use.
    pub async fn oauth_providers(&self) -> Arc<rd_plugin_ext::OAuthProviders> {
        let cell = self.oauth_cell().await;
        Arc::clone(&*cell.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Joins the sign-in plugin of a first install to the running sets (RD-170-12) and answers
    /// whether plugin `id` runs now.
    ///
    /// Only a plugin none of whose versions runs: an update waits for the next start, like
    /// every other. A set nobody has read yet loads from what is installed, the new plugin
    /// included, and needs nothing joined. Compiling happens on a blocking thread; a reader of
    /// the sets waits for nothing but the swap of one pointer.
    pub async fn activate_first_install(
        &self,
        registry: rd_plugin_host::PluginTypeRegistry,
        id: rd_core::PluginId,
    ) -> bool {
        let host = Some(Arc::clone(&self.inner.plugin_host));
        let built = tokio::task::spawn_blocking(move || {
            (
                rd_plugin_ext::AuthProviders::from_registry(&registry, host.clone()),
                rd_plugin_ext::OAuthProviders::from_registry(&registry, host),
            )
        })
        .await;
        let Ok((auth, oauth)) = built else {
            return false;
        };
        let auth_cell = self.auth_cell().await;
        let oauth_cell = self.oauth_cell().await;
        let mut running = false;
        {
            let mut current = auth_cell.write().unwrap_or_else(PoisonError::into_inner);
            if !auth.is_empty() && !current.has_plugin(id) {
                let joined = current.joined(auth);
                *current = Arc::new(joined);
            }
            running |= current.has_plugin(id);
        }
        {
            let mut current = oauth_cell.write().unwrap_or_else(PoisonError::into_inner);
            if !oauth.is_empty() && !current.has_plugin(id) {
                let joined = current.joined(oauth);
                *current = Arc::new(joined);
            }
            running |= current.has_plugin(id);
        }
        running
    }
}

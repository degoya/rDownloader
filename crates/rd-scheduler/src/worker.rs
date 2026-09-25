use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rd_core::{AuthMethod, DownloadFile, DownloadState, Failure, FailureKind, StorageRootId};
use rd_db::{Database, PersistedChunk};
use rd_files::StorageRoot;
use rd_http::{
    AuthMaterial, CheckpointSink, ChunkSpec, ClientContext, ClientKey, DownloadEngine,
    DownloadOutcome, DownloadRequest, HttpDownloadError, ProxyCredentials, StreamTransform,
    TransformCheckpoint, TransformPlan, import_into, plan_chunks, probe_with_headers,
};
use reqwest::cookie::Jar;
use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::{
    BlockReason, ProfileBoundary, SchedulerHandle, StopReason,
    failures::{
        not_a_file, record_error, record_http_error, record_http_error_with_replay, size_mismatch,
    },
    finish::{
        adopt_existing_final, current_destination, prepare_final_path, remove_if_empty,
        verify_part_and_promote,
    },
    profile_boundary::admitted,
};

pub(crate) async fn run(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    cancellation: CancellationToken,
) -> Result<()> {
    if file.state == DownloadState::RetryWait {
        scheduler
            .database
            .transition_download(file.id, DownloadState::Queued)
            .await?;
    }
    scheduler
        .database
        .transition_download(file.id, DownloadState::Resolving)
        .await?;
    let resolver_pin = scheduler.database.resolver_pin(file.id).await?;
    // A span of its own inside the download's trace (RD-110-03), so a slow or failing
    // resolver is visible as a step rather than as a gap. No address goes on it: the span
    // carries the download id, and the trace leads to the log records that name the rest.
    let resolve = tracing::Instrument::instrument(
        scheduler.resolvers.resolve(
            file.source.clone(),
            file.account_id,
            file.proxy_profile_id,
            resolver_pin.as_ref(),
        ),
        tracing::info_span!("download.resolve", download_id = %file.id),
    );
    let resolved = match tokio::select! {
        () = cancellation.cancelled() => return transition_stopped(scheduler, file).await,
        result = resolve => result,
    } {
        Ok(resolved) => resolved,
        Err(failure) => return record_error(scheduler, file, failure).await,
    };
    // A provider that encrypts on the client answers with an address *and* how its bytes
    // become a file (RD-103-02, ADR 0011). Asked only where the resolver chain said nothing,
    // so an ordinary link never pays for a world no installed plugin may even implement.
    let transformed = if resolved.is_some() || scheduler.transforms.is_empty() {
        None
    } else {
        // The fragment the intake put in the vault comes back here, and only here
        // (RD-110-38). The stored address lost it so that no row, event or log line could
        // ever carry a decryption key; the plugin needs it to derive one, so it is restored
        // onto the address a single call before it is handed over, and the restored `Url`
        // never leaves this block. A download with no reference, or a vault that cannot open
        // it, is asked with the address exactly as it is stored -- the plugin then refuses on
        // its own terms instead of being handed something half-rebuilt.
        let mut url = file.source.clone();
        match scheduler.database.download_secret_fragment(file.id).await {
            Ok(Some(fragment)) => url.set_fragment(Some(&fragment)),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                %error,
                download_id = %file.id,
                "the vaulted link fragment could not be read back"
            ),
        }
        let request = rd_plugin_api::ResolveRequest {
            url,
            client: rd_plugin_api::ClientIdentity {
                account_id: file.account_id,
                proxy_profile_id: file.proxy_profile_id,
                tls_revision: 0,
            },
        };
        let ask = tracing::Instrument::instrument(
            scheduler.transforms.resolve(&request),
            tracing::info_span!("download.transform", download_id = %file.id),
        );
        match tokio::select! {
            () = cancellation.cancelled() => return transition_stopped(scheduler, file).await,
            answer = ask => answer,
        } {
            None => None,
            Some(Ok(answer)) => Some(answer),
            Some(Err(failure)) => return record_error(scheduler, file, failure).await,
        }
    };
    // From here the address is handled exactly like a resolver's, which is the point: the
    // only thing the twelfth world adds is the description travelling beside it.
    let (resolved, transform) = match transformed {
        Some(answer) => (Some(answer.download), Some((answer.transform, answer.key))),
        None => (resolved, None),
    };

    let mut working_file = file.clone();
    // A plugin-resolved link points at a transfer URL; anything else is a plain direct link
    // that is downloaded exactly as it was added.
    let resolved_by_plugin = resolved.is_some();
    let (source, headers, resolved_size) = match resolved {
        Some(resolved) => {
            if let Some(name) = resolved.file_name {
                let name = rd_files::sanitize_file_name(&name);
                scheduler
                    .database
                    .set_download_file_name(file.id, name.clone())
                    .await?;
                // The package may still be named after the hoster, because intake had nothing
                // else: this address carries no path segment and no check result (RD-109-45).
                // It happens here, before the destination directory is created below, so the
                // folder is named rather than renamed. A failure costs a name, not a download.
                if let Err(error) = scheduler.adopt_resolved_package_name(file, &name).await {
                    tracing::warn!(
                        package_id = %file.package_id,
                        %error,
                        "the package kept its hoster name"
                    );
                }
                working_file.file_name = name;
            }
            let headers = resolved
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect::<Vec<_>>();
            (resolved.url, headers, resolved.size)
        }
        None => (file.source.clone(), Vec::new(), None),
    };
    let file = &working_file;
    let replay = crate::replay::load(scheduler, file).await?;
    let mut source = source;
    let mut headers = headers;

    // Only a resume risks continuing a partial file that was fetched from a URL which has
    // since expired. A fresh start has nothing to protect and pays nothing here.
    if file.committed_bytes.get() > 0 {
        match crate::replay::before_resume(scheduler, file, &source, replay.as_ref()).await? {
            crate::replay::Refreshed::Fresh => {}
            crate::replay::Refreshed::Replaced {
                url,
                headers: resolved,
            } => {
                source = url;
                headers = resolved;
            }
            crate::replay::Refreshed::Impossible(reason) => {
                return record_error(scheduler, file, crate::replay::blocked(reason)).await;
            }
        }
    }

    let network = tokio::select! {
        () = cancellation.cancelled() => return transition_stopped(scheduler, file).await,
        result = build_replay_client(scheduler, file, replay.as_ref()) => result?,
    };
    let NetworkClient {
        client,
        headers: profile_headers,
        profile_boundary,
        provider_credential,
    } = network;
    // What goes to every address: the resolver's or the capture's own headers. The profile's
    // and the account's are decided per address, because each is confined to a scope.
    let unauthenticated = headers.clone();
    // The profile was chosen for the link as it was added; `source` may be a resolver's answer
    // on another host, and gets the profile's headers only inside its scope (RD-120-43).
    headers.extend(admitted(
        &profile_headers,
        profile_boundary.as_ref(),
        &source,
    ));
    // Credential headers go in before the probe so the online check authenticates too.
    match provider_authorization(scheduler, provider_credential.as_ref(), &source).await? {
        Ok(Some(header)) => headers.push(header),
        Ok(None) => {}
        Err(failure) => return record_error(scheduler, file, failure).await,
    }

    let probe_result = match tokio::select! {
        () = cancellation.cancelled() => return transition_stopped(scheduler, file).await,
        result = probe_with_headers(&client, source.clone(), &headers) => result,
    } {
        Ok(result) => result,
        Err(error) => return record_http_error(scheduler, file, error).await,
    };
    // A resolver that hands back a landing page (expired direct link, hoster limit notice)
    // must not be written to disk as if it were the file.
    if resolved_by_plugin && !probe_result.looks_downloadable() {
        let failure = not_a_file(&client, &source, &headers, &probe_result).await;
        return record_error(scheduler, file, failure).await;
    }
    // The size the hoster announced for this file: what the resolver just read off the link
    // page, else what the online check recorded when the link was added. It is the only signal
    // that outlives a transfer which otherwise succeeds -- 1150 bytes fetched for a 405 MB
    // release completed, was checksummed, and showed a green tick (RD-109-36). Once the first
    // probe has run, `total_bytes` holds that probe's own number, so a later attempt compares a
    // value with itself and the rule cannot fire twice on the same evidence.
    let announced_size = resolved_size
        .or(file.total_bytes)
        .map(rd_core::ByteCount::get);
    if let (Some(announced), Some(offered)) = (announced_size, probe_result.total_bytes)
        && rd_http::contradicts_announced_size(announced, offered)
    {
        return record_error(scheduler, file, size_mismatch(announced, offered)).await;
    }
    let total_bytes = probe_result.total_bytes.or(announced_size);
    let transfer = scheduler.database.load_transfer(file.id).await?;
    let committed = transfer
        .chunks
        .iter()
        .any(|chunk| chunk.committed > chunk.start);
    // Named causes, not a bare `Blocked`: these two must survive a storage release untouched.
    // Restarting a transfer whose ETag moved writes a different file's bytes over confirmed
    // ones, which is the whole reason it is stopped here.
    if committed && validators_changed(&transfer, &probe_result) {
        scheduler
            .database
            .block_download(file.id, BlockReason::ValidatorsChanged.as_str())
            .await?;
        return Ok(());
    }
    if committed && !probe_result.accepts_ranges {
        scheduler
            .database
            .block_download(file.id, BlockReason::RangesRefused.as_str())
            .await?;
        return Ok(());
    }

    let chunks = if transfer.chunks.is_empty() || !committed {
        let planned = plan_chunks(
            total_bytes,
            probe_result.accepts_ranges,
            scheduler.chunk_budget(&probe_result.final_url),
        );
        let persisted = planned.iter().map(to_persisted).collect::<Vec<_>>();
        scheduler
            .database
            .prepare_transfer(
                file.id,
                total_bytes,
                probe_result.etag.clone(),
                probe_result.last_modified.clone(),
                persisted,
            )
            .await?;
        planned
    } else {
        transfer
            .chunks
            .iter()
            .map(|chunk| ChunkSpec {
                id: chunk.id,
                start: chunk.start,
                end: chunk.end,
                committed: chunk.committed,
            })
            .collect()
    };

    let packages = scheduler.database.list_packages().await?;
    let package = packages
        .into_iter()
        .find(|package| package.id == file.package_id)
        .context("download package not found")?;
    let root = StorageRoot::create(
        StorageRootId::new(),
        "download destination".to_owned(),
        PathBuf::from(&package.destination),
    )
    .await?;
    let staging = root.resolve(Path::new(".rdownloader"))?;
    tokio::fs::create_dir_all(&staging).await?;
    let part_path = staging.join(format!("{}.part", file.id));
    // A previous run may have got the file into its final place and stopped before recording
    // it (`scheduler.before_promote`). Adopting it is the difference between finishing and
    // fetching the whole thing again to file it next to itself as `name (1).ext`.
    if adopt_existing_final(
        scheduler,
        file,
        root.path(),
        &staging,
        &part_path,
        total_bytes,
    )
    .await?
    {
        return Ok(());
    }
    let final_path = prepare_final_path(scheduler, file, root.path()).await?;
    // The same policy every other runner passes through; an insufficient root is blocked
    // instead of letting the transfer fail on a write halfway through.
    let remaining = total_bytes.map(|total| total.saturating_sub(file.committed_bytes.get()));
    if !scheduler
        .ensure_capacity(root.path().to_string_lossy().as_ref(), remaining)
        .await?
    {
        scheduler
            .database
            .block_download(file.id, BlockReason::Capacity.as_str())
            .await?;
        return Ok(());
    }
    scheduler
        .database
        .transition_download(file.id, DownloadState::Downloading)
        .await?;

    // The description becomes something computable only here, where the key is checked
    // against the primitives this build implements and the previous attempt's chunk MACs are
    // read back. The key goes straight into the transform and is dropped with it: it is
    // never written to a row, never put in a header and never printed.
    let transform = match transform {
        None => None,
        Some((description, key)) => {
            // The key goes to the vault before the description is computable at all, and the
            // reference it comes back as goes into the description (RD-120-11). `rd-http`
            // refuses a description without one, so this is not a precaution but the step
            // that makes a transformed download possible; the reference is also part of the
            // fingerprint, which is why `adopt_transform_key` hands the *same* one back for
            // the same key rather than minting a fresh one per attempt.
            let description = match scheduler
                .database
                .adopt_transform_key(file.id, key.expose())
                .await
            {
                Ok(Some(reference)) => description.with_key_reference(reference),
                Ok(None) => {
                    return record_error(
                        scheduler,
                        file,
                        rd_core::Failure::coded(
                            rd_core::FailureKind::Permanent,
                            rd_core::CODE_KEY_MISSING,
                            "this installation has no vault to put a transform key in".to_owned(),
                        ),
                    )
                    .await;
                }
                Err(error) => {
                    return record_error(
                        scheduler,
                        file,
                        rd_core::Failure::coded(
                            rd_core::FailureKind::Transient {
                                retry_after_seconds: None,
                            },
                            rd_core::CODE_KEY_MISSING,
                            format!("the transform key could not be put away: {error}"),
                        ),
                    )
                    .await;
                }
            };
            let stream = match StreamTransform::new(description, &key) {
                Ok(stream) => stream,
                Err(failure) => return record_error(scheduler, file, failure).await,
            };
            let (fingerprint, macs) = scheduler.database.transform_checkpoint(file.id).await?;
            Some(TransformPlan {
                transform: Arc::new(stream),
                checkpoint: TransformCheckpoint { fingerprint, macs },
            })
        }
    };
    // Which description the MACs this run produces belong to, so a continuation can tell its
    // own state from somebody else's.
    let mac_stream = transform
        .as_ref()
        .map(|plan| (file.id, plan.transform.fingerprint().to_owned()));

    // The address the chunks are fetched from: the per-host budget and the memory of a
    // host that ignores ranges both belong to it, not to the link the user pasted.
    let transfer_url = probe_result.final_url.clone();
    // And so does the account's credential (RD-120-38). The probe followed the source's
    // redirects, and reqwest dropped `Authorization` at every change of host on the way; the
    // chunks are then fetched from where it ended, directly. Reusing the probe's headers there
    // would hand the credential to exactly the foreign host the redirect was stripped for. So
    // it is decided again, against the address the bytes actually come from. The profile's
    // headers the same way, against the profile's scope (RD-120-43): they had the same hole.
    let headers = if transfer_url == source {
        headers
    } else {
        let mut rebuilt = unauthenticated;
        rebuilt.extend(admitted(
            &profile_headers,
            profile_boundary.as_ref(),
            &transfer_url,
        ));
        match provider_authorization(scheduler, provider_credential.as_ref(), &transfer_url).await?
        {
            Ok(Some(header)) => rebuilt.push(header),
            Ok(None) => {}
            Err(failure) => return record_error(scheduler, file, failure).await,
        }
        rebuilt
    };
    let engine = DownloadEngine::new(client, scheduler.scoped_limiter(file).await)
        .with_host_limits(scheduler.host_limits().clone());
    let outcome = engine
        .download(
            DownloadRequest {
                url: probe_result.final_url,
                part_path: part_path.clone(),
                total_bytes,
                etag: probe_result.etag,
                last_modified: probe_result.last_modified,
                use_ranges: probe_result.accepts_ranges,
                chunks,
                headers,
                method: replay.as_ref().map(|r| r.method).unwrap_or_default(),
                body: replay.as_ref().and_then(|r| r.body.clone()),
                approved_origins: Arc::new(
                    replay
                        .as_ref()
                        .map(|r| r.approved_origins.clone())
                        .unwrap_or_default(),
                ),
                captured_user_agent: replay.as_ref().and_then(|r| r.captured_user_agent.clone()),
                // `None` for every ordinary download, which therefore runs exactly the
                // code it ran before the twelfth world existed (RD-110-33).
                transform,
            },
            Arc::new(DatabaseCheckpoint {
                database: scheduler.database.clone(),
                mac_stream,
            }),
            cancellation,
        )
        .await;

    match outcome {
        Ok(DownloadOutcome::Complete) => {
            scheduler
                .database
                .transition_download(file.id, DownloadState::Verifying)
                .await?;
            // The package category may have changed while downloading; finish into the
            // destination that is current now.
            let final_path = match current_destination(scheduler, file).await {
                Ok(Some(destination)) if destination != *root.path() => {
                    tokio::fs::create_dir_all(&destination).await?;
                    prepare_final_path(scheduler, file, &destination).await?
                }
                _ => final_path,
            };
            let result = verify_part_and_promote(scheduler, file, &part_path, &final_path).await;
            if result.is_ok() {
                remove_if_empty(&staging).await;
            }
            match result {
                Ok(()) => Ok(()),
                Err(error) => {
                    // The bytes arrived; putting them in place on this machine did not work
                    // — a checksum that did not match, a rename that was refused, a
                    // destination that filled up. Coded so the mirror group does not read a
                    // local obstacle as a reason to go and ask the next hoster.
                    let failure = match error.downcast::<Failure>() {
                        Ok(stated) => stated,
                        Err(local) => Failure::coded(
                            FailureKind::Permanent,
                            crate::mirrors::LOCAL_PROMOTE_CODE,
                            local.to_string(),
                        ),
                    };
                    record_error(scheduler, file, failure).await
                }
            }
        }
        Ok(DownloadOutcome::Paused) => transition_stopped(scheduler, file).await,
        Err(error) => {
            let is_post_replay = replay
                .as_ref()
                .is_some_and(crate::replay::ReplayContext::is_post);
            // Remembered for the retry: this host does not serve the parts it was asked
            // for, so the next attempt asks for the whole file in one connection instead
            // of repeating the same refusal four times.
            if matches!(error, HttpDownloadError::RangeIgnored) && !is_post_replay {
                scheduler.host_limits().note_ranges_ignored(&transfer_url);
            }
            record_http_error_with_replay(scheduler, file, error, is_post_replay).await
        }
    }
}

pub(crate) async fn transition_stopped(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
) -> Result<()> {
    let reason = scheduler
        .active
        .lock()
        .await
        .reasons
        .get(&file.id)
        .copied()
        .unwrap_or(StopReason::Paused);
    match reason {
        StopReason::Paused => {
            scheduler
                .database
                .transition_download(file.id, DownloadState::Paused)
                .await?;
        }
        StopReason::Cancelled => {
            scheduler
                .database
                .transition_download(file.id, DownloadState::Cancelled)
                .await?;
        }
        // The cause is written with the state: the release that undoes this stop has to be
        // able to pick out exactly the transfers it stopped and leave the rest alone.
        StopReason::Blocked(blocked) => {
            scheduler
                .database
                .block_download(file.id, blocked.as_str())
                .await?;
        }
    }
    Ok(())
}

/// A client together with the credential headers that belong on every request it makes.
///
/// `Basic` and `Bearer` ride along per request rather than as client defaults: that keeps
/// the pool from fragmenting, lets the online-check probe use the same credential, and
/// leaves reqwest free to strip the header on a cross-origin redirect.
pub struct NetworkClient {
    pub client: reqwest::Client,
    pub headers: Vec<(String, String)>,
    /// The scope `headers` are confined to, when a profile contributed any.
    ///
    /// `headers` fit the address the client was built for. A request to any other address —
    /// the end of a redirect, a resolver's answer — takes them only where this admits it
    /// (RD-120-43).
    pub profile_boundary: Option<ProfileBoundary>,
    /// The account's provider, user name and stored secret, when it has a secret.
    ///
    /// Carried out rather than turned into a header here, because whether that secret may be
    /// sent depends on the address the transfer ends up at — which is the resolver's answer,
    /// not the source this client was built for. See [`provider_authorization`].
    pub provider_credential: Option<ProviderCredential>,
}

/// The account credential a transfer may carry, by reference. Never the value: that is read
/// from the vault only once an address has been found that may receive it.
#[derive(Clone)]
pub struct ProviderCredential {
    /// The account's provider slug.
    pub provider: String,
    /// The account's user name, which HTTP Basic pairs with the secret (RD-120-38).
    pub username: Option<String>,
    /// The vault reference of the secret.
    pub reference: String,
}

/// Builds (or reuses) the isolated client for an account/proxy/profile combination.
/// Reached from outside through [`SchedulerHandle::network_client`].
pub(crate) async fn build_client(
    scheduler: &SchedulerHandle,
    account_id: Option<rd_core::AccountId>,
    proxy_profile_id: Option<rd_core::ProxyProfileId>,
    auth_profile: rd_core::AuthProfileSelection,
    scope: &url::Url,
) -> Result<NetworkClient> {
    let defaults = scheduler.network_defaults.read().await.clone();
    let config = scheduler
        .database
        .network_client_config(
            account_id,
            proxy_profile_id,
            defaults.global_proxy_profile_id,
            auth_profile,
            scope,
        )
        .await?;
    assemble_client(scheduler, config, defaults, scope, None).await
}

/// The transfer client for a download, confined to the replay's approved origins when it
/// has a consented template.
pub(crate) async fn build_replay_client(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    replay: Option<&crate::replay::ReplayContext>,
) -> Result<NetworkClient> {
    let defaults = scheduler.network_defaults.read().await.clone();
    let config = scheduler
        .database
        .network_client_config(
            file.account_id,
            file.proxy_profile_id,
            defaults.global_proxy_profile_id,
            file.auth_profile,
            &file.source,
        )
        .await?;
    let scope = replay
        .and_then(crate::replay::ReplayContext::scope)
        .map(Arc::new);
    assemble_client(scheduler, config, defaults, &file.source, scope).await
}

/// Builds a client for one specific profile without consulting the selection rules, so a
/// profile can be tested before it is approved or while it is switched off.
pub(crate) async fn build_test_client(
    scheduler: &SchedulerHandle,
    profile: rd_core::AuthProfile,
    scope: &url::Url,
) -> Result<NetworkClient> {
    let defaults = scheduler.network_defaults.read().await.clone();
    let mut config = scheduler
        .database
        .network_client_config(
            None,
            None,
            defaults.global_proxy_profile_id,
            rd_core::AuthProfileSelection::None,
            scope,
        )
        .await?;
    config.auth = Some(profile);
    assemble_client(scheduler, config, defaults, scope, None).await
}

/// Turns a resolved network configuration into a pooled client plus its credential headers.
async fn assemble_client(
    scheduler: &SchedulerHandle,
    config: rd_db::NetworkClientConfig,
    defaults: rd_http::NetworkDefaults,
    scope: &url::Url,
    replay_scope: Option<Arc<rd_http::ReplayScope>>,
) -> Result<NetworkClient> {
    // Read before `config` is taken apart below; the header itself is built later, and only
    // once the address the transfer actually goes to is known.
    let provider_credential = provider_transfer_credential(scheduler, &config).await?;
    let cookie_jar = Arc::new(Jar::default());
    if let Some(reference) = &config.cookie_ref {
        let content = scheduler.secrets.get(reference).await?;
        let cookie_scope = config
            .account_provider
            .as_deref()
            .and_then(rd_plugin_host::provider_cookie_scope)
            .unwrap_or_else(|| scope.clone());
        let cookie_scope = rd_http::CookieScope::provider(&cookie_scope)?;
        import_into(&cookie_jar, content.expose_secret(), &cookie_scope)?;
    }
    let mut headers = Vec::new();
    let mut profile_boundary = None;
    let mut auth_material = None;
    if let Some(profile) = &config.auth {
        let secret = match &profile.secret_ref {
            Some(reference) => Some(scheduler.secrets.get(reference).await?),
            None => None,
        };
        match profile.method {
            AuthMethod::Cookies => {
                if let Some(secret) = &secret {
                    let cookie_scope = rd_http::CookieScope::new(
                        &profile.scope.probe_url().unwrap_or_else(|| scope.clone()),
                        profile.scope.include_subdomains,
                    )?;
                    import_into(&cookie_jar, secret.expose_secret(), &cookie_scope)?;
                }
            }
            AuthMethod::Basic | AuthMethod::Bearer => {
                if let Some(secret) = &secret {
                    headers.push((
                        "authorization".to_owned(),
                        authorization_value(profile, secret)?,
                    ));
                    profile_boundary = Some(ProfileBoundary::new(profile.scope.clone(), scope));
                }
            }
        }
        if let Some(reference) = &profile.certificate_ref {
            auth_material = Some(AuthMaterial {
                identity_pem: scheduler.secrets.get(reference).await?,
            });
        }
    }
    // Only client-wide material may key the pool; a per-request header must not.
    let client_wide = config
        .auth
        .as_ref()
        .filter(|profile| {
            profile.certificate_ref.is_some() || profile.method == AuthMethod::Cookies
        })
        .map(|profile| (profile.id, profile.revision()));
    let proxy_profile_id = config.proxy.as_ref().map(|profile| profile.id);
    let proxy_credentials = match config.proxy.as_ref() {
        Some(profile) if profile.secret_ref.is_some() => {
            let reference = profile
                .secret_ref
                .as_deref()
                .context("proxy secret missing")?;
            let password = scheduler.secrets.get(reference).await?;
            let username = profile
                .username
                .clone()
                .context("proxy password requires a username")?;
            Some(ProxyCredentials { username, password })
        }
        _ => None,
    };
    let client = scheduler
        .clients
        .get_or_create(ClientContext {
            key: ClientKey {
                proxy_profile_id,
                account_id: config.account_id,
                cookie_ref: config.cookie_ref,
                auth_profile_id: client_wide.map(|(id, _)| id),
                auth_revision: client_wide.map_or(0, |(_, revision)| revision),
                replay_scope: replay_scope.as_ref().map(|scope| scope.key()),
                tls_revision: defaults.tls_revision,
            },
            proxy: config.proxy,
            proxy_credentials,
            cookie_jar,
            custom_ca_pem: defaults.custom_ca_pem,
            auth: auth_material,
            replay_scope,
        })
        .await?;
    Ok(NetworkClient {
        client,
        headers,
        profile_boundary,
        provider_credential,
    })
}

/// Which stored credential a transfer may carry for this account.
///
/// Almost always the account's own secret, which for an OAuth provider *is* the access token.
/// The exception is a provider whose person registered their own application (RD-106-03): there
/// the account's secret is the **client** secret, every renewal still needs it, and the access
/// token lives beside the sign-in flow. Handing the first to [`provider_authorization`] would
/// put the client secret in an `Authorization` header on every transfer — the wrong credential,
/// sent where the right one belongs.
///
/// Real-Debrid is the other provider of that shape and never noticed, because its download
/// addresses are generated and carry no bearer at all. Box is the first whose bytes come from
/// the API host itself (RD-120-05), which is where this became reachable.
///
/// An account whose sign-in has not produced a token yet gets `None` rather than a fallback:
/// the transfer then goes out unauthenticated and the provider says so, which is a legible
/// failure. The fallback would be the client secret, and that is not.
async fn provider_transfer_credential(
    scheduler: &SchedulerHandle,
    config: &rd_db::NetworkClientConfig,
) -> Result<Option<ProviderCredential>> {
    let Some(provider) = config.account_provider.clone() else {
        return Ok(None);
    };
    let username = config.account_username.clone();
    if !rd_plugin_host::provider_token_beside_the_flow(&provider) {
        return Ok(config
            .account_secret_ref
            .clone()
            .map(|reference| ProviderCredential {
                provider,
                username,
                reference,
            }));
    }
    let Some(account_id) = config.account_id else {
        return Ok(None);
    };
    let stored = scheduler
        .database
        .auth_flow(account_id)
        .await?
        .and_then(|flow| flow.access_ref);
    Ok(stored.map(|reference| ProviderCredential {
        provider,
        username,
        reference,
    }))
}

/// The `Authorization` header the account's own credential puts on a request to `target`.
///
/// Two shapes, one gate (`rd_plugin_host::provider_download_authorization`): an OAuth-signed
/// provider's access token as `Bearer` (RD-106-04), and `Basic` for a provider whose row
/// declares `transfer_auth = "basic"` (RD-120-38) — Seedr's file addresses and Pixeldrain's.
/// Either way only over TLS and only to an exact host the provider's own manifest listed under
/// `secret_domains`. `target` is the address the request goes to, never the one the download
/// started from: a resolver answering with somebody else's host, or a source redirecting to
/// one, must not take the credential there.
///
/// The secret is read from the vault only once the gate has said yes. The inner `Err` is an
/// account that cannot form a Basic pair — a provider that requires a user name and an account
/// without one — and becomes the download's failure; it names no part of the credential.
async fn provider_authorization(
    scheduler: &SchedulerHandle,
    credential: Option<&ProviderCredential>,
    target: &url::Url,
) -> Result<std::result::Result<Option<(String, String)>, Failure>> {
    let Some(credential) = credential else {
        return Ok(Ok(None));
    };
    // The gate first: a host outside `secret_domains` does not even open the vault.
    if !rd_plugin_host::provider_download_carries_credential(&credential.provider, target) {
        return Ok(Ok(None));
    }
    let secret = scheduler.secrets.get(&credential.reference).await?;
    Ok(rd_plugin_host::provider_download_authorization(
        &credential.provider,
        target,
        credential.username.as_deref(),
        secret.expose_secret(),
    )
    .map(|value| value.map(|value| ("authorization".to_owned(), value))))
}

/// Renders the `Authorization` value for a profile.
fn authorization_value(profile: &rd_core::AuthProfile, secret: &SecretString) -> Result<String> {
    match profile.method {
        AuthMethod::Bearer => Ok(format!("Bearer {}", secret.expose_secret())),
        AuthMethod::Basic => {
            let username = profile
                .username
                .as_deref()
                .context("basic auth profile has no username")?;
            let encoded =
                BASE64_STANDARD.encode(format!("{username}:{}", secret.expose_secret()).as_bytes());
            Ok(format!("Basic {encoded}"))
        }
        AuthMethod::Cookies => anyhow::bail!("cookie profiles carry no authorization header"),
    }
}

fn validators_changed(transfer: &rd_db::TransferMetadata, probe: &rd_http::ProbeResult) -> bool {
    transfer
        .total_bytes
        .zip(probe.total_bytes)
        .is_some_and(|(old, new)| old != new)
        || transfer
            .etag
            .as_ref()
            .zip(probe.etag.as_ref())
            .is_some_and(|(old, new)| old != new)
        || (transfer.etag.is_none()
            && transfer
                .last_modified
                .as_ref()
                .zip(probe.last_modified.as_ref())
                .is_some_and(|(old, new)| old != new))
}

fn to_persisted(chunk: &ChunkSpec) -> PersistedChunk {
    PersistedChunk {
        id: chunk.id,
        start: chunk.start,
        end: chunk.end,
        committed: chunk.committed,
    }
}

struct DatabaseCheckpoint {
    database: Database,
    /// Which download and which transform description the chunk MACs belong to. `None` for
    /// an ordinary transfer, which produces none.
    mac_stream: Option<(rd_core::DownloadId, String)>,
}

#[async_trait]
impl CheckpointSink for DatabaseCheckpoint {
    async fn commit(&self, chunk_id: rd_core::ChunkId, committed_offset: u64) -> Result<()> {
        self.database
            .checkpoint_chunk(chunk_id, committed_offset)
            .await
    }

    async fn commit_chunk_mac(&self, index: u64, mac: [u8; 16]) -> Result<()> {
        let Some((download_id, fingerprint)) = &self.mac_stream else {
            // A run with no transform has no MAC to record; a call here would be a bug in
            // the engine rather than something to write down.
            return Ok(());
        };
        self.database
            .checkpoint_chunk_mac(*download_id, fingerprint.clone(), index, mac)
            .await
    }
}

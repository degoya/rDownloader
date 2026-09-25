//! Seeing, answering and ending a job that runs at a provider (RD-108-04).
//!
//! RD-107-06 built the contract and RD-108-03 the flow, and neither touched a single REST
//! surface: `remote_job.changed` was emitted into a bus nobody listened on, a job in
//! `awaiting_choice` waited for an answer no endpoint could deliver, and `discard` was a
//! contract call nothing reached. This is that surface, and it is five endpoints.
//!
//! The one decision worth stating here is the split between the last two. Deleting at the
//! provider is irreversible and happens outside this machine, so `docs/adr/0003-*` keeps it
//! behind one explicit confirmed request and behind nothing else. Removing a row from this
//! installation's own list is a different act with a different endpoint, and it sends nothing
//! anywhere. A single "delete" that did both is exactly the mistake the separation prevents.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{AccountId, RemoteJob, RemoteJobId};
use rd_plugin_host::extension::RemoteJobSource;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    AppState,
    error::ApiError,
    remote_job_service::{ChoiceOutcome, DiscardOutcome, RemoteJobRefused, SubmitOutcome},
};

/// Longest address accepted for a remote job. A magnet is a few hundred characters; anything
/// past this is not one, and the check belongs here rather than in a plugin's fuel budget.
const MAX_SOURCE: usize = 8 * 1024;

/// Largest container a remote job may carry: 16 MiB.
///
/// Smaller than the 48 MiB an import takes, on purpose. The bytes are stored in the job's row
/// until the sweep submits them and are then copied into a plugin whose whole memory is 32 to
/// 64 MiB. What a provider takes is smaller still — every `remote-job` plugin shipped today
/// refuses a container over 4 or 8 MiB itself — so this is the host's own ceiling, the torrent
/// intake's 16 MiB, and not a promise that a provider accepts that much.
pub(crate) const MAX_REMOTE_JOB_CONTAINER_BYTES: usize = 16 * 1024 * 1024;

/// A source handed to one account's provider.
///
/// Exactly one of the three fields is given. They are separate fields rather than one tagged
/// value because they are different things and the provider treats them differently: a
/// magnet names content, a plain address names a place, a container is the file itself. A
/// request carrying more than one, or none, is refused under `remote_job.source_invalid`
/// rather than resolved by a rule the caller would have to know.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubmitRemoteJobRequest {
    /// The `magnet:` address to hand over.
    #[serde(default)]
    pub magnet: Option<String>,
    /// A plain `http(s)` address the provider fetches for itself (RD-120-20).
    #[serde(default)]
    pub address: Option<String>,
    /// A `.torrent` or `.nzb` file, base64 with the standard alphabet, at most 16 MiB once
    /// decoded (RD-120-31). The plugin reads the format from the bytes.
    #[serde(default)]
    pub container: Option<String>,
}

/// What handing a source over produced.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubmitRemoteJobResponse {
    pub job: RemoteJob,
    /// True when the account already had a job for this content. Nothing was written and
    /// nothing was sent — this is the duplicate guard answering, not a failure.
    pub already_running: bool,
}

/// The entries a person picked, by the provider's own identifiers.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RemoteJobChoiceRequest {
    /// Entry ids the job offered. Anything else is dropped rather than forwarded.
    pub entries: Vec<u32>,
}

/// The explicit confirmation a deletion at the provider needs.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct DiscardRemoteJobRequest {
    /// Has to be `true`. Absent or `false`, nothing is sent and the request is refused under
    /// `remote_job.not_confirmed` — so a client that forgot to ask cannot delete by omission.
    #[serde(default)]
    pub confirmed: bool,
}

/// The provider slugs an installed `remote-job` plugin runs jobs on.
///
/// The form that offers accounts needs this, and it needs it from the manifests rather than
/// from a list somebody maintains: a provider exists exactly as long as its plugin does
/// (`rd-provider-registry` fills itself from installed manifests alone), so a hard-coded list
/// would be wrong the moment a plugin is added or removed. It is the `claims` of every
/// installed, signature-checked `remote-job` manifest -- the keys `RemoteJobRunners` routes
/// by -- read without compiling a component (RD-120-51): answering from the runners made the
/// first open of the page after a start wait for every component to compile.
///
/// This does **not** replace `remote_job.no_plugin`. A plugin can be removed between the
/// moment the form is drawn and the moment somebody presses the button, and then the refusal
/// on submit is still the right answer -- it just becomes rare instead of routine
/// (RD-120-23).
#[utoipa::path(get, path = "/api/v1/remote-jobs/providers", tag = "configuration", responses((status = 200, body = Vec<String>)))]
pub async fn list_remote_job_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(
        state.remote_jobs.providers().await?.into_iter().collect(),
    ))
}

/// Every remote job this installation knows about, newest first.
#[utoipa::path(get, path = "/api/v1/remote-jobs", tag = "configuration", responses((status = 200, body = Vec<rd_core::RemoteJob>)))]
pub async fn list_remote_jobs(
    State(state): State<AppState>,
) -> Result<Json<Vec<RemoteJob>>, ApiError> {
    Ok(Json(state.remote_jobs.jobs().await?))
}

/// Hands a magnet, a plain address or a container file to one account's provider.
///
/// Answers with the row either way: a second paste of the same source is the job that is
/// already running, flagged rather than refused, because that is what the reader wants to see.
#[utoipa::path(post, path = "/api/v1/accounts/{id}/remote-jobs", tag = "configuration", params(("id" = rd_core::AccountId, Path)), request_body = SubmitRemoteJobRequest, responses((status = 200, body = SubmitRemoteJobResponse), (status = 400), (status = 404), (status = 413, description = "The container decodes to more than 16 MiB"), (status = 502)))]
pub async fn submit_remote_job(
    State(state): State<AppState>,
    Path(id): Path<AccountId>,
    Json(request): Json<SubmitRemoteJobRequest>,
) -> Result<Json<SubmitRemoteJobResponse>, ApiError> {
    let invalid = || {
        ApiError::bad_request(
            "remote_job.source_invalid",
            "a remote job needs exactly one source -- a magnet, an address or a container -- of a usable size",
        )
    };
    let source = match (
        request.magnet.as_deref().map(str::trim),
        request.address.as_deref().map(str::trim),
        request.container.as_deref(),
    ) {
        (Some(magnet), None, None) => RemoteJobSource::Magnet(magnet.to_owned()),
        (None, Some(address), None) => RemoteJobSource::Address(address.to_owned()),
        // The same decoder, and the same refusals, as a container handed to the LinkGrabber.
        (None, None, Some(container)) => RemoteJobSource::Container(
            crate::container_upload::decode_base64(container, MAX_REMOTE_JOB_CONTAINER_BYTES)?,
        ),
        _ => return Err(invalid()),
    };
    let usable = match &source {
        RemoteJobSource::Magnet(value) | RemoteJobSource::Address(value) => {
            !value.is_empty() && value.len() <= MAX_SOURCE
        }
        // Its ceiling was enforced while decoding.
        RemoteJobSource::Container(bytes) => !bytes.is_empty(),
    };
    if !usable {
        return Err(invalid());
    }
    let outcome = state.remote_jobs.submit(id, source).await?;
    match outcome {
        SubmitOutcome::Started(job) => Ok(Json(SubmitRemoteJobResponse {
            job,
            already_running: false,
        })),
        SubmitOutcome::AlreadyOurs(job) => Ok(Json(SubmitRemoteJobResponse {
            job,
            already_running: true,
        })),
        SubmitOutcome::NotClaimed => Err(ApiError::bad_request(
            "remote_job.not_claimed",
            "no installed plugin runs a job like this on this account's provider",
        )),
        SubmitOutcome::Refused(refusal) => Err(refused(refusal)),
    }
}

/// Answers the question a job in `awaiting_choice` asked.
#[utoipa::path(post, path = "/api/v1/remote-jobs/{id}/choice", tag = "configuration", params(("id" = rd_core::RemoteJobId, Path)), request_body = RemoteJobChoiceRequest, responses((status = 200, body = rd_core::RemoteJob), (status = 400), (status = 404), (status = 502)))]
pub async fn choose_remote_job_entries(
    State(state): State<AppState>,
    Path(id): Path<RemoteJobId>,
    Json(request): Json<RemoteJobChoiceRequest>,
) -> Result<Json<RemoteJob>, ApiError> {
    match state.remote_jobs.choose(id, &request.entries).await? {
        ChoiceOutcome::Chosen(job) => Ok(Json(*job)),
        ChoiceOutcome::Refused(refusal) => Err(refused(refusal)),
    }
}

/// Deletes the job **at the provider**, on an explicit confirmation.
///
/// The row is not removed: it stays in `discarded`, still naming the job the provider knew, so
/// what a confirmed request did remains readable afterwards.
#[utoipa::path(post, path = "/api/v1/remote-jobs/{id}/discard", tag = "configuration", params(("id" = rd_core::RemoteJobId, Path)), request_body = DiscardRemoteJobRequest, responses((status = 200, body = rd_core::RemoteJob), (status = 400), (status = 404), (status = 502)))]
pub async fn discard_remote_job(
    State(state): State<AppState>,
    Path(id): Path<RemoteJobId>,
    Json(request): Json<DiscardRemoteJobRequest>,
) -> Result<Json<RemoteJob>, ApiError> {
    match state.remote_jobs.discard(id, request.confirmed).await? {
        DiscardOutcome::Discarded(job) => Ok(Json(*job)),
        DiscardOutcome::Refused(refusal) => Err(refused(refusal)),
    }
}

/// Removes the row from this installation's list. Nothing is sent to the provider.
#[utoipa::path(delete, path = "/api/v1/remote-jobs/{id}", tag = "configuration", params(("id" = rd_core::RemoteJobId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn forget_remote_job(
    State(state): State<AppState>,
    Path(id): Path<RemoteJobId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    if !state.remote_jobs.forget(id).await? {
        return Err(ApiError::not_found(
            "remote_job.not_found",
            "there is no such remote job",
        ));
    }
    Ok(Json(crate::dto::MessageResponse::new(
        "remote_job.removed",
        "Removed from this list; nothing was deleted at the provider",
    )))
}

/// The status a refusal earns, decided by the code it carries.
///
/// Three groups, and the split is about who has to do something next. The row is gone or was
/// never there — 404. The request itself was wrong: unconfirmed, empty, answering a question
/// nobody asked, naming a job the provider never created — 400. Anything else came from the
/// provider through a plugin, under a code this crate cannot enumerate and must not rewrite,
/// so it keeps the code and travels as a bad gateway.
fn refused(refusal: RemoteJobRefused) -> ApiError {
    match refusal.code.as_str() {
        "remote_job.not_found" => ApiError::not_found("remote_job.not_found", refusal.message),
        "remote_job.no_account" => ApiError::not_found("remote_job.no_account", refusal.message),
        "remote_job.not_awaiting_choice" => {
            ApiError::bad_request("remote_job.not_awaiting_choice", refusal.message)
        }
        "remote_job.empty_choice" => {
            ApiError::bad_request("remote_job.empty_choice", refusal.message)
        }
        "remote_job.not_confirmed" => {
            ApiError::bad_request("remote_job.not_confirmed", refusal.message)
        }
        "remote_job.already_discarded" => {
            ApiError::bad_request("remote_job.already_discarded", refusal.message)
        }
        "remote_job.missing_remote_id" => {
            ApiError::bad_request("remote_job.missing_remote_id", refusal.message)
        }
        _ => ApiError::bad_gateway_owned(refusal.code, refusal.message),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::{DiscardRemoteJobRequest, refused};
    use crate::remote_job_service::RemoteJobRefused;

    fn status(code: &str) -> StatusCode {
        let error = refused(RemoteJobRefused {
            code: code.to_owned(),
            message: "whatever was said".to_owned(),
        });
        axum::response::IntoResponse::into_response(error).status()
    }

    /// The codes this crate owns keep their own status; a plugin's keeps its code and becomes
    /// a bad gateway. Reporting a provider's refusal as a client mistake sends the reader to
    /// check their own request, which is the one place the answer is not.
    #[test]
    fn a_refusal_is_reported_as_whoever_has_to_act_on_it() {
        assert_eq!(status("remote_job.not_found"), StatusCode::NOT_FOUND);
        assert_eq!(status("remote_job.no_account"), StatusCode::NOT_FOUND);
        assert_eq!(status("remote_job.not_confirmed"), StatusCode::BAD_REQUEST);
        assert_eq!(
            status("remote_job.already_discarded"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(status("remote_job.empty_choice"), StatusCode::BAD_REQUEST);
        assert_eq!(
            status("remote_job.not_awaiting_choice"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status("remote_job.missing_remote_id"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(status("realdebrid.quota_reached"), StatusCode::BAD_GATEWAY);
    }

    /// A body that says nothing must not read as a confirmation. This is the whole reason the
    /// field is required to be `true` rather than merely present: a client that forgot to ask
    /// deletes nothing at anybody's provider.
    #[test]
    fn a_discard_body_without_the_flag_is_not_a_confirmation() {
        let empty: DiscardRemoteJobRequest = serde_json::from_str("{}").expect("empty body");
        assert!(!empty.confirmed);
        let explicit: DiscardRemoteJobRequest =
            serde_json::from_str(r#"{"confirmed":false}"#).expect("explicit false");
        assert!(!explicit.confirmed);
        let confirmed: DiscardRemoteJobRequest =
            serde_json::from_str(r#"{"confirmed":true}"#).expect("confirmed");
        assert!(confirmed.confirmed);
    }
}

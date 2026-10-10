//! "Add all from LinkGrabber" from the capture agent's tray (RD-1240-07).
//!
//! What the web interface's `E` and `W` do, for an agent paired with queue control
//! (`capture:queue`, checked by `rd_api_core::auth::require_capture_queue`): every package with
//! a link checked online goes to the queue whole, started or paused, and every NZB import that
//! may be queued follows it. The web interface's route wants the package ids, which only reading
//! the LinkGrabber gives, and a capture token reaches no route of the API and must not read the
//! list; so the selection is made here, and the answer is counts and a code -- nothing in it
//! names a link, a package or a file, like the rest of the capture surface.
//!
//! One rule differs from the web interface: a package or an NZB import holding something that
//! was already added stays in the LinkGrabber. The web interface asks before it adds duplicates
//! again; a tray click has nobody to ask, so it adds the rest and says how many stayed.

use std::collections::HashMap;

use axum::{Json, extract::State};
use rd_core::{
    CategoryId, CollectorPackageId, DownloadPriority, LinkCandidateState, NzbImportId,
    NzbImportState,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState};

#[derive(Deserialize, ToSchema)]
pub struct CaptureLinkGrabberRequest {
    /// Create the downloads paused, as `W` does; absent or `false` starts them, as `E` does.
    #[serde(default)]
    pub paused: bool,
}

#[derive(Default, Serialize, ToSchema)]
pub struct CaptureLinkGrabberResponse {
    /// Links of the packages that went to the queue.
    pub links: u32,
    /// NZB imports that went to the queue.
    pub nzbs: u32,
    /// Packages and NZB imports left in the LinkGrabber because they hold something that was
    /// already added; the web interface asks about those.
    pub duplicates: u32,
    /// Packages and NZB imports that could not be queued.
    pub failed: u32,
    /// The stable code of the first failure, when one failed.
    pub first_error: Option<String>,
}

impl CaptureLinkGrabberResponse {
    fn fail(&mut self, error: &ApiError) {
        self.failed += 1;
        self.first_error
            .get_or_insert_with(|| error.code().to_owned());
    }
}

/// Moves everything the LinkGrabber holds into the queue, started or paused, as the web
/// interface's `E` and `W` do; what holds a duplicate stays behind.
#[utoipa::path(post, path = "/api/v1/capture/linkgrabber/enqueue", tag = "capture", request_body = CaptureLinkGrabberRequest, responses((status = 200, body = CaptureLinkGrabberResponse), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn enqueue_capture_linkgrabber(
    State(state): State<AppState>,
    Json(request): Json<CaptureLinkGrabberRequest>,
) -> Result<Json<CaptureLinkGrabberResponse>, ApiError> {
    let packages = state.database.list_collector_packages().await?;
    let candidates = state.database.list_candidates().await?;
    let imports = state.database.list_nzb_imports().await?;
    let selection = select(
        packages.iter().map(|package| package.id),
        candidates
            .iter()
            .filter_map(|candidate| Some((candidate.package_id?, candidate.state))),
        imports.iter().map(|import| ImportFacts {
            id: import.id,
            state: import.state,
            duplicate: import.duplicate,
            category: import.category_id,
        }),
    );
    let mut answer = CaptureLinkGrabberResponse {
        duplicates: selection.duplicates,
        ..CaptureLinkGrabberResponse::default()
    };
    for (id, links) in selection.packages {
        match crate::collector_enqueue::enqueue_package(&state, id, request.paused, None).await {
            Ok(_) => answer.links += links,
            Err(error) => {
                tracing::warn!(package_id = %id, code = error.code(), "the tray's enqueue of a package failed");
                answer.fail(&error);
            }
        }
    }
    for (id, category) in selection.nzbs {
        match enqueue_import(&state, id, category, request.paused).await {
            Ok(()) => answer.nzbs += 1,
            Err(error) => {
                tracing::warn!(import_id = %id, code = error.code(), "the tray's enqueue of an NZB import failed");
                answer.fail(&error);
            }
        }
    }
    Ok(Json(answer))
}

/// What `POST /api/v1/nzb/imports/{id}/enqueue` does for one import the selection allowed.
async fn enqueue_import(
    state: &AppState,
    id: NzbImportId,
    category: Option<CategoryId>,
    paused: bool,
) -> Result<(), ApiError> {
    let destination =
        crate::destination::intake_destination(&state.database, &state.scheduler, category).await?;
    state
        .database
        .enqueue_nzb_import(id, destination, DownloadPriority::Normal, paused)
        .await?;
    Ok(())
}

/// The part of an NZB import the selection reads.
pub(crate) struct ImportFacts {
    pub(crate) id: NzbImportId,
    pub(crate) state: NzbImportState,
    pub(crate) duplicate: bool,
    pub(crate) category: Option<CategoryId>,
}

/// What one press of the tray entry queues.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    /// The packages, in the LinkGrabber's order, each with the links it queues.
    pub(crate) packages: Vec<(CollectorPackageId, u32)>,
    /// The NZB imports, with the category each was imported into.
    pub(crate) nzbs: Vec<(NzbImportId, Option<CategoryId>)>,
    /// Packages and NZB imports left behind for holding a duplicate.
    pub(crate) duplicates: u32,
}

/// The web interface's choice for `E`: a package goes when one of its links is online, an NZB
/// import when it was imported and has not failed; minus what holds a duplicate.
pub(crate) fn select(
    packages: impl IntoIterator<Item = CollectorPackageId>,
    members: impl IntoIterator<Item = (CollectorPackageId, LinkCandidateState)>,
    imports: impl IntoIterator<Item = ImportFacts>,
) -> Selection {
    #[derive(Default)]
    struct Seen {
        online: bool,
        duplicate: bool,
        enqueueable: u32,
    }
    let mut seen: HashMap<CollectorPackageId, Seen> = HashMap::new();
    for (package, state) in members {
        let entry = seen.entry(package).or_default();
        entry.online |= state == LinkCandidateState::Online;
        entry.duplicate |= state == LinkCandidateState::Duplicate;
        entry.enqueueable += u32::from(state.is_enqueueable());
    }
    let mut selection = Selection::default();
    for package in packages {
        let Some(entry) = seen.get(&package) else {
            continue;
        };
        if entry.duplicate {
            selection.duplicates += 1;
        } else if entry.online {
            selection.packages.push((package, entry.enqueueable));
        }
    }
    for import in imports {
        if import.state != NzbImportState::Imported {
            continue;
        }
        if import.duplicate {
            selection.duplicates += 1;
        } else {
            selection.nzbs.push((import.id, import.category));
        }
    }
    selection
}

#[cfg(test)]
#[path = "capture_linkgrabber_tests.rs"]
mod tests;

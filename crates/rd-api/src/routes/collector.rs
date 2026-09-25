//! LinkGrabber intake, media selection, DLC and captcha routes.

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, captcha_handlers, collector_handlers, container_handlers, media_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/collector/candidates/{id}/media",
            get(media_handlers::get_candidate_media_formats),
        )
        .route(
            "/api/v1/collector/candidates/{id}/media/preview",
            post(media_handlers::preview_media_selection),
        )
        .route(
            "/api/v1/collector/candidates/{id}/media/selection",
            put(media_handlers::put_candidate_media_selection),
        )
        .route(
            "/api/v1/collector/candidates/{id}/media/output-preview",
            post(media_handlers::preview_media_output),
        )
        .route(
            "/api/v1/collector/candidates/{id}/auth-profile",
            put(media_handlers::put_candidate_auth_profile),
        )
        .route(
            "/api/v1/containers/import",
            post(container_handlers::import_container),
        )
        .route("/api/v1/dlc/import", post(container_handlers::import_dlc))
        .route(
            "/api/v1/collector/packages",
            get(collector_handlers::list_collector_packages),
        )
        .route(
            "/api/v1/collector/packages/bulk",
            post(collector_handlers::bulk_update_collector_packages),
        )
        .route(
            "/api/v1/collector/packages/reorder",
            post(collector_handlers::reorder_collector_packages),
        )
        .route(
            "/api/v1/collector/entries/reorder",
            post(collector_handlers::reorder_grabber_entries),
        )
        .route(
            "/api/v1/collector/packages/regroup",
            post(collector_handlers::regroup_collector_packages),
        )
        .route(
            "/api/v1/collector/packages/enqueue",
            post(collector_handlers::enqueue_collector_packages),
        )
        .route(
            "/api/v1/collector/packages/{id}",
            axum::routing::patch(collector_handlers::update_collector_package)
                .delete(collector_handlers::delete_collector_package),
        )
        .route(
            "/api/v1/collector/packages/{id}/enqueue",
            post(collector_handlers::enqueue_collector_package),
        )
        .route(
            "/api/v1/collector/candidates/move",
            post(collector_handlers::move_candidates),
        )
        .route(
            "/api/v1/collector/candidates/reorder",
            post(collector_handlers::reorder_candidates),
        )
        .route(
            "/api/v1/collector/candidates/check",
            post(collector_handlers::check_candidates),
        )
        .route(
            "/api/v1/collector/candidates/{id}/mirror",
            post(collector_handlers::pin_mirror).delete(collector_handlers::release_mirror),
        )
        .route(
            "/api/v1/collector/candidates/{id}/mirror/dissolve",
            post(collector_handlers::dissolve_mirror),
        )
        .route(
            "/api/v1/collector/mirror-preference",
            get(collector_handlers::get_mirror_preference)
                .put(collector_handlers::put_mirror_preference),
        )
        .route("/api/v1/captchas", get(captcha_handlers::list_captchas))
        .route(
            "/api/v1/captchas/{id}/solution",
            post(captcha_handlers::solve_captcha),
        )
        .route(
            "/api/v1/captchas/{id}/click",
            post(captcha_handlers::click_captcha),
        )
        .route(
            "/api/v1/captchas/{id}/skip",
            post(captcha_handlers::skip_captcha),
        )
        .route(
            "/api/v1/captcha-answerers",
            get(captcha_handlers::get_captcha_answerers),
        )
        .route(
            "/api/v1/captcha-config",
            get(captcha_handlers::get_captcha_config).put(captcha_handlers::update_captcha_config),
        )
        .route(
            "/api/v1/captcha-config/test",
            post(captcha_handlers::test_captcha_solver),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    container_handlers::import_container,
    container_handlers::import_dlc,
    media_handlers::get_candidate_media_formats,
    media_handlers::preview_media_selection,
    media_handlers::put_candidate_media_selection,
    media_handlers::preview_media_output,
    media_handlers::put_candidate_auth_profile,
    collector_handlers::capture_intake,
    collector_handlers::collector_intake,
    collector_handlers::list_collector_packages,
    collector_handlers::update_collector_package,
    collector_handlers::bulk_update_collector_packages,
    collector_handlers::reorder_collector_packages,
    collector_handlers::reorder_grabber_entries,
    collector_handlers::regroup_collector_packages,
    collector_handlers::delete_collector_package,
    collector_handlers::enqueue_collector_package,
    collector_handlers::enqueue_collector_packages,
    collector_handlers::move_candidates,
    collector_handlers::reorder_candidates,
    collector_handlers::check_candidates,
    collector_handlers::pin_mirror,
    collector_handlers::release_mirror,
    collector_handlers::dissolve_mirror,
    collector_handlers::get_mirror_preference,
    collector_handlers::put_mirror_preference,
    collector_handlers::rename_candidate,
    captcha_handlers::list_captchas,
    captcha_handlers::solve_captcha,
    captcha_handlers::click_captcha,
    captcha_handlers::skip_captcha,
    captcha_handlers::get_captcha_answerers,
    captcha_handlers::get_captcha_config,
    captcha_handlers::update_captcha_config,
    captcha_handlers::test_captcha_solver,
    // Registered in the capture router (see `crate::router`), documented here with the rest
    // of the captcha surface.
    captcha_handlers::list_capture_captchas,
    captcha_handlers::answer_capture_captcha,
    captcha_handlers::skip_capture_captcha,
    captcha_handlers::report_capture_captcha_without_widget,
))]
pub(crate) struct Doc;

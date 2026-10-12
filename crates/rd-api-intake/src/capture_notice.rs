//! Which hand-overs through the capture door the browser extension announces itself
//! (RD-1270-05).
//!
//! One hand-over, one notice. The extension shows its own for everything it hands over, so the
//! intake it causes is stored under `rd_db::intake_notice::announced_by_extension`, and the web
//! tab and the capture agent leave its `collector.intake` alone. Capture tokens carry no kind --
//! the extension and the agent pair the same way -- so the way in decides:
//!
//! - `capture/batches` by the source the body names: `browser_extension` (context menu, popup)
//!   and `browser_download` (an intercepted download) are the extension's alone; the agent
//!   names `clipboard` or `click_and_load`.
//! - `capture/click-and-load` always: it is the extension's route; the agent takes Click'n'Load
//!   on its own port and hands the links over through `capture/batches`.
//! - `capture/file` unless the agent sent it: the agent names itself in its `User-Agent`
//!   (`rd_core::CAPTURE_AGENT_PRODUCT`), and its file associations keep its own notice.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use rd_core::IngressSource;

use crate::{
    ApiError, AppState,
    dto::{CollectorIntakeRequest, CollectorIntakeResponse},
};

/// A LinkGrabber intake through the capture door, announced by the extension when its source
/// says it came from there.
pub(crate) async fn intake(
    state: State<AppState>,
    audit: crate::audit::AuditContext,
    request: CollectorIntakeRequest,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    let by_extension = from_extension(request.source);
    let intake = crate::collector_handlers::collector_intake(state, audit, Json(request));
    if by_extension {
        rd_db::intake_notice::announced_by_extension(intake).await
    } else {
        intake.await
    }
}

/// Whether a capture batch's source is one only the browser extension sends.
pub(crate) fn from_extension(source: IngressSource) -> bool {
    matches!(
        source,
        IngressSource::BrowserExtension | IngressSource::BrowserDownload
    )
}

/// Whether a request came from the capture agent, which names itself in its `User-Agent`.
pub(crate) fn from_agent(headers: &HeaderMap) -> bool {
    rd_api_core::capture_agents::reported_version(headers).is_some()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};
    use rd_core::IngressSource;

    use super::{from_agent, from_extension};

    #[test]
    fn only_the_extensions_own_sources_are_its_to_announce() {
        assert!(from_extension(IngressSource::BrowserExtension));
        assert!(from_extension(IngressSource::BrowserDownload));
        for other in [
            IngressSource::Clipboard,
            IngressSource::ClickAndLoad,
            IngressSource::Api,
        ] {
            assert!(!from_extension(other), "{other:?}");
        }
    }

    #[test]
    fn the_agent_is_known_by_its_user_agent() {
        let mut headers = HeaderMap::new();
        assert!(!from_agent(&headers), "no User-Agent is no agent");
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (X11; Linux x86_64) Firefox/131.0"),
        );
        assert!(!from_agent(&headers), "a browser is not the agent");
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("rdownloader-capture/1.27.0"),
        );
        assert!(from_agent(&headers));
    }
}

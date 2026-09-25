//! The one path from a subscription item into the LinkGrabber.
//!
//! Split out of `subscription_service.rs` for size (RD-110-37); the items are unchanged and
//! are re-exported from the parent module, so every path into them stayed the same.

use rd_db::Database;

use crate::{ApiError, link_check_service::LinkCheckService};

/// The handles the intake needs, so the poll loop and the review action share one path.
pub(crate) struct SubscriptionIntake<'a> {
    pub database: &'a Database,
    pub link_check: &'a LinkCheckService,
    pub media_settings: &'a rd_media::SharedMediaSettings,
    pub gallery_settings: &'a rd_gallery::SharedGallerySettings,
}

/// Hands subscription URLs to the ordinary LinkGrabber intake.
///
/// Deliberately the same path a pasted link takes: routing rules, categories, the online
/// check and the review list all apply, and a subscription gets no privileges a person does
/// not have. Both ways an item can be accepted come through here — the automatic poll, and a
/// person queueing a reviewed item — because an item that only changed state in the database
/// never reaches the LinkGrabber at all.
pub(crate) async fn hand_urls_to_intake(
    intake: &SubscriptionIntake<'_>,
    subscription_name: &str,
    links: Vec<crate::collector_handlers::DeclaredLink>,
    category_id: Option<rd_core::CategoryId>,
) -> Result<(), ApiError> {
    let media = intake.media_settings.read().await.clone();
    let gallery = intake.gallery_settings.read().await.clone();
    crate::collector_handlers::submit_plain_links_as(
        crate::collector_handlers::PlainIntake {
            database: intake.database,
            link_check: intake.link_check,
            media: &media,
            gallery: &gallery,
            source: rd_core::IngressSource::Subscription,
            source_label: Some(subscription_name.to_owned()),
            category_id,
        },
        links,
    )
    .await
    .map(|_| ())
}

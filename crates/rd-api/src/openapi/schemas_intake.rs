//! Component schemas of the captcha broker, sign-in and setup, the capture agent and the site
//! rules, apart from the shared ones in `schemas.rs` so neither file outgrows the 500-line rule
//! (RD-1280-08).

use utoipa::OpenApi;

use crate::{
    captcha_handlers, capture_agent_handlers, capture_linkgrabber, capture_queue, dto, error,
    site_rules_dto,
};

/// The captcha, sign-in, capture agent and site rule schemas.
#[derive(OpenApi)]
#[openapi(components(schemas(
    captcha_handlers::CaptchaConfigResponse,
    captcha_handlers::UpdateCaptchaConfigRequest,
    captcha_handlers::SolveCaptchaRequest,
    captcha_handlers::ClickCaptchaRequest,
    captcha_handlers::AnswerWidgetCaptchaRequest,
    captcha_handlers::CaptureCaptchaClient,
    captcha_handlers::TestCaptchaSolverRequest,
    captcha_handlers::CaptchaSolverTestResponse,
    rd_captcha::PendingCaptcha,
    rd_captcha::PendingWidget,
    rd_captcha::CaptchaAnswerers,
    rd_captcha::CaptchaKind,
    rd_captcha::SolverKind,
    dto::AuthStatus,
    dto::SetupStatus,
    dto::SetupRequest,
    dto::LoginRequest,
    dto::CapturePairRequest,
    dto::CapturePairResponse,
    capture_queue::CaptureQueuePauseRequest,
    capture_queue::CaptureQueueResponse,
    capture_linkgrabber::CaptureLinkGrabberRequest,
    capture_linkgrabber::CaptureLinkGrabberResponse,
    capture_agent_handlers::CaptureAgentSettingsResponse,
    capture_agent_handlers::CaptureAgentSettingsPatch,
    capture_agent_handlers::CaptureClipboardRequest,
    rd_core::CaptureAgentSettings,
    rd_core::CaptureShortcuts,
    rd_core::CaptureShortcutReport,
    rd_core::CaptureCommand,
    rd_core::CapturePlatform,
    rd_core::ShortcutsUnavailable,
    dto::ApiTokenScopesRequest,
    dto::ApiTokenLimitsRequest,
    dto::ScopeDescriptor,
    dto::MessageResponse,
    error::ErrorBody,
    site_rules_dto::SiteRuleCheckResponse,
    site_rules_dto::SiteRuleResponse,
    site_rules_dto::SiteRuleGroupResponse,
    site_rules_dto::SiteRulesResponse,
    site_rules_dto::SiteRuleSwitchRequest,
    site_rules_dto::SaveSiteRuleRequest,
    site_rules_dto::TestSiteRuleRequest,
    site_rules_dto::TestedLinkResponse,
    site_rules_dto::TestSiteRuleResponse,
    site_rules_dto::SiteRuleDocument,
    site_rules_dto::SiteRuleDocumentEntry,
    site_rules_dto::SiteRuleImportRequest,
    site_rules_dto::ImportedSiteRuleResponse,
    site_rules_dto::ImportSiteRulesResponse,
    site_rules_dto::SiteRuleImportPreviewResponse,
    site_rules_dto::SiteRulesClearRequest,
    site_rules_dto::SiteRulesClearResponse,
    site_rules_dto::SiteRuleExamplesResponse,
    site_rules_dto::SiteRuleOriginResponse,
)))]
pub(crate) struct IntakeSchemas;

//! Captcha challenges a resolver hands to the host, their answers and the solver trait.

use std::time::Duration;

use async_trait::async_trait;
use rd_plugin_types::Failure;
use serde::{Deserialize, Serialize};

use super::DEFAULT_CAPTCHA_ALLOWANCE;

/// A captcha the resolver cannot solve itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CaptchaChallenge {
    RecaptchaV2(WidgetChallenge),
    HCaptcha(WidgetChallenge),
    Turnstile(WidgetChallenge),
    Image(ImageChallenge),
    /// A picture answered by clicking one spot in it (RD-110-15).
    ClickPoint(ImageChallenge),
    /// A CutCaptcha widget, which only a solver service answers (RD-110-15).
    Cutcaptcha(CutcaptchaChallenge),
}

impl CaptchaChallenge {
    /// The hoster page a widget challenge is rendered on, or `None` for a picture.
    ///
    /// A widget token is only valid for the origin that produced it, so the page is what
    /// every consumer of a widget challenge needs: the solver service to reproduce it, the
    /// host to check it against the plugin's declared domains, and the browser extension to
    /// know which page a person is about to be shown.
    #[must_use]
    pub fn page_url(&self) -> Option<&str> {
        match self {
            Self::RecaptchaV2(widget) | Self::HCaptcha(widget) | Self::Turnstile(widget) => {
                Some(widget.page_url.as_str())
            }
            Self::Cutcaptcha(widget) => Some(widget.page_url.as_str()),
            Self::Image(_) | Self::ClickPoint(_) => None,
        }
    }

    /// Whether the answer is a coordinate rather than a token or typed text.
    #[must_use]
    pub const fn answers_with_point(&self) -> bool {
        matches!(self, Self::ClickPoint(_))
    }
}

/// Widget captcha, solvable from its site key and the page it is embedded in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WidgetChallenge {
    pub site_key: String,
    pub page_url: String,
    pub invisible: bool,
}

/// Classic image captcha as served by the hoster.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImageChallenge {
    pub mime: String,
    pub data: Vec<u8>,
    pub prompt: Option<String>,
}

/// CutCaptcha widget as its solver task needs it: the widget's own identifier and the page's
/// misery key, both read from the hoster page, plus the page itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CutcaptchaChallenge {
    pub site_key: String,
    pub misery_key: String,
    pub page_url: String,
}

/// Where a person clicked in a click-point captcha, in pixels of the image as served.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClickPoint {
    pub x: u32,
    pub y: u32,
}

/// The answer to a challenge, in the shape the challenge has: a widget token or the typed
/// text of an image captcha, or the spot clicked in a click-point captcha.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CaptchaAnswer {
    Token(String),
    Point(ClickPoint),
}

impl CaptchaAnswer {
    /// The token, or `None` for a point.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Token(token) => Some(token),
            Self::Point(_) => None,
        }
    }
}

/// Application-side captcha solving, injected into the resolver host.
///
/// Kept as a trait so the host depends on the capability rather than on the solver
/// implementation, and so tests can answer challenges without a service or a UI.
#[async_trait]
pub trait CaptchaSolver: Send + Sync {
    /// Waiting time one challenge may need, given the current configuration.
    async fn allowance(&self) -> Duration {
        DEFAULT_CAPTCHA_ALLOWANCE
    }

    /// Answers a challenge within `limit`, which the host has reserved for it, in the shape
    /// the challenge has.
    async fn solve(
        &self,
        challenge: CaptchaChallenge,
        limit: Duration,
    ) -> Result<CaptchaAnswer, Failure>;
}

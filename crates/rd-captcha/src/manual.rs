//! Queue of captchas waiting for a person to solve them.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rd_core::CaptchaId;
use rd_plugin_api::{CaptchaAnswer, CaptchaChallenge};
use serde::Serialize;
use tokio::sync::oneshot;
use utoipa::ToSchema;

/// Which kind of challenge is waiting, for the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptchaKind {
    RecaptchaV2,
    HCaptcha,
    Turnstile,
    Image,
    /// A picture answered by clicking one spot in it (RD-110-15).
    ClickPoint,
    /// Listed for completeness of the contract: a CutCaptcha is answered by a solver service
    /// alone and never enters this queue (RD-110-15).
    Cutcaptcha,
}

impl CaptchaKind {
    /// A widget a browser can open the hoster's page for and read the token out of.
    #[must_use]
    pub(crate) const fn is_browser_widget(self) -> bool {
        matches!(self, Self::RecaptchaV2 | Self::HCaptcha | Self::Turnstile)
    }

    /// The answer shape this kind takes from a person: text, a point, or nothing typed at all.
    const fn accepts(self, answer: &CaptchaAnswer, source: AnswerSource) -> Option<SubmitOutcome> {
        match (self, answer) {
            (Self::Image, CaptchaAnswer::Token(_))
            | (Self::ClickPoint, CaptchaAnswer::Point(_)) => None,
            (Self::Image | Self::ClickPoint | Self::Cutcaptcha, _) => {
                Some(SubmitOutcome::WrongAnswerShape)
            }
            (_, CaptchaAnswer::Point(_)) => Some(SubmitOutcome::WrongAnswerShape),
            (_, CaptchaAnswer::Token(_)) => match source {
                AnswerSource::Browser => None,
                AnswerSource::Typed => Some(SubmitOutcome::WidgetNeedsSolver),
            },
        }
    }
}

/// A captcha the user is being asked to solve.
///
/// Image and click-point challenges carry their picture as a `data:` URI so the UI can show
/// it without a second request; widget challenges carry only their site key and page, which
/// is all a solver service needs and all the UI can meaningfully display.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PendingCaptcha {
    pub id: CaptchaId,
    pub kind: CaptchaKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_key: Option<String>,
    /// `data:<mime>;base64,…` for image and click-point challenges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Outcome of handing a challenge to the user.
pub(crate) enum ManualOutcome {
    Solved(CaptchaAnswer),
    Skipped,
    /// The browser opened the hoster's page and found no widget on it (RD-120-45).
    PageWithoutWidget,
    TimedOut,
}

/// What travels from whoever answered a challenge to the resolver waiting on it.
#[derive(Debug, PartialEq)]
pub(crate) enum Reply {
    Answer(CaptchaAnswer),
    /// A person declined it.
    Declined,
    /// The browser extension opened the hoster's page for a widget and the page showed none.
    ///
    /// The case RD-120-45 was reported for: the service fetches the sign-in page with an empty
    /// cookie jar and meets a Turnstile, the person's browser is signed in already and is
    /// redirected straight past the form. Nobody can answer a widget that is not there, so
    /// the wait used to run until its timeout with nothing on screen saying why.
    PageWithoutWidget,
}

/// What happened to a submitted answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitOutcome {
    /// The waiting resolver received the answer.
    Delivered,
    /// No captcha with that id is waiting any more.
    NotWaiting,
    /// A widget captcha is bound to the hoster's domain, so a *typed* answer cannot solve
    /// it — nothing a person could type here would be accepted. The challenge keeps
    /// waiting, because a browser token or a solver service is still a way out.
    WidgetNeedsSolver,
    /// The answer has the wrong shape for the challenge: text for a click-point captcha, a
    /// point for anything else. The challenge keeps waiting for the right one.
    WrongAnswerShape,
}

/// Where an answer came from, and therefore whether it can be trusted for a widget.
///
/// The measurement in RD-107-03 is what this encodes: a widget token is only produced by the
/// hoster's own page, so the source decides whether an answer is even plausible. Typing stays
/// refused for widgets; a token harvested by the desktop agent's WebView on that very page is
/// accepted, because it is the only thing that can have produced one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnswerSource {
    /// A person typed it into the web interface.
    Typed,
    /// The desktop agent read it out of the hoster's own page in a WebView.
    Browser,
}

/// A widget challenge as the desktop agent needs to see it.
///
/// Deliberately narrower than [`PendingCaptcha`]: a capture token is a restricted credential,
/// and everything an agent needs to open the right page is here — nothing else is. There is
/// no image, no prompt and, above all, no answer.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PendingWidget {
    pub id: CaptchaId,
    pub kind: CaptchaKind,
    /// The hoster page the WebView must load; the token is only valid for this origin.
    pub page_url: String,
    pub site_key: String,
    pub expires_at: DateTime<Utc>,
}

struct Waiting {
    description: PendingCaptcha,
    sender: oneshot::Sender<Reply>,
}

/// Registry of challenges awaiting an answer. Purely in-memory: a restart drops them, and
/// the resolvers waiting on them fail with the download they belong to.
#[derive(Clone, Default)]
pub(crate) struct ManualQueue {
    waiting: Arc<Mutex<HashMap<CaptchaId, Waiting>>>,
}

impl ManualQueue {
    /// Registers a challenge and returns its description plus the receiver to await.
    pub(crate) fn enqueue(
        &self,
        challenge: &CaptchaChallenge,
        timeout: std::time::Duration,
    ) -> (PendingCaptcha, oneshot::Receiver<Reply>) {
        let id = CaptchaId::new();
        let created_at = Utc::now();
        let expires_at =
            created_at + chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::zero());
        let description = describe(id, challenge, created_at, expires_at);
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.insert(
                id,
                Waiting {
                    description: description.clone(),
                    sender,
                },
            );
        }
        (description, receiver)
    }

    /// Every challenge still waiting, oldest first.
    ///
    /// Expired entries are dropped on the way out: their sender goes with them, which wakes
    /// the resolver still awaiting an answer. Every announcement therefore sweeps the queue,
    /// so a challenge nobody answered cannot be offered past its deadline.
    pub(crate) fn pending(&self) -> Vec<PendingCaptcha> {
        let Ok(mut waiting) = self.waiting.lock() else {
            return Vec::new();
        };
        let now = Utc::now();
        waiting.retain(|_, entry| entry.description.expires_at > now);
        let mut pending: Vec<PendingCaptcha> = waiting
            .values()
            .map(|entry| entry.description.clone())
            .collect();
        pending.sort_by_key(|entry| entry.created_at);
        pending
    }

    /// The widget challenges still waiting, oldest first, narrowed for the desktop agent.
    ///
    /// Sweeps the queue exactly as [`Self::pending`] does, so an expired challenge is never
    /// handed to a browser that would then open a page nobody can answer in time. Only the
    /// widgets a browser can read a token out of are listed: a CutCaptcha never enters the
    /// queue, and would not be offered even if it did.
    pub(crate) fn pending_widgets(&self) -> Vec<PendingWidget> {
        self.pending()
            .into_iter()
            .filter(|entry| entry.kind.is_browser_widget())
            .filter_map(|entry| {
                Some(PendingWidget {
                    id: entry.id,
                    kind: entry.kind,
                    page_url: entry.page_url?,
                    site_key: entry.site_key?,
                    expires_at: entry.expires_at,
                })
            })
            .collect()
    }

    /// Answers a challenge; `None` skips it.
    ///
    /// The answer has to fit the challenge: text answers an image, a point answers a
    /// click-point captcha, and either one is refused for the other. A **typed** token is
    /// refused for widget challenges: the token comes out of the hoster's own page and
    /// nothing a person could type is accepted there. Skipping stays possible, since that is
    /// how the user declines it, and a token a browser harvested from that page carries
    /// [`AnswerSource::Browser`] and is taken. A refused answer leaves the challenge waiting.
    pub(crate) fn resolve(
        &self,
        id: CaptchaId,
        answer: Option<CaptchaAnswer>,
        source: AnswerSource,
    ) -> SubmitOutcome {
        let Ok(mut waiting) = self.waiting.lock() else {
            return SubmitOutcome::NotWaiting;
        };
        let Some(entry) = waiting.get(&id) else {
            return SubmitOutcome::NotWaiting;
        };
        if let Some(answer) = &answer
            && let Some(refusal) = entry.description.kind.accepts(answer, source)
        {
            return refusal;
        }
        let Some(entry) = waiting.remove(&id) else {
            return SubmitOutcome::NotWaiting;
        };
        // A closed receiver means the download gave up first; the answer is simply dropped.
        let _ = entry
            .sender
            .send(answer.map_or(Reply::Declined, Reply::Answer));
        SubmitOutcome::Delivered
    }

    /// Ends a widget challenge because the browser found no widget on the hoster's page.
    ///
    /// Only a browser widget can be reported this way: it is the only kind a browser opens a
    /// page for, and an image captcha "without a widget" is a statement about nothing.
    pub(crate) fn report_page_without_widget(&self, id: CaptchaId) -> SubmitOutcome {
        let Ok(mut waiting) = self.waiting.lock() else {
            return SubmitOutcome::NotWaiting;
        };
        let Some(entry) = waiting.get(&id) else {
            return SubmitOutcome::NotWaiting;
        };
        if !entry.description.kind.is_browser_widget() {
            return SubmitOutcome::WrongAnswerShape;
        }
        let Some(entry) = waiting.remove(&id) else {
            return SubmitOutcome::NotWaiting;
        };
        let _ = entry.sender.send(Reply::PageWithoutWidget);
        SubmitOutcome::Delivered
    }

    /// Drops a challenge nobody answered, so it stops being offered. Reports whether it was
    /// still queued, so a caller only announces a change that actually happened.
    pub(crate) fn forget(&self, id: CaptchaId) -> bool {
        self.waiting
            .lock()
            .is_ok_and(|mut waiting| waiting.remove(&id).is_some())
    }
}

fn describe(
    id: CaptchaId,
    challenge: &CaptchaChallenge,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> PendingCaptcha {
    let base = PendingCaptcha {
        id,
        kind: CaptchaKind::Image,
        host: None,
        page_url: None,
        site_key: None,
        image: None,
        prompt: None,
        created_at,
        expires_at,
    };
    match challenge {
        CaptchaChallenge::RecaptchaV2(widget) => PendingCaptcha {
            kind: CaptchaKind::RecaptchaV2,
            ..widget_fields(base, widget)
        },
        CaptchaChallenge::HCaptcha(widget) => PendingCaptcha {
            kind: CaptchaKind::HCaptcha,
            ..widget_fields(base, widget)
        },
        CaptchaChallenge::Turnstile(widget) => PendingCaptcha {
            kind: CaptchaKind::Turnstile,
            ..widget_fields(base, widget)
        },
        CaptchaChallenge::Cutcaptcha(widget) => PendingCaptcha {
            kind: CaptchaKind::Cutcaptcha,
            host: host_of(&widget.page_url),
            page_url: Some(widget.page_url.clone()),
            site_key: Some(widget.site_key.clone()),
            ..base
        },
        CaptchaChallenge::Image(image) => PendingCaptcha {
            kind: CaptchaKind::Image,
            ..picture_fields(base, image)
        },
        CaptchaChallenge::ClickPoint(image) => PendingCaptcha {
            kind: CaptchaKind::ClickPoint,
            ..picture_fields(base, image)
        },
    }
}

fn picture_fields(base: PendingCaptcha, image: &rd_plugin_api::ImageChallenge) -> PendingCaptcha {
    PendingCaptcha {
        image: Some(format!(
            "data:{};base64,{}",
            image_mime(&image.mime),
            STANDARD.encode(&image.data)
        )),
        prompt: image.prompt.clone(),
        ..base
    }
}

fn host_of(page_url: &str) -> Option<String> {
    url::Url::parse(page_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
}

fn widget_fields(base: PendingCaptcha, widget: &rd_plugin_api::WidgetChallenge) -> PendingCaptcha {
    PendingCaptcha {
        host: host_of(&widget.page_url),
        page_url: Some(widget.page_url.clone()),
        site_key: Some(widget.site_key.clone()),
        ..base
    }
}

/// Keeps the `data:` URI honest: an unknown or malformed type from a hoster must not end up
/// as an arbitrary string inside the URI the browser parses.
fn image_mime(reported: &str) -> &str {
    match reported.trim().to_ascii_lowercase().as_str() {
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

#[cfg(test)]
#[path = "manual_tests.rs"]
mod tests;

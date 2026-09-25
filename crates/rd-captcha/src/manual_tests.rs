//! Tests of the manual queue: what a person may answer, and how.

use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, ClickPoint, CutcaptchaChallenge, ImageChallenge,
    WidgetChallenge,
};

use super::{AnswerSource, CaptchaKind, ManualQueue, Reply, SubmitOutcome, image_mime};

/// The spot a person clicked, as the broker delivers it.
fn resolve_point(queue: &ManualQueue, id: rd_core::CaptchaId, point: ClickPoint) -> SubmitOutcome {
    queue.resolve(id, Some(CaptchaAnswer::Point(point)), AnswerSource::Typed)
}

fn click_point() -> CaptchaChallenge {
    CaptchaChallenge::ClickPoint(ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"BM".to_vec(),
        prompt: Some("Click the circle".to_owned()),
    })
}

fn cutcaptcha() -> CaptchaChallenge {
    CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge {
        site_key: "SAs61IAI".to_owned(),
        misery_key: "a1488b66da00bf332a1488993a5443c79047e752".to_owned(),
        page_url: "https://filecrypt.cc/Container/ABC.html".to_owned(),
    })
}

fn image() -> CaptchaChallenge {
    CaptchaChallenge::Image(ImageChallenge {
        mime: "image/png".to_owned(),
        data: b"BM".to_vec(),
        prompt: Some("Type the code".to_owned()),
    })
}

fn widget() -> CaptchaChallenge {
    CaptchaChallenge::Turnstile(WidgetChallenge {
        site_key: "0x4AAA".to_owned(),
        page_url: "https://katfile.biz/abc/file.rar".to_owned(),
        invisible: false,
    })
}

#[tokio::test]
async fn a_submitted_answer_reaches_the_waiting_resolver() {
    let queue = ManualQueue::default();
    let (pending, receiver) = queue.enqueue(&image(), std::time::Duration::from_secs(60));

    assert_eq!(pending.kind, CaptchaKind::Image);
    assert_eq!(pending.image.as_deref(), Some("data:image/png;base64,Qk0="));
    assert_eq!(queue.pending().len(), 1);

    assert_eq!(
        queue.resolve(
            pending.id,
            Some(CaptchaAnswer::Token("42".to_owned())),
            AnswerSource::Typed
        ),
        SubmitOutcome::Delivered
    );
    assert_eq!(
        receiver.await.expect("sender kept"),
        Reply::Answer(CaptchaAnswer::Token("42".to_owned()))
    );
    assert!(
        queue.pending().is_empty(),
        "an answered captcha stops being offered"
    );
    assert_eq!(
        queue.resolve(pending.id, None, AnswerSource::Typed),
        SubmitOutcome::NotWaiting,
        "answering twice must not succeed"
    );
}

#[tokio::test]
async fn skipping_reports_no_solution() {
    let queue = ManualQueue::default();
    let (pending, receiver) = queue.enqueue(&image(), std::time::Duration::from_secs(60));

    assert_eq!(
        queue.resolve(pending.id, None, AnswerSource::Typed),
        SubmitOutcome::Delivered
    );

    assert_eq!(receiver.await.expect("sender kept"), Reply::Declined);
}

#[test]
fn a_widget_challenge_reports_its_site_key_and_host() {
    let queue = ManualQueue::default();

    let (pending, _receiver) = queue.enqueue(&widget(), std::time::Duration::from_secs(60));

    assert_eq!(pending.kind, CaptchaKind::Turnstile);
    assert_eq!(pending.site_key.as_deref(), Some("0x4AAA"));
    assert_eq!(pending.host.as_deref(), Some("katfile.biz"));
    assert!(pending.image.is_none());
}

/// A widget captcha is rendered on the hoster's domain, so nothing the user could type
/// here would be accepted. The challenge must survive the attempt: a solver configured
/// afterwards is still a way out.
#[test]
fn a_typed_answer_is_refused_for_a_widget_but_skipping_is_not() {
    let queue = ManualQueue::default();
    let (pending, _receiver) = queue.enqueue(&widget(), std::time::Duration::from_secs(60));

    assert_eq!(
        queue.resolve(
            pending.id,
            Some(CaptchaAnswer::Token("guess".to_owned())),
            AnswerSource::Typed
        ),
        SubmitOutcome::WidgetNeedsSolver
    );
    assert_eq!(queue.pending().len(), 1, "the challenge keeps waiting");

    assert_eq!(
        queue.resolve(pending.id, None, AnswerSource::Typed),
        SubmitOutcome::Delivered
    );
}

/// The opposite of the test above, and the whole point of RD-107-03: the same widget
/// that refuses a typed guess takes a token the desktop agent harvested from the
/// hoster's own page, because only that page can have produced one.
#[tokio::test]
async fn a_browser_token_answers_a_widget_the_typed_path_refuses() {
    let queue = ManualQueue::default();
    let (pending, receiver) = queue.enqueue(&widget(), std::time::Duration::from_secs(60));

    assert_eq!(
        queue.resolve(
            pending.id,
            Some(CaptchaAnswer::Token("0.abcdef".to_owned())),
            AnswerSource::Browser
        ),
        SubmitOutcome::Delivered
    );
    assert_eq!(
        receiver.await.expect("sender kept"),
        Reply::Answer(CaptchaAnswer::Token("0.abcdef".to_owned()))
    );
    assert!(queue.pending().is_empty());
}

/// The desktop agent is handed the page and the site key and nothing else: a capture
/// token must not become a way to read image captchas or their prompts.
#[test]
fn the_agents_view_lists_widgets_only_and_carries_no_image() {
    let queue = ManualQueue::default();
    let (_image, _image_receiver) = queue.enqueue(&image(), std::time::Duration::from_secs(60));
    let (expected, _widget_receiver) = queue.enqueue(&widget(), std::time::Duration::from_secs(60));

    let widgets = queue.pending_widgets();

    assert_eq!(widgets.len(), 1, "the image challenge is not an agent task");
    assert_eq!(widgets[0].id, expected.id);
    assert_eq!(widgets[0].kind, CaptchaKind::Turnstile);
    assert_eq!(widgets[0].page_url, "https://katfile.biz/abc/file.rar");
    assert_eq!(widgets[0].site_key, "0x4AAA");
}

/// An agent must never be sent to a window that is already over.
#[test]
fn an_expired_widget_is_not_offered_to_the_agent() {
    let queue = ManualQueue::default();
    let (_pending, _receiver) = queue.enqueue(&widget(), std::time::Duration::ZERO);

    assert!(queue.pending_widgets().is_empty());
}

/// Listing the queue sweeps it, so an expired challenge is neither offered to the user
/// nor left holding a resolver that stopped waiting for it.
#[tokio::test]
async fn an_expired_challenge_is_swept_out_of_the_queue() {
    let queue = ManualQueue::default();
    let (_pending, receiver) = queue.enqueue(&image(), std::time::Duration::ZERO);

    assert!(queue.pending().is_empty());
    assert!(
        receiver.await.is_err(),
        "sweeping wakes the resolver instead of leaving it hanging"
    );
}

#[test]
fn forgetting_reports_whether_anything_was_queued() {
    let queue = ManualQueue::default();
    let (pending, _receiver) = queue.enqueue(&image(), std::time::Duration::from_secs(60));

    assert!(queue.forget(pending.id));
    assert!(
        !queue.forget(pending.id),
        "forgetting twice changes nothing"
    );
}

#[test]
fn an_unexpected_image_type_falls_back_to_jpeg() {
    assert_eq!(image_mime("image/PNG"), "image/png");
    assert_eq!(image_mime("text/html\";alert(1)"), "image/jpeg");
    assert_eq!(image_mime(""), "image/jpeg");
}

/// RD-110-15: a click-point captcha is a picture like an image captcha, and the UI needs the
/// same things to show it — but its answer is a point, and the queue delivers it as one.
#[tokio::test]
async fn a_click_point_captcha_shows_its_picture_and_takes_a_point() {
    let queue = ManualQueue::default();
    let (pending, receiver) = queue.enqueue(&click_point(), std::time::Duration::from_secs(60));

    assert_eq!(pending.kind, CaptchaKind::ClickPoint);
    assert_eq!(pending.image.as_deref(), Some("data:image/png;base64,Qk0="));
    assert_eq!(pending.prompt.as_deref(), Some("Click the circle"));
    assert!(pending.site_key.is_none() && pending.page_url.is_none());

    assert_eq!(
        resolve_point(&queue, pending.id, ClickPoint { x: 120, y: 44 }),
        SubmitOutcome::Delivered
    );
    assert_eq!(
        receiver.await.expect("sender kept"),
        Reply::Answer(CaptchaAnswer::Point(ClickPoint { x: 120, y: 44 }))
    );
    assert!(queue.pending().is_empty());
}

/// Text cannot answer a click, and a click cannot answer text. Either mistake leaves the
/// challenge waiting for the answer it actually takes.
#[test]
fn an_answer_of_the_wrong_shape_is_refused_and_the_challenge_keeps_waiting() {
    let queue = ManualQueue::default();
    let (clicked, _click_receiver) =
        queue.enqueue(&click_point(), std::time::Duration::from_secs(60));
    let (typed, _typed_receiver) = queue.enqueue(&image(), std::time::Duration::from_secs(60));
    let (widget, _widget_receiver) = queue.enqueue(&widget(), std::time::Duration::from_secs(60));

    assert_eq!(
        queue.resolve(
            clicked.id,
            Some(CaptchaAnswer::Token("42".to_owned())),
            AnswerSource::Typed
        ),
        SubmitOutcome::WrongAnswerShape,
        "text for a click-point captcha"
    );
    assert_eq!(
        queue.resolve(
            clicked.id,
            Some(CaptchaAnswer::Token("0.abc".to_owned())),
            AnswerSource::Browser
        ),
        SubmitOutcome::WrongAnswerShape,
        "a browser token is still text"
    );
    assert_eq!(
        resolve_point(&queue, typed.id, ClickPoint { x: 1, y: 1 }),
        SubmitOutcome::WrongAnswerShape,
        "a point for an image captcha"
    );
    assert_eq!(
        resolve_point(&queue, widget.id, ClickPoint { x: 1, y: 1 }),
        SubmitOutcome::WrongAnswerShape,
        "a point for a widget"
    );
    assert_eq!(
        queue.pending().len(),
        3,
        "every refusal left its challenge waiting"
    );
}

/// A CutCaptcha is never queued by the broker; should one ever be, it must still not be
/// offered to a browser, which could open the page but not read the token out of it.
#[test]
fn a_cutcaptcha_is_described_but_never_offered_to_a_browser() {
    let queue = ManualQueue::default();
    let (pending, _receiver) = queue.enqueue(&cutcaptcha(), std::time::Duration::from_secs(60));

    assert_eq!(pending.kind, CaptchaKind::Cutcaptcha);
    assert_eq!(pending.host.as_deref(), Some("filecrypt.cc"));
    assert_eq!(pending.site_key.as_deref(), Some("SAs61IAI"));
    assert!(queue.pending_widgets().is_empty());
    assert_eq!(
        queue.resolve(
            pending.id,
            Some(CaptchaAnswer::Token("typed".to_owned())),
            AnswerSource::Typed
        ),
        SubmitOutcome::WrongAnswerShape
    );
}

/// RD-120-45: the extension opened the hoster's page and found no widget on it. The resolver
/// learns exactly that, rather than waiting out its timeout or reading it as a person's decline.
#[tokio::test]
async fn a_page_without_its_widget_ends_the_wait_with_its_own_reply() {
    let queue = ManualQueue::default();
    let (pending, receiver) = queue.enqueue(&widget(), std::time::Duration::from_secs(60));

    assert_eq!(
        queue.report_page_without_widget(pending.id),
        SubmitOutcome::Delivered
    );
    assert_eq!(
        receiver.await.expect("sender kept"),
        Reply::PageWithoutWidget
    );
    assert!(
        queue.pending().is_empty(),
        "the challenge stops being offered"
    );
    assert_eq!(
        queue.report_page_without_widget(pending.id),
        SubmitOutcome::NotWaiting,
        "reporting twice must not succeed"
    );
}

/// Only a widget is opened in a browser, so only a widget can be reported missing; an image
/// captcha keeps waiting for its answer.
#[test]
fn only_a_widget_can_be_reported_missing_from_its_page() {
    let queue = ManualQueue::default();
    let (pending, _receiver) = queue.enqueue(&image(), std::time::Duration::from_secs(60));

    assert_eq!(
        queue.report_page_without_widget(pending.id),
        SubmitOutcome::WrongAnswerShape
    );
    assert_eq!(queue.pending().len(), 1, "the image captcha keeps waiting");
}

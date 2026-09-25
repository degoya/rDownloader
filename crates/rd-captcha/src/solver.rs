//! Client for solver services speaking the 2captcha `createTask`/`getTaskResult` JSON API
//! (2captcha, CapMonster, CapSolver, …).

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use rd_core::{Failure, FailureKind};
use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, ClickPoint, CutcaptchaChallenge, ImageChallenge,
    WidgetChallenge,
};
use serde_json::{Value, json};

/// How long to keep polling one task before giving up.
const MAX_POLL: Duration = Duration::from_secs(150);
/// Delay between result polls; services need several seconds for a widget captcha.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Grace period before the first poll, so the obvious "not ready yet" round trip is skipped.
const FIRST_POLL_DELAY: Duration = Duration::from_secs(8);

/// Submits a challenge and polls until the service returns an answer in the challenge's
/// shape: a token, or a point for a click-point captcha.
pub(crate) async fn solve(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    challenge: &CaptchaChallenge,
) -> Result<CaptchaAnswer, Failure> {
    let task = task_for(challenge);
    let created = post(
        http,
        endpoint,
        "createTask",
        &json!({ "clientKey": api_key, "task": task }),
    )
    .await?;
    check_error(&created)?;
    let task_id = created
        .get("taskId")
        .and_then(|value| {
            value
                .as_i64()
                .map(|id| id.to_string())
                .or_else(|| value.as_str().map(str::to_owned))
        })
        .ok_or_else(|| solver_failure("solver did not return a task id"))?;

    tokio::time::sleep(FIRST_POLL_DELAY).await;
    let deadline = tokio::time::Instant::now() + MAX_POLL;
    loop {
        let result = post(
            http,
            endpoint,
            "getTaskResult",
            &json!({ "clientKey": api_key, "taskId": task_id }),
        )
        .await?;
        check_error(&result)?;
        if result.get("status").and_then(Value::as_str) == Some("ready") {
            return answer_from(&result, challenge.answers_with_point())
                .ok_or_else(|| solver_failure("solver returned a solution of the wrong shape"));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::coded(
                FailureKind::CaptchaFailed,
                "captcha.solver_timeout",
                "The captcha solver did not answer in time",
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Asks the service what the key is worth, which is the cheapest way to prove that endpoint
/// and key work together before a download depends on them.
pub(crate) async fn balance(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<f64, Failure> {
    let response = post(
        http,
        endpoint,
        "getBalance",
        &json!({ "clientKey": api_key }),
    )
    .await?;
    check_error(&response)?;
    response
        .get("balance")
        .and_then(Value::as_f64)
        .ok_or_else(|| solver_failure("solver did not report a balance"))
}

fn task_for(challenge: &CaptchaChallenge) -> Value {
    fn widget(kind: &str, value: &WidgetChallenge) -> Value {
        json!({
            "type": kind,
            "websiteURL": value.page_url,
            "websiteKey": value.site_key,
            "isInvisible": value.invisible,
        })
    }
    match challenge {
        CaptchaChallenge::RecaptchaV2(value) => widget("RecaptchaV2TaskProxyless", value),
        CaptchaChallenge::HCaptcha(value) => widget("HCaptchaTaskProxyless", value),
        CaptchaChallenge::Turnstile(value) => widget("TurnstileTaskProxyless", value),
        CaptchaChallenge::Image(ImageChallenge { data, .. }) => json!({
            "type": "ImageToTextTask",
            "body": STANDARD.encode(data),
        }),
        // The prompt is the instruction the worker needs ("click the odd one out"); without it
        // a coordinate task has no question.
        CaptchaChallenge::ClickPoint(ImageChallenge { data, prompt, .. }) => json!({
            "type": "CoordinatesTask",
            "body": STANDARD.encode(data),
            "comment": prompt.as_deref().unwrap_or_default(),
        }),
        CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge {
            site_key,
            misery_key,
            page_url,
        }) => json!({
            "type": "CutCaptchaTaskProxyless",
            "miseryKey": misery_key,
            "apiKey": site_key,
            "websiteURL": page_url,
        }),
    }
}

/// Reads the answer in the shape the challenge has, or `None` when the service answered
/// with something else — an empty token, or a token where a point was asked for.
fn answer_from(result: &Value, point: bool) -> Option<CaptchaAnswer> {
    if point {
        return point_from(result).map(CaptchaAnswer::Point);
    }
    token_from(result).map(CaptchaAnswer::Token)
}

/// Services disagree on the solution field: reCAPTCHA and hCaptcha answer with
/// `gRecaptchaResponse`, Turnstile and CutCaptcha with `token`, image tasks with `text`.
fn token_from(result: &Value) -> Option<String> {
    let solution = result.get("solution")?;
    ["gRecaptchaResponse", "token", "text"]
        .into_iter()
        .find_map(|field| solution.get(field).and_then(Value::as_str))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

/// A coordinate task answers with a list of points; a click-point captcha wants exactly one,
/// and the first is it. Services write the numbers as numbers or as strings.
fn point_from(result: &Value) -> Option<ClickPoint> {
    let first = result.get("solution")?.get("coordinates")?.get(0)?;
    let axis = |name: &str| {
        let value = first.get(name)?;
        value
            .as_u64()
            .or_else(|| value.as_f64().map(|float| float.max(0.0) as u64))
            .or_else(|| {
                value
                    .as_str()?
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|float| float.max(0.0) as u64)
            })
            .and_then(|whole| u32::try_from(whole).ok())
    };
    Some(ClickPoint {
        x: axis("x")?,
        y: axis("y")?,
    })
}

fn check_error(response: &Value) -> Result<(), Failure> {
    let failed = response
        .get("errorId")
        .and_then(Value::as_i64)
        .is_some_and(|id| id != 0);
    if !failed {
        return Ok(());
    }
    let code = response
        .get("errorCode")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    // A rejected or exhausted key is a configuration problem the user must fix, not
    // something to retry on every link of the queue.
    let category = if code.contains("KEY") || code.contains("BALANCE") {
        FailureKind::Permanent
    } else {
        FailureKind::CaptchaFailed
    };
    Err(Failure::coded(
        category,
        "captcha.solver_failed",
        format!("Captcha solver rejected the request: {code}"),
    )
    .with_param("reason", code))
}

async fn post(
    http: &reqwest::Client,
    endpoint: &str,
    path: &str,
    body: &Value,
) -> Result<Value, Failure> {
    let url = format!("{}/{path}", endpoint.trim_end_matches('/'));
    let response = http
        .post(url)
        .json(body)
        .send()
        .await
        // The URL carries the solver API key in neither path nor query, but the error's
        // Display appends the full URL; strip it anyway to keep the habit.
        .map_err(|error| solver_failure(&error.without_url().to_string()))?;
    if !response.status().is_success() {
        return Err(solver_failure(&format!("HTTP {}", response.status())));
    }
    response
        .json()
        .await
        .map_err(|error| solver_failure(&error.without_url().to_string()))
}

fn solver_failure(reason: &str) -> Failure {
    Failure::coded(
        FailureKind::CaptchaFailed,
        "captcha.solver_failed",
        format!("Captcha solver request failed: {reason}"),
    )
    .with_param("reason", reason.to_owned())
}

#[cfg(test)]
mod tests {
    use rd_plugin_api::{
        CaptchaAnswer, CaptchaChallenge, ClickPoint, CutcaptchaChallenge, ImageChallenge,
        WidgetChallenge,
    };
    use serde_json::json;

    use super::{answer_from, check_error, task_for, token_from};

    /// RD-110-15: the two new kinds map to the tasks the 2captcha API names for them, and a
    /// CutCaptcha task carries both keys the service insists on.
    #[test]
    fn click_point_and_cutcaptcha_challenges_map_to_their_task_types() {
        let clicked = CaptchaChallenge::ClickPoint(ImageChallenge {
            mime: "image/png".to_owned(),
            data: b"BM".to_vec(),
            prompt: Some("Click the circle".to_owned()),
        });
        let task = task_for(&clicked);
        assert_eq!(task["type"], "CoordinatesTask");
        assert_eq!(task["body"], "Qk0=");
        assert_eq!(task["comment"], "Click the circle");

        let cut = CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge {
            site_key: "SAs61IAI".to_owned(),
            misery_key: "a1488b66da00bf332a1488993a5443c79047e752".to_owned(),
            page_url: "https://filecrypt.cc/Container/ABC.html".to_owned(),
        });
        let task = task_for(&cut);
        assert_eq!(task["type"], "CutCaptchaTaskProxyless");
        assert_eq!(task["apiKey"], "SAs61IAI");
        assert_eq!(
            task["miseryKey"],
            "a1488b66da00bf332a1488993a5443c79047e752"
        );
        assert_eq!(
            task["websiteURL"],
            "https://filecrypt.cc/Container/ABC.html"
        );
    }

    /// The answer is read in the challenge's shape: a point for a coordinate task, whether the
    /// service wrote the numbers as numbers or strings, and never a token in its place.
    #[test]
    fn a_coordinate_answer_is_a_point_and_a_token_is_not() {
        let numbers =
            json!({ "status": "ready", "solution": { "coordinates": [{ "x": 179, "y": 154 }] } });
        assert_eq!(
            answer_from(&numbers, true),
            Some(CaptchaAnswer::Point(ClickPoint { x: 179, y: 154 }))
        );
        let strings = json!({ "status": "ready", "solution": { "coordinates": [{ "x": "12", "y": "7.4" }] } });
        assert_eq!(
            answer_from(&strings, true),
            Some(CaptchaAnswer::Point(ClickPoint { x: 12, y: 7 }))
        );
        let token = json!({ "status": "ready", "solution": { "token": "abc" } });
        assert_eq!(
            answer_from(&token, true),
            None,
            "a token cannot answer a click"
        );
        assert_eq!(
            answer_from(&token, false),
            Some(CaptchaAnswer::Token("abc".to_owned()))
        );
        assert_eq!(
            answer_from(&numbers, false),
            None,
            "a point cannot answer text"
        );
    }

    #[test]
    fn widget_and_image_challenges_map_to_their_task_types() {
        let widget = CaptchaChallenge::RecaptchaV2(WidgetChallenge {
            site_key: "6Lc-key".to_owned(),
            page_url: "https://rapidgator.net/download/captcha".to_owned(),
            invisible: false,
        });
        let task = task_for(&widget);
        assert_eq!(task["type"], "RecaptchaV2TaskProxyless");
        assert_eq!(task["websiteKey"], "6Lc-key");
        assert_eq!(task["isInvisible"], false);

        let image = CaptchaChallenge::Image(ImageChallenge {
            mime: "image/png".to_owned(),
            data: b"BM".to_vec(),
            prompt: None,
        });
        let task = task_for(&image);
        assert_eq!(task["type"], "ImageToTextTask");
        assert_eq!(task["body"], "Qk0=");
    }

    #[test]
    fn every_services_solution_field_is_understood() {
        for field in ["gRecaptchaResponse", "token", "text"] {
            let result = json!({ "status": "ready", "solution": { field: "answer" } });
            assert_eq!(token_from(&result).as_deref(), Some("answer"), "{field}");
        }
        assert_eq!(token_from(&json!({ "status": "processing" })), None);
        let empty = json!({ "solution": { "text": "" } });
        assert_eq!(token_from(&empty), None, "an empty token is no solution");
    }

    /// A bad or empty key must not be retried for every link in the queue.
    #[test]
    fn key_and_balance_errors_are_permanent_but_others_are_retryable() {
        let key = check_error(&json!({ "errorId": 1, "errorCode": "ERROR_KEY_DOES_NOT_EXIST" }))
            .expect_err("error");
        assert_eq!(key.category, rd_core::FailureKind::Permanent);

        let balance = check_error(&json!({ "errorId": 1, "errorCode": "ERROR_ZERO_BALANCE" }))
            .expect_err("error");
        assert_eq!(balance.category, rd_core::FailureKind::Permanent);

        let busy = check_error(&json!({ "errorId": 1, "errorCode": "ERROR_NO_SLOT_AVAILABLE" }))
            .expect_err("error");
        assert_eq!(busy.category, rd_core::FailureKind::CaptchaFailed);

        assert!(check_error(&json!({ "errorId": 0, "taskId": 7 })).is_ok());
    }
}

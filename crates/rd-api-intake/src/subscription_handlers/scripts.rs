//! Script subscriptions: the script, its schedule and arguments, and who may set them.

use super::*;

/// Longest script name, the same bound the scripts directory's resolver applies.
pub(super) const MAX_SCRIPT_NAME: usize = 128;

/// The `script:<name>` address of a script subscription (RD-130-19).
///
/// A bare name is accepted as well, because it is what somebody types. The name follows the
/// scripts directory's own rule -- letters, digits, `.`, `_`, `-`, not starting with a dot --
/// so a subscription can only ever point at a file directly inside that directory.
pub(super) fn script_url(raw: &str) -> Result<url::Url, ApiError> {
    let name = raw
        .strip_prefix(rd_core::SCRIPT_URL_SCHEME)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(raw);
    let valid = !name.is_empty()
        && name.len() <= MAX_SCRIPT_NAME
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(ApiError::unprocessable(
            "subscription.script_name_invalid",
            "A script is named by a file directly inside the scripts directory",
        )
        .with_param("value", name.chars().take(64).collect::<String>()));
    }
    url::Url::parse(&format!("{}:{name}", rd_core::SCRIPT_URL_SCHEME)).map_err(|_| {
        ApiError::unprocessable(
            "subscription.script_name_invalid",
            "A script is named by a file directly inside the scripts directory",
        )
        .with_param("value", name.to_owned())
    })
}

/// The cron expression, checked to name a time (RD-130-19); `None` keeps the interval.
pub(super) fn schedule_input(request: &SubscriptionRequest) -> Result<Option<String>, ApiError> {
    let Some(expression) = request
        .schedule
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    // Script subscriptions only, for now: every other kind polls somebody else's server, and
    // `* * * * *` would walk straight past the per-kind interval floor that protects it.
    if request.kind != SubscriptionKind::Script {
        return Err(ApiError::unprocessable(
            "subscription.schedule_kind",
            "Only a script subscription runs on a schedule",
        ));
    }
    rd_subscription::next_scheduled(expression, chrono::Utc::now(), &chrono::Local).map_err(
        |error| {
            ApiError::unprocessable("subscription.schedule_invalid", error.to_string())
                .with_param("value", expression.chars().take(64).collect::<String>())
        },
    )?;
    Ok(Some(expression.to_owned()))
}

/// The arguments a script subscription hands its script (RD-150-08), checked one by one.
///
/// Nothing is trimmed or dropped: each entry is an argument exactly as the script will see
/// it, an empty one and a trailing space included. A line break is refused because a batch
/// file on Windows would end its command line there, and a NUL because no argv can carry one.
pub(super) fn script_arguments_input(
    request: &SubscriptionRequest,
) -> Result<Vec<String>, ApiError> {
    let arguments = &request.script_arguments;
    if arguments.is_empty() {
        return Ok(Vec::new());
    }
    if request.kind != SubscriptionKind::Script {
        return Err(ApiError::unprocessable(
            "subscription.script_arguments_kind",
            "Only a script subscription takes arguments",
        ));
    }
    if arguments.len() > rd_core::MAX_SCRIPT_ARGUMENTS {
        return Err(ApiError::unprocessable(
            "subscription.script_arguments_too_many",
            "Too many script arguments",
        )
        .with_param("maximum", rd_core::MAX_SCRIPT_ARGUMENTS.to_string()));
    }
    for (index, argument) in arguments.iter().enumerate() {
        let position = (index + 1).to_string();
        if argument.chars().count() > rd_core::MAX_SCRIPT_ARGUMENT_CHARS {
            return Err(ApiError::unprocessable(
                "subscription.script_argument_too_long",
                "A script argument is too long",
            )
            .with_param("position", position)
            .with_param("maximum", rd_core::MAX_SCRIPT_ARGUMENT_CHARS.to_string()));
        }
        if argument.contains(['\0', '\n', '\r']) {
            return Err(ApiError::unprocessable(
                "subscription.script_argument_invalid",
                "A script argument may not contain a line break or a NUL character",
            )
            .with_param("position", position));
        }
    }
    Ok(arguments.clone())
}

/// Records that a script subscription was created or changed (RD-150-08).
///
/// Which script runs on this machine, when, and with which arguments is the administrator's
/// decision, so every such change is kept -- the arguments included, which is one more reason
/// they must never hold a secret. `involved` is whether a script was on either side of it; a
/// script turned into another kind is recorded with an empty `script`.
pub(super) async fn audit_script_change(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    subscription: &Subscription,
    change: &str,
    involved: bool,
) {
    if !involved {
        return;
    }
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::ScriptSubscriptionChanged)
            .by(audit)
            .target("subscription", subscription.id)
            .named(subscription.name.clone())
            .detail("change", change)
            .detail("script", subscription.script_name().unwrap_or_default())
            .detail(
                "arguments",
                serde_json::to_string(&subscription.script_arguments).unwrap_or_default(),
            )
            .detail(
                "schedule",
                subscription.schedule.as_deref().unwrap_or_default(),
            ),
    )
    .await;
}

/// A script subscription starts code on this machine, so creating one, changing one, or
/// turning one into something else costs the administration scope (RD-130-19) -- whatever
/// the route itself costs. `kinds` are the kinds involved: the requested one, and on an edit
/// the stored one.
pub(super) fn require_admin_for_script(
    granted: Option<&crate::auth::Granted>,
    kinds: &[SubscriptionKind],
) -> Result<(), ApiError> {
    if !kinds.contains(&SubscriptionKind::Script)
        || granted.is_some_and(|granted| granted.holds(rd_core::Scope::Admin))
    {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "auth.scope_insufficient",
        "A script subscription requires the administration scope",
    )
    .with_param("scope", rd_core::Scope::Admin.as_str()))
}

/// The same rule for an action on a stored subscription: switching a script subscription on
/// or off decides when code runs, and "check now" runs it (RD-130-19). An unknown id passes,
/// so the action itself answers with its own 404.
pub(super) async fn require_admin_for_stored_script(
    state: &AppState,
    granted: Option<&crate::auth::Granted>,
    id: SubscriptionId,
) -> Result<(), ApiError> {
    let stored = state
        .database
        .subscription(id)
        .await?
        .map(|subscription| subscription.kind);
    require_admin_for_script(granted, &stored.into_iter().collect::<Vec<_>>())
}

/// Refuses a script subscription whose script is not in the scripts directory.
///
/// Checked when it is saved rather than only when it runs, so a typo is a form error and not
/// a failure in the history the next morning. The run checks again, because a file can go.
pub(super) async fn ensure_script_exists(
    state: &AppState,
    input: &NewSubscription,
) -> Result<(), ApiError> {
    if input.kind != SubscriptionKind::Script {
        return Ok(());
    }
    let name = input.url.path();
    let directory = state.extraction.scripts_directory().await?;
    if tokio::fs::metadata(directory.join(name))
        .await
        .is_ok_and(|meta| meta.is_file())
    {
        return Ok(());
    }
    Err(ApiError::unprocessable(
        "subscription.script_not_found",
        "There is no such script in the scripts directory",
    )
    .with_param("name", name.to_owned()))
}

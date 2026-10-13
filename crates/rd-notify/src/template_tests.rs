//! RD-1280-02: a rule's own wording, filled from the placeholder table.

use chrono::{FixedOffset, TimeZone as _, Utc};
use rd_core::{NotificationDeliveryId, NotificationRuleId, NotificationTargetId};

use super::{
    MAX_BODY_BYTES, MAX_BODY_CHARS, MAX_TITLE_CHARS, TemplateError, TemplatePart, Values, render,
    size_text, validate_template,
};
use crate::{
    Delivery, DeliveryState, NotificationEvent, NotificationRule, Placeholder, Severity,
    message_of, placeholder_table_markdown,
};

fn rule(title: Option<&str>, body: Option<&str>) -> NotificationRule {
    NotificationRule {
        id: NotificationRuleId::new(),
        name: "own words".to_owned(),
        enabled: true,
        target_id: NotificationTargetId::new(),
        events: Vec::new(),
        category_id: None,
        min_severity: Severity::Info,
        title_template: title.map(str::to_owned),
        body_template: body.map(str::to_owned),
    }
}

fn values(event: NotificationEvent) -> Values {
    let at = FixedOffset::east_opt(2 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2026, 10, 13, 14, 32, 0)
        .single()
        .expect("moment");
    Values::stamped(event, &at)
}

fn fill(template: &str, values: &Values) -> String {
    render(TemplatePart::Body, template, values).expect("a valid template")
}

/// The table, pinned: which event fills which placeholder beyond the common six.
#[test]
fn every_event_fills_its_own_placeholders() {
    use NotificationEvent as E;
    use Placeholder as P;
    let expected: &[(NotificationEvent, &[Placeholder])] = &[
        (
            E::PackageCompleted,
            &[P::Package, P::Category, P::State, P::Size, P::Path],
        ),
        (
            E::PackageFailed,
            &[P::Package, P::Category, P::State, P::Size, P::Error],
        ),
        (E::StorageBlocked, &[P::StorageTarget, P::Path]),
        (E::BudgetExhausted, &[]),
        (E::CaptchaWaiting, &[]),
        (E::PowerPending, &[]),
        (E::BackupFailed, &[P::Error]),
        (E::BackupVerifyFailed, &[P::StorageTarget, P::Error]),
        (E::UpdateAvailable, &[P::Version]),
        (E::PluginUpdateAvailable, &[P::Plugin, P::Version]),
        (E::PluginUpdateFailed, &[P::Plugin, P::Version, P::Error]),
        (E::UpdateInstalled, &[P::Version]),
        (E::UpdateFailed, &[P::Version, P::Error]),
        (E::ServiceRestarting, &[]),
        (E::AccountExpiring, &[P::Account]),
        (E::AccountInvalid, &[P::Account, P::Error]),
        (E::UsenetJobHopeless, &[P::Package, P::Category]),
        (E::UsenetQuotaReached, &[]),
        (E::StopMarkReached, &[P::Package, P::File, P::Category]),
        (
            E::StreamRecorded,
            &[P::File, P::Package, P::Category, P::Size],
        ),
        (E::SubscriptionMatched, &[P::Category]),
    ];
    for (event, specific) in expected {
        let mut wanted = Placeholder::COMMON.to_vec();
        wanted.extend_from_slice(specific);
        assert_eq!(event.placeholders(), wanted, "{event:?}");
        assert!(event.takes_template(), "{event:?}");
    }
    // A burst arrives as one summary and keeps its text; an automation goes through no rule.
    for event in [E::DownloadStarted, E::LinksAdded, E::Automation] {
        assert!(!event.takes_template(), "{event:?}");
        assert!(event.placeholders().is_empty(), "{event:?}");
    }
    let named: Vec<_> = expected.iter().map(|(event, _)| *event).collect();
    for event in NotificationEvent::all() {
        assert!(
            named.contains(&event) || !event.takes_template(),
            "{event:?} is missing from this table"
        );
    }
}

#[test]
fn a_template_fills_every_placeholder_its_event_names() {
    for event in NotificationEvent::all()
        .into_iter()
        .filter(|event| event.takes_template())
    {
        let mut filled = values(event);
        for placeholder in event.placeholders() {
            if !Placeholder::COMMON.contains(&placeholder) {
                filled.set(placeholder, placeholder.example());
            }
        }
        let template = event
            .placeholders()
            .iter()
            .map(|placeholder| format!("{{{}}}", placeholder.name()))
            .collect::<Vec<_>>()
            .join("|");
        let rule = rule(None, Some(template.as_str()));
        let (title, body) = rule.wording("Built-in title", "Built-in text", &filled);
        assert_eq!(title, "Built-in title", "{event:?}: no title template");
        let parts: Vec<&str> = body.split('|').collect();
        assert_eq!(parts.len(), event.placeholders().len(), "{event:?}: {body}");
        assert_eq!(parts[0], event.code());
        assert_eq!(parts[2], "14:32");
        assert_eq!(parts[3], "2026-10-13");
        assert_eq!(parts[4], "Built-in title");
        assert_eq!(parts[5], "Built-in text");
        for (part, placeholder) in parts.iter().zip(event.placeholders()).skip(6) {
            assert_eq!(*part, placeholder.example(), "{event:?}");
        }
    }
}

#[test]
fn a_placeholder_without_a_value_becomes_empty_text() {
    let filled = values(NotificationEvent::PackageCompleted).with(Placeholder::Package, "Photos");
    // `{error}` is no value of a finished package, `{path}` one this occasion did not have.
    assert_eq!(fill("{package}{error} at {path}.", &filled), "Photos at .");
    // A part that comes out empty is sent in its built-in wording.
    let rule = rule(Some("{error}"), Some("  {file}  "));
    assert_eq!(
        rule.wording("Package finished: Photos", "Photos (completed)", &filled),
        (
            "Package finished: Photos".to_owned(),
            "Photos (completed)".to_owned()
        )
    );
}

#[test]
fn an_unknown_placeholder_is_refused_with_its_code() {
    let error = validate_template(TemplatePart::Title, "Done: {pakage}").expect_err("refused");
    assert_eq!(
        error,
        TemplateError::UnknownPlaceholder {
            name: "pakage".to_owned()
        }
    );
    assert_eq!(error.code(), "notification.template_unknown_placeholder");
    // Spaces inside the braces are forgiven, the name is not.
    assert!(validate_template(TemplatePart::Body, "{ package }").is_ok());
    assert!(validate_template(TemplatePart::Body, "{Package}").is_err());
}

#[test]
fn a_brace_is_written_doubled_and_an_unbalanced_one_is_refused() {
    let filled = values(NotificationEvent::PackageCompleted).with(Placeholder::Package, "Photos");
    assert_eq!(
        fill("{{package}} is {package}", &filled),
        "{package} is Photos"
    );
    assert_eq!(fill("{{{package}}}", &filled), "{Photos}");
    for broken in ["{package", "package}", "{}", "{ }", "{pack{age}}", "{"] {
        assert_eq!(
            validate_template(TemplatePart::Body, broken),
            Err(TemplateError::Syntax),
            "{broken}"
        );
        assert_eq!(TemplateError::Syntax.code(), "notification.template_syntax");
        assert!(render(TemplatePart::Body, broken, &filled).is_none());
    }
}

#[test]
fn a_template_longer_than_its_part_allows_is_refused() {
    let title = "x".repeat(super::MAX_TITLE_TEMPLATE_CHARS + 1);
    let error = validate_template(TemplatePart::Title, &title).expect_err("too long");
    assert_eq!(error.code(), "notification.template_too_long");
    assert_eq!(
        error,
        TemplateError::TooLong {
            max: super::MAX_TITLE_TEMPLATE_CHARS
        }
    );
    // Counted in characters, so an umlaut costs one, not two.
    let body = "\u{e4}".repeat(super::MAX_BODY_TEMPLATE_CHARS);
    assert!(validate_template(TemplatePart::Body, &body).is_ok());
}

/// Values are data: they are not expanded again, their line breaks do not split a title, and
/// every script, emoji and markup comes through as written.
#[test]
fn special_characters_and_unicode_pass_through_as_data() {
    let filled = values(NotificationEvent::PackageFailed)
        .with(
            Placeholder::Package,
            "\u{1F4E6} M\u{fc}ller \u{05e9}\u{05dc}\u{05d5}\u{05dd} {error} <b>&amp;</b> 100% $1",
        )
        .with(Placeholder::Error, "line one\r\nline two\u{7}");
    assert_eq!(
        fill("{package}", &filled),
        "\u{1F4E6} M\u{fc}ller \u{05e9}\u{05dc}\u{05d5}\u{05dd} {error} <b>&amp;</b> 100% $1"
    );
    let rule = rule(Some("Failed: {error}"), Some("Error:\n{error}"));
    let (title, body) = rule.wording("t", "b", &filled);
    assert_eq!(title, "Failed: line one  line two");
    assert!(!title.contains(['\r', '\n']), "a mail subject is one line");
    // The template's own line breaks stay in the text; a value's do not.
    assert_eq!(body, "Error:\nline one  line two");
    assert_eq!(fill("\u{1F600} {severity}", &filled), "\u{1F600} error");
}

/// What a destination takes, and that a filled template never goes beyond it: the limits each
/// destination documents for a title and a message.
#[test]
fn a_filled_template_fits_every_destination() {
    struct Limit {
        name: &'static str,
        title_chars: usize,
        body_chars: usize,
        body_bytes: usize,
    }
    let limits = [
        Limit {
            name: "ntfy",
            title_chars: 250,
            body_chars: 4096,
            body_bytes: 4096,
        },
        Limit {
            name: "Telegram",
            title_chars: 4096,
            body_chars: 4096,
            body_bytes: usize::MAX,
        },
        Limit {
            name: "Discord",
            title_chars: 256,
            body_chars: 2000,
            body_bytes: usize::MAX,
        },
        Limit {
            name: "e-mail",
            title_chars: 998,
            body_chars: usize::MAX,
            body_bytes: usize::MAX,
        },
        Limit {
            name: "webhook",
            title_chars: usize::MAX,
            body_chars: usize::MAX,
            body_bytes: usize::MAX,
        },
    ];
    // Four bytes a character, so the byte limit bites before the character limit.
    let huge = "\u{1F4E6}".repeat(5000);
    let filled =
        values(NotificationEvent::PackageCompleted).with(Placeholder::Package, huge.as_str());
    let rule = rule(Some("{package}"), Some("{package}\n{package}"));
    let (title, body) = rule.wording("t", "b", &filled);
    assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
    assert!(title.ends_with('\u{2026}'));
    assert!(body.chars().count() <= MAX_BODY_CHARS);
    assert!(body.len() <= MAX_BODY_BYTES);
    assert!(body.ends_with('\u{2026}'));
    for limit in limits {
        assert!(title.chars().count() <= limit.title_chars, "{}", limit.name);
        assert!(body.chars().count() <= limit.body_chars, "{}", limit.name);
        assert!(body.len() <= limit.body_bytes, "{}", limit.name);
    }
    // Plain ASCII is cut at the character limit.
    let ascii =
        values(NotificationEvent::PackageCompleted).with(Placeholder::Package, "a".repeat(9000));
    let (_, body) = rule.wording("t", "b", &ascii);
    assert_eq!(body.chars().count(), MAX_BODY_CHARS);
    // A short text is never cut.
    assert_eq!(
        fill(
            "{package}",
            &values(NotificationEvent::PackageCompleted).with(Placeholder::Package, "abc")
        ),
        "abc"
    );
}

/// The webhook's JSON keeps its shape: the template fills its `title` and `body`, nothing else.
#[test]
fn the_webhook_body_keeps_its_structure() {
    let filled = values(NotificationEvent::PackageCompleted).with(Placeholder::Package, "Photos");
    let (title, body) = rule(Some("{package} done at {time}"), Some("{message}")).wording(
        "Package finished: Photos",
        "Photos (completed)",
        &filled,
    );
    let now = Utc::now();
    let delivery = Delivery {
        id: NotificationDeliveryId::new(),
        rule_id: NotificationRuleId::new(),
        target_id: NotificationTargetId::new(),
        idempotency_key: "rule:evt".to_owned(),
        event: NotificationEvent::PackageCompleted,
        title,
        body,
        state: DeliveryState::Queued,
        attempt: 0,
        next_attempt_at: None,
        response_status: None,
        response_excerpt: None,
        created_at: now,
        updated_at: now,
        package_path: None,
    };
    let payload = message_of(&delivery).payload;
    let mut keys: Vec<&String> = payload.as_object().expect("an object").keys().collect();
    keys.sort();
    assert_eq!(keys, ["body", "event", "idempotency_key", "title"]);
    assert_eq!(payload["title"], "Photos done at 14:32");
    assert_eq!(payload["body"], "Photos (completed)");
    assert_eq!(payload["event"], "package_completed");
}

#[test]
fn time_and_date_are_read_in_the_zone_they_are_given() {
    let late = Utc
        .with_ymd_and_hms(2026, 10, 13, 23, 30, 0)
        .single()
        .expect("moment")
        .with_timezone(&FixedOffset::east_opt(2 * 3600).expect("offset"));
    let stamped = Values::stamped(NotificationEvent::BackupFailed, &late);
    assert_eq!(stamped.get(Placeholder::Time), "01:30");
    assert_eq!(stamped.get(Placeholder::Date), "2026-10-14");
    assert_eq!(stamped.get(Placeholder::Severity), "error");
    assert_eq!(stamped.get(Placeholder::Event), "backup_failed");
}

#[test]
fn a_rule_without_a_template_keeps_the_built_in_wording() {
    let plain = rule(None, Some("   "));
    assert!(!plain.has_template());
    let filled = values(NotificationEvent::PackageCompleted).with(Placeholder::Package, "Photos");
    assert_eq!(
        plain.wording("Package finished: Photos", "Photos (completed)", &filled),
        (
            "Package finished: Photos".to_owned(),
            "Photos (completed)".to_owned()
        )
    );
    assert!(rule(Some("{package}"), None).has_template());
    // A rule stored before RD-1280-02 has neither field.
    let stored: NotificationRule = serde_json::from_value(serde_json::json!({
        "id": NotificationRuleId::new(), "name": "old", "enabled": true,
        "target_id": NotificationTargetId::new(), "events": [], "category_id": null,
        "min_severity": "info"
    }))
    .expect("an old rule");
    assert!(!stored.has_template());
}

#[test]
fn a_size_reads_in_binary_units() {
    assert_eq!(size_text(512), "512 B");
    assert_eq!(size_text(1536), "1.5 KiB");
    assert_eq!(size_text(1_503_238_554), "1.4 GiB");
}

#[test]
fn every_placeholder_name_reads_back() {
    for placeholder in Placeholder::ALL {
        assert_eq!(
            Placeholder::from_name(placeholder.name()),
            Some(placeholder)
        );
        assert_eq!(
            serde_json::to_value(placeholder).expect("serialize"),
            serde_json::json!(placeholder.name())
        );
    }
}

/// The handbook's table is this crate's output (`placeholders.md`, copied into the wiki's
/// *Notifications* page): a change to the table shows here first.
#[test]
fn the_handbook_table_is_generated_from_the_code() {
    let committed = include_str!("../placeholders.md");
    assert_eq!(
        committed,
        placeholder_table_markdown(),
        "crates/rd-notify/placeholders.md is stale: write placeholder_table_markdown()'s output \
         there and into the wiki's Notifications page"
    );
}

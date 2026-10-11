//! The automation engine's contract: definitions, conditions, runs and their policy.
//!
//! The crate owns what an automation *is* and how it is judged. Persisting runs and actually
//! executing actions is the service's job, exactly as in `rd-notify`, so the rules stay
//! testable without a database and without a network.

#![warn(unreachable_pub)]

mod condition;
mod model;
mod schedule;

pub use condition::{ConditionError, ConditionNode, EventContext, Field, Operator, Predicate};
pub use model::{
    Action, Automation, AutomationVersion, DefinitionError, LinkDestination, MAX_ACTION_LINKS,
    MAX_ACTIONS, MAX_CONDITION_DEPTH, MAX_LINK_LEN, MAX_NOTIFY_MESSAGE, Run, RunState, Trigger,
    is_script_name, validate, validate_condition, validate_trigger,
};
pub use rd_notify::{MAX_ATTEMPTS, backoff, next_attempt_at};
pub use schedule::{
    GRACE_SECONDS, MAX_CRON_LEN, MAX_INTERVAL_MINUTES, Schedule, ScheduleError, Slot,
};

/// Builds the idempotency key of a run.
///
/// Derived from the version and the event rather than generated, so replaying an event after
/// a crash produces the same key and the unique index drops the duplicate. The *version*
/// rather than the automation: editing an automation is a deliberate act, and the edited
/// definition is entitled to see an event the previous one already handled.
#[must_use]
pub fn idempotency_key(
    version_id: rd_core::AutomationVersionId,
    event_id: &rd_core::EventId,
) -> String {
    format!("{version_id}:{event_id}")
}

/// Builds the idempotency key of a scheduled run (RD-1240-10).
///
/// Keyed on the *automation* and the slot's wall-clock name rather than the version: a slot
/// runs once, whether the service restarted inside its window or the automation was saved
/// again in the same minute. The key is in the run table, so it holds across a restart.
#[must_use]
pub fn schedule_key(automation_id: rd_core::AutomationId, slot: &Slot) -> String {
    format!("{automation_id}:schedule:{}", slot.label())
}

#[cfg(test)]
mod tests {
    use std::slice;

    use rd_core::{AutomationVersionId, EventId};

    use super::{
        Action, ConditionNode, EventContext, Field, MAX_ACTIONS, Operator, Predicate, Trigger,
        idempotency_key, is_script_name, validate,
    };

    fn predicate(field: Field, operator: Operator, value: &str) -> ConditionNode {
        ConditionNode::Predicate {
            predicate: Predicate {
                field,
                operator,
                value: value.to_owned(),
            },
        }
    }

    fn context() -> EventContext {
        let mut context = EventContext::default();
        context.set(Field::Name, "Some.Release.S01E01.mkv");
        context.set(Field::Domain, "example.com");
        context.set(Field::Extension, "mkv");
        context.set(Field::Source, "api");
        context.set_number(Field::SizeBytes, 2_000);
        context
    }

    #[test]
    fn the_same_event_and_version_always_produce_the_same_key() {
        let version = AutomationVersionId::new();
        let event = EventId::new();
        assert_eq!(
            idempotency_key(version, &event),
            idempotency_key(version, &event)
        );
        assert_ne!(
            idempotency_key(version, &event),
            idempotency_key(AutomationVersionId::new(), &event)
        );
    }

    #[test]
    fn text_matching_ignores_case_the_way_routing_rules_do() {
        let event = context();
        assert!(predicate(Field::Name, Operator::Contains, "S01E01").matches(&event));
        assert!(predicate(Field::Name, Operator::Contains, "s01e01").matches(&event));
        assert!(predicate(Field::Extension, Operator::Equals, "MKV").matches(&event));
        assert!(predicate(Field::Domain, Operator::EndsWith, ".COM").matches(&event));
    }

    #[test]
    fn a_field_the_event_does_not_carry_never_matches() {
        // Not even negatively: whether "no category" should satisfy a rule is the author's
        // decision, expressed with `not`, and must not be smuggled in by the comparison.
        let event = context();
        assert!(!predicate(Field::Category, Operator::Equals, "tv").matches(&event));
        assert!(!predicate(Field::Category, Operator::Contains, "").matches(&event));
        assert!(
            ConditionNode::Not {
                node: Box::new(predicate(Field::Category, Operator::Equals, "tv")),
            }
            .matches(&event)
        );
    }

    #[test]
    fn groups_compose_and_nest() {
        let event = context();
        let condition = ConditionNode::All {
            nodes: vec![
                predicate(Field::Extension, Operator::Equals, "mkv"),
                ConditionNode::Any {
                    nodes: vec![
                        predicate(Field::Domain, Operator::Equals, "other.test"),
                        predicate(Field::Domain, Operator::Contains, "example"),
                    ],
                },
                ConditionNode::Not {
                    node: Box::new(predicate(Field::Source, Operator::Equals, "clipboard")),
                },
            ],
        };
        assert!(condition.matches(&event));
        assert_eq!(condition.depth(), 3);
    }

    #[test]
    fn numbers_and_text_never_share_an_operator() {
        // A size compared with `contains`, or a name compared with `>`, is a mistake the
        // author should see while writing it rather than a rule that never fires.
        assert!(
            predicate(Field::SizeBytes, Operator::GreaterThan, "1000")
                .validate()
                .is_ok()
        );
        assert!(
            predicate(Field::SizeBytes, Operator::Contains, "1000")
                .validate()
                .is_err()
        );
        assert!(
            predicate(Field::Name, Operator::GreaterThan, "1000")
                .validate()
                .is_err()
        );
        assert!(
            predicate(Field::SizeBytes, Operator::GreaterThan, "big")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn numeric_comparison_uses_the_number_not_its_spelling() {
        let event = context();
        assert!(predicate(Field::SizeBytes, Operator::GreaterThan, "999").matches(&event));
        assert!(predicate(Field::SizeBytes, Operator::LessThan, "10000").matches(&event));
        // Lexicographically "2000" < "999"; numerically it is not.
        assert!(!predicate(Field::SizeBytes, Operator::LessThan, "999").matches(&event));
    }

    #[test]
    fn an_invalid_regex_is_refused_before_it_is_stored() {
        assert!(
            predicate(Field::Name, Operator::Matches, "S0[12]E")
                .validate()
                .is_ok()
        );
        assert!(
            predicate(Field::Name, Operator::Matches, "S0[12")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn an_empty_group_is_refused_rather_than_guessed_at() {
        // `all` of nothing is true and `any` of nothing is false; either is a rule the
        // author did not mean to write.
        assert!(ConditionNode::All { nodes: Vec::new() }.validate().is_err());
        assert!(ConditionNode::Any { nodes: Vec::new() }.validate().is_err());
    }

    #[test]
    fn a_definition_needs_a_name_and_at_least_one_action() {
        let action = Action::PausePackage;
        assert!(
            validate(
                "Move to TV",
                &ConditionNode::Always,
                slice::from_ref(&action)
            )
            .is_ok()
        );
        assert!(validate("  ", &ConditionNode::Always, slice::from_ref(&action)).is_err());
        assert!(validate("Name", &ConditionNode::Always, &[]).is_err());
        let too_many = vec![action; MAX_ACTIONS + 1];
        assert!(validate("Name", &ConditionNode::Always, &too_many).is_err());
    }

    #[test]
    fn a_deeply_nested_condition_is_refused() {
        let mut node = ConditionNode::Always;
        for _ in 0..10 {
            node = ConditionNode::Not {
                node: Box::new(node),
            };
        }
        assert!(validate("Name", &node, &[Action::PausePackage]).is_err());
    }

    #[test]
    fn a_script_action_can_only_name_a_file_in_the_scripts_directory() {
        for name in ["notify.sh", "post_import.py", "run-me.bat"] {
            assert!(is_script_name(name), "{name}");
        }
        // Traversal, absolute paths, subdirectories and dotfiles are all refused here so an
        // automation is rejected while it is written, not on its first run.
        for name in ["../escape.sh", "/etc/passwd", "sub/dir.sh", ".hidden", ""] {
            assert!(!is_script_name(name), "{name} was accepted");
        }
        assert!(
            validate(
                "Name",
                &ConditionNode::Always,
                &[Action::Script {
                    name: "../escape.sh".to_owned()
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn an_action_cannot_name_a_capability_of_its_own() {
        // The action vocabulary is closed. There is no variant that carries a command line,
        // a path or a secret reference, so there is no spelling of an automation that
        // reaches a credential or a file the core did not already hand it.
        for unknown in [
            r#"{"kind":"command","command":"rm -rf /"}"#,
            r#"{"kind":"webhook","url":"https://attacker.example"}"#,
            r#"{"kind":"script","name":"x.sh","secret_ref":"vault:other"}"#,
            r#"{"kind":"read_secret","reference":"vault:smtp"}"#,
        ] {
            let parsed = serde_json::from_str::<Action>(unknown);
            // A webhook without its target and a script with an extra field are both
            // refused: the first is missing a required field, the second names one that
            // does not exist on the type.
            assert!(
                parsed.is_err() || matches!(parsed, Ok(Action::Script { .. })),
                "{unknown} deserialized into {parsed:?}"
            );
        }
        // What a script action carries is a file name and nothing else.
        let script: Action =
            serde_json::from_str(r#"{"kind":"script","name":"notify.sh"}"#).expect("script action");
        assert_eq!(
            script,
            Action::Script {
                name: "notify.sh".to_owned()
            }
        );
    }

    #[test]
    fn a_time_trigger_needs_a_schedule_and_no_package_action() {
        use super::{Schedule, validate_trigger};
        let now = chrono::Utc::now();
        let hourly = Schedule::Interval { minutes: 60 };
        let start = [Action::StartQueue];
        assert!(
            validate_trigger(Trigger::Schedule, Some(&hourly), &start, now, &chrono::Utc).is_ok()
        );
        let refused = |schedule: Option<&Schedule>, actions: &[Action]| {
            validate_trigger(Trigger::Schedule, schedule, actions, now, &chrono::Utc)
                .map_err(|error| error.code())
        };
        assert_eq!(refused(None, &start), Err("automation.schedule_invalid"));
        let never = Schedule::Cron {
            expression: "0 0 30 2 *".to_owned(),
        };
        assert_eq!(
            refused(Some(&never), &start),
            Err("automation.schedule_invalid")
        );
        // A clock names no package, so an action on "the package" has nothing to act on.
        assert_eq!(
            refused(Some(&hourly), &[Action::ExtractPackage]),
            Err("automation.action_needs_package")
        );
        // Links into the LinkGrabber on its own intake would fire again on every batch.
        let grab = [Action::AddLinks {
            links: vec!["https://example.com/".to_owned()],
            destination: super::LinkDestination::LinkGrabber,
        }];
        assert_eq!(
            validate_trigger(Trigger::IntakeReceived, None, &grab, now, &chrono::Utc)
                .map_err(|error| error.code()),
            Err("automation.links_loop")
        );
        // Every other trigger ignores the schedule question altogether.
        assert!(
            validate_trigger(
                Trigger::PackageCompleted,
                None,
                &[Action::ExtractPackage],
                now,
                &chrono::Utc
            )
            .is_ok()
        );
    }

    #[test]
    fn the_new_actions_are_checked_before_they_are_stored() {
        use super::{LinkDestination, MAX_ACTION_LINKS};
        let target = rd_core::NotificationTargetId::new();
        let code = |action: Action| {
            validate("Name", &ConditionNode::Always, &[action]).map_err(|error| error.code())
        };
        let notify = |message: &str| Action::Notify {
            target_id: target,
            message: message.to_owned(),
        };
        assert!(code(notify("Night queue started")).is_ok());
        assert_eq!(code(notify("  ")), Err("automation.notify_message_invalid"));
        assert_eq!(
            code(notify(&"x".repeat(501))),
            Err("automation.notify_message_invalid")
        );
        let links = |links: &[&str], destination| Action::AddLinks {
            links: links.iter().map(|link| (*link).to_owned()).collect(),
            destination,
        };
        assert!(
            code(links(
                &["https://example.com/a.zip"],
                LinkDestination::Downloads
            ))
            .is_ok()
        );
        assert!(
            code(links(
                &["magnet:?xt=urn:btih:abc"],
                LinkDestination::LinkGrabber
            ))
            .is_ok()
        );
        // The downloads take HTTP(S) directly; anything else goes through the LinkGrabber.
        assert_eq!(
            code(links(
                &["magnet:?xt=urn:btih:abc"],
                LinkDestination::Downloads
            )),
            Err("automation.links_invalid")
        );
        assert_eq!(
            code(links(&["not a link"], LinkDestination::LinkGrabber)),
            Err("automation.links_invalid")
        );
        assert_eq!(
            code(links(&[], LinkDestination::LinkGrabber)),
            Err("automation.links_invalid")
        );
        let many = vec!["https://example.com/"; MAX_ACTION_LINKS + 1];
        assert_eq!(
            code(links(&many, LinkDestination::LinkGrabber)),
            Err("automation.links_invalid")
        );
    }

    #[test]
    fn the_vocabulary_names_every_action_kind() {
        let target = rd_core::NotificationTargetId::new();
        let every = [
            Action::Webhook { target_id: target },
            Action::Script {
                name: "x.sh".to_owned(),
            },
            Action::SetCategory {
                category_id: rd_core::CategoryId::new(),
            },
            Action::PausePackage,
            Action::ResumePackage,
            Action::SetPriority {
                priority: rd_core::DownloadPriority::High,
            },
            Action::PauseQueue,
            Action::StartQueue,
            Action::ExtractPackage,
            Action::Notify {
                target_id: target,
                message: "m".to_owned(),
            },
            Action::AddLinks {
                links: Vec::new(),
                destination: super::LinkDestination::Downloads,
            },
        ];
        let kinds: Vec<&str> = every.iter().map(Action::kind).collect();
        assert_eq!(kinds, Action::KINDS);
        for action in &every {
            // The kind is the wire tag, so an export carries the variant a re-import reads.
            let value = serde_json::to_value(action).expect("serialize");
            assert_eq!(value["kind"], action.kind());
            let back: Action = serde_json::from_value(value).expect("deserialize");
            assert_eq!(&back, action);
        }
    }

    #[test]
    fn every_trigger_is_listed_exactly_once() {
        let all = Trigger::all();
        let mut seen: Vec<String> = all
            .iter()
            .map(|trigger| serde_json::to_string(trigger).expect("serialize"))
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), all.len(), "a trigger is listed twice");
    }
}

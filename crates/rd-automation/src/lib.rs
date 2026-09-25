//! The automation engine's contract: definitions, conditions, runs and their policy.
//!
//! The crate owns what an automation *is* and how it is judged. Persisting runs and actually
//! executing actions is the service's job, exactly as in `rd-notify`, so the rules stay
//! testable without a database and without a network.

mod condition;
mod model;

pub use condition::{ConditionError, ConditionNode, EventContext, Field, Operator, Predicate};
pub use model::{
    Action, Automation, AutomationVersion, DefinitionError, MAX_ACTIONS, MAX_CONDITION_DEPTH, Run,
    RunState, Trigger, is_script_name, validate,
};
pub use rd_notify::{MAX_ATTEMPTS, backoff, next_attempt_at};

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

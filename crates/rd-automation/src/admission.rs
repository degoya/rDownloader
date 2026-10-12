//! Whether an event reaches an automation's condition at all (RD-1270-04).
//!
//! The storage threshold fires when a root runs out of space and again when it has room. A rule
//! written for the first -- "storage low: pause the queue, call the webhook" -- must not run on
//! the second, and no stored rule was written with the second in mind: the event used to carry
//! nothing that told the two apart. So a storage rule whose condition says nothing about the
//! change is a rule about the block, and only a condition that names `change` hears the release
//! (`change equals released`, or both with `any`). Every other trigger goes straight to its
//! condition.

use crate::{
    condition::{ConditionNode, EventContext, Field},
    model::Trigger,
};

/// The change a storage threshold rule hears when its condition names none.
const DEFAULT_STORAGE_CHANGE: &str = "blocked";

/// Whether an automation on `trigger` with `condition` runs for the event `event` describes.
///
/// What the event engine asks; the dry run judges the condition alone against a package and
/// does not come through here.
#[must_use]
pub fn admits(trigger: Trigger, condition: &ConditionNode, event: &EventContext) -> bool {
    if trigger == Trigger::StorageThreshold
        && !condition.mentions(Field::Change)
        && event.text.get(&Field::Change).map(String::as_str) != Some(DEFAULT_STORAGE_CHANGE)
    {
        return false;
    }
    condition.matches(event)
}

#[cfg(test)]
mod tests {
    use super::admits;
    use crate::{ConditionNode, EventContext, Field, Operator, Predicate, Trigger};

    fn storage(change: &str) -> EventContext {
        let mut context = EventContext::default();
        context.set(Field::Change, change);
        context
    }

    fn change_is(value: &str) -> ConditionNode {
        ConditionNode::Predicate {
            predicate: Predicate {
                field: Field::Change,
                operator: Operator::Equals,
                value: value.to_owned(),
            },
        }
    }

    #[test]
    fn a_storage_rule_without_a_word_on_the_change_hears_only_the_block() {
        // The shape every storage rule was stored in before the change was a field.
        let stored: ConditionNode =
            serde_json::from_str(r#"{"type":"always"}"#).expect("a stored condition");
        let trigger = Trigger::StorageThreshold;
        assert!(admits(trigger, &stored, &storage("blocked")));
        assert!(!admits(trigger, &stored, &storage("released")));
        // A condition on something else leaves the default as it is.
        let named = ConditionNode::Not {
            node: Box::new(ConditionNode::Predicate {
                predicate: Predicate {
                    field: Field::Name,
                    operator: Operator::Equals,
                    value: "x".to_owned(),
                },
            }),
        };
        assert!(admits(trigger, &named, &storage("blocked")));
        assert!(!admits(trigger, &named, &storage("released")));
    }

    #[test]
    fn a_condition_on_the_change_chooses_the_release_or_both() {
        let trigger = Trigger::StorageThreshold;
        let released = change_is("released");
        assert!(admits(trigger, &released, &storage("released")));
        assert!(!admits(trigger, &released, &storage("blocked")));
        let negated = ConditionNode::Not {
            node: Box::new(change_is("blocked")),
        };
        assert!(admits(trigger, &negated, &storage("released")));
        assert!(!admits(trigger, &negated, &storage("blocked")));
        let both = ConditionNode::Any {
            nodes: vec![change_is("blocked"), change_is("released")],
        };
        assert!(admits(trigger, &both, &storage("blocked")));
        assert!(admits(trigger, &both, &storage("released")));
    }

    #[test]
    fn every_other_trigger_goes_straight_to_its_condition() {
        let mut completed = EventContext::default();
        completed.set(Field::State, "completed");
        assert!(admits(
            Trigger::PackageCompleted,
            &ConditionNode::Always,
            &completed
        ));
        assert!(!admits(
            Trigger::PackageCompleted,
            &change_is("blocked"),
            &completed
        ));
    }

    #[test]
    fn the_change_is_a_text_field_with_its_own_wire_name() {
        let field: Field = serde_json::from_str(r#""change""#).expect("field");
        assert_eq!(field, Field::Change);
        assert!(!field.is_numeric());
        assert!(change_is("blocked").validate().is_ok());
        assert!(change_is("blocked").mentions(Field::Change));
        assert!(!ConditionNode::Always.mentions(Field::Change));
    }

    #[test]
    fn a_rule_on_one_storage_target_hears_that_target_and_still_the_block_only() {
        let field: Field = serde_json::from_str(r#""storage_target""#).expect("field");
        assert_eq!(field, Field::StorageTarget);
        assert!(!field.is_numeric());
        let media = ConditionNode::Predicate {
            predicate: Predicate {
                field: Field::StorageTarget,
                operator: Operator::Equals,
                value: "0f6c2d2e-root".to_owned(),
            },
        };
        let event = |change: &str, target: &str| {
            let mut context = storage(change);
            context.set(Field::StorageTarget, target);
            context
        };
        let trigger = Trigger::StorageThreshold;
        assert!(admits(trigger, &media, &event("blocked", "0f6c2d2e-root")));
        assert!(!admits(trigger, &media, &event("blocked", "fallback")));
        // The target is not the change: a rule naming only the target keeps the default.
        assert!(!admits(
            trigger,
            &media,
            &event("released", "0f6c2d2e-root")
        ));
    }
}

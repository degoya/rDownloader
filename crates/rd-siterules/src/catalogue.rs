//! Shipped and user rules side by side.
//!
//! The database cannot know which ids the shipped pack uses, and the pack cannot know what
//! the database holds; this is where the two meet. A user rule whose `id` a shipped rule
//! already carries is refused here, never silently placed over it — the person picks another
//! id, and the selection (RD-110-06) decides between the two rules on their own merits.

use crate::format::Rule;

/// Every rule this installation knows, shipped first.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    shipped: Vec<Rule>,
    user: Vec<Rule>,
}

/// Why a user rule was not admitted.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CatalogueError {
    #[error("rule id {0:?} belongs to a shipped rule")]
    IdTaken(String),
    #[error("rule id {0:?} is already used by another user rule")]
    DuplicateId(String),
}

impl CatalogueError {
    /// The stable code, translated by the interface.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::IdTaken(_) => "site_rules.id_taken",
            Self::DuplicateId(_) => "site_rules.duplicate_id",
        }
    }
}

impl Catalogue {
    /// Starts from the rules a verified pack delivered.
    #[must_use]
    pub fn new(shipped: Vec<Rule>) -> Self {
        Self {
            shipped,
            user: Vec::new(),
        }
    }

    /// Admits a user rule, or says why not. Replacing a user rule is a removal and an
    /// admission, so an edit cannot slip past the id check.
    pub fn add_user_rule(&mut self, rule: Rule) -> Result<(), CatalogueError> {
        if self.shipped.iter().any(|shipped| shipped.id == rule.id) {
            return Err(CatalogueError::IdTaken(rule.id));
        }
        if self.user.iter().any(|user| user.id == rule.id) {
            return Err(CatalogueError::DuplicateId(rule.id));
        }
        self.user.push(rule);
        Ok(())
    }

    /// Removes a user rule; returns whether one was there. Shipped rules cannot be removed.
    pub fn remove_user_rule(&mut self, id: &str) -> bool {
        let before = self.user.len();
        self.user.retain(|rule| rule.id != id);
        self.user.len() != before
    }

    /// The rules the pack delivered.
    #[must_use]
    pub fn shipped(&self) -> &[Rule] {
        &self.shipped
    }

    /// The rules the person wrote.
    #[must_use]
    pub fn user(&self) -> &[Rule] {
        &self.user
    }

    /// Every rule, shipped first.
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.shipped.iter().chain(self.user.iter())
    }

    /// One rule by id, from either side.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules().find(|rule| rule.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::tests::example;

    fn user(id: &str) -> Rule {
        let mut rule = example();
        rule.id = id.to_owned();
        rule
    }

    #[test]
    fn a_user_rule_never_takes_a_shipped_id() {
        let mut catalogue = Catalogue::new(vec![example()]);
        let refused = catalogue
            .add_user_rule(user("scnlog"))
            .expect_err("refused");
        assert_eq!(refused.code(), "site_rules.id_taken");
        assert_eq!(catalogue.user().len(), 0);
        assert_eq!(catalogue.get("scnlog").map(|rule| rule.version), Some(1));
    }

    #[test]
    fn a_user_rule_with_its_own_id_sits_beside_the_shipped_one() {
        let mut catalogue = Catalogue::new(vec![example()]);
        catalogue
            .add_user_rule(user("my-scnlog"))
            .expect("admitted");
        let ids: Vec<_> = catalogue.rules().map(|rule| rule.id.as_str()).collect();
        assert_eq!(ids, ["scnlog", "my-scnlog"]);
    }

    #[test]
    fn two_user_rules_cannot_share_an_id() {
        let mut catalogue = Catalogue::default();
        catalogue.add_user_rule(user("mine")).expect("admitted");
        let refused = catalogue.add_user_rule(user("mine")).expect_err("refused");
        assert_eq!(refused.code(), "site_rules.duplicate_id");
        assert!(catalogue.remove_user_rule("mine"));
        assert!(!catalogue.remove_user_rule("mine"));
        catalogue
            .add_user_rule(user("mine"))
            .expect("admitted again");
    }
}

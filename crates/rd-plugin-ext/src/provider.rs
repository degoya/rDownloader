//! Why a call into a provider plugin produced no answer (RD-106-02).
//!
//! Two things go wrong here, and they deserve opposite treatment. A provider that no installed
//! plugin claims will not be claimed by waiting: nothing about the next attempt differs from
//! this one, so repeating it is a warning line every five minutes and never a sign-in. A call
//! that was made and did not come back says the opposite -- it says nothing at all about the
//! credential, and giving up on it throws away a token that is very probably still good.
//!
//! Both used to arrive as one opaque `anyhow::Error`, so the only way to tell them apart at
//! the call site was to compare the message text. This type is that distinction, made by the
//! compiler instead.

use std::fmt;

/// What kept a provider call from producing an answer.
#[derive(Debug)]
pub enum ProviderError {
    /// No installed plugin claims this provider slug. Terminal: only an installation changes it.
    NoPlugin {
        /// The slug the account carries, which nothing claimed.
        provider_slug: String,
    },
    /// A plugin claims the provider but does not serve the way in that was asked for
    /// (RD-106-01). Terminal for the same reason as `NoPlugin`: the manifest says what it
    /// serves, and asking again reads the same manifest.
    UnsupportedFlow {
        /// The slug the account carries.
        provider_slug: String,
        /// The way in nobody serves, as the manifest spells it -- `redirect` or `device`.
        flow: &'static str,
    },
    /// A plugin claimed the provider, ran, and the call did not produce an answer. Says
    /// nothing about whether the same call would work in a minute.
    Failed(anyhow::Error),
}

/// What every provider lookup and provider call reports.
pub type ProviderResult<T> = Result<T, ProviderError>;

impl ProviderError {
    /// Whether nothing installed claims the provider, as opposed to a call that failed.
    #[must_use]
    pub const fn is_missing_plugin(&self) -> bool {
        matches!(self, Self::NoPlugin { .. })
    }

    /// Whether waiting can change the answer. Both of the first two variants are decided by
    /// what is installed, so repeating the call repeats the refusal.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::NoPlugin { .. } | Self::UnsupportedFlow { .. })
    }

    pub(crate) fn no_plugin(provider_slug: &str) -> Self {
        Self::NoPlugin {
            provider_slug: provider_slug.to_owned(),
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Only the slug the account already carries. A provider that was never reached
            // said nothing that could be quoted here in the first place.
            Self::NoPlugin { provider_slug } => {
                write!(formatter, "no installed plugin claims {provider_slug}")
            }
            // The slug and one of two literals this build wrote itself. Nothing foreign.
            Self::UnsupportedFlow {
                provider_slug,
                flow,
            } => write!(
                formatter,
                "the plugin for {provider_slug} does not offer the {flow} sign-in"
            ),
            Self::Failed(error) => write!(formatter, "{error:#}"),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<anyhow::Error> for ProviderError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderError;

    #[test]
    fn a_missing_plugin_is_told_apart_from_a_call_that_failed() {
        let missing = ProviderError::no_plugin("example");
        assert!(missing.is_missing_plugin());
        assert!(!ProviderError::from(anyhow::anyhow!("connection reset")).is_missing_plugin());
    }

    /// A way in nobody serves is as final as a plugin nobody installed, and as harmless to
    /// quote: neither wording carries a word a provider wrote.
    #[test]
    fn an_unserved_way_in_is_terminal_and_names_only_what_this_build_knows() {
        let unsupported = ProviderError::UnsupportedFlow {
            provider_slug: "example".to_owned(),
            flow: "device",
        };
        assert!(unsupported.is_terminal());
        assert!(!unsupported.is_missing_plugin());
        assert!(ProviderError::no_plugin("example").is_terminal());
        assert!(!ProviderError::from(anyhow::anyhow!("connection reset")).is_terminal());
        assert_eq!(
            unsupported.to_string(),
            "the plugin for example does not offer the device sign-in"
        );
    }

    #[test]
    fn the_missing_plugin_wording_names_the_provider_and_nothing_else() {
        let missing = ProviderError::no_plugin("example");
        assert_eq!(missing.to_string(), "no installed plugin claims example");
    }

    #[test]
    fn a_failed_call_keeps_the_whole_chain_in_its_wording() {
        let error = ProviderError::from(
            anyhow::anyhow!("connection reset").context("ask the provider for a token"),
        );
        assert_eq!(
            error.to_string(),
            "ask the provider for a token: connection reset"
        );
    }
}

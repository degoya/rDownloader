//! What a pool answers when an article did not arrive (RD-108-29).

/// Why an article did not arrive - and whether that says anything about the article.
///
/// The whole point of the type (RD-108-29). Only [`Self::Unavailable`] is a statement about
/// the article, and only it may cost the caller the bytes; [`Self::ServerFault`] means the
/// question was never answered and has to be asked again later.
#[derive(Debug)]
pub enum FetchError {
    /// Every server said it does not have this article.
    Unavailable(String),
    /// At least one server could not answer: a transient status, a connection that broke, a
    /// body that did not decode - each of them already retried.
    ServerFault(String),
}

impl FetchError {
    /// The detail worth logging or putting into a failure message.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Unavailable(detail) | Self::ServerFault(detail) => detail,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(formatter, "{detail}"),
            Self::ServerFault(detail) => write!(formatter, "{detail}"),
        }
    }
}

impl std::error::Error for FetchError {}

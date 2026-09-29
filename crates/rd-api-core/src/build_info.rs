//! The commit and build time the binary hands over for the About page (RD-130-12).

/// Commit and build time of the running binary.
///
/// Handed in by the binary rather than read here, because the build script that knows them
/// belongs to the binary crate. A test router never sets them and answers with neither.
#[derive(Clone, Debug, Default)]
pub struct BuildInfo {
    commit: Option<String>,
    built: Option<String>,
}

impl BuildInfo {
    /// `unknown` is what VERSION.txt and the build script write without git; it is no value.
    #[must_use]
    pub fn new(commit: &str, built: &str) -> Self {
        let known = |value: &str| {
            let value = value.trim();
            (!value.is_empty() && value != "unknown").then(|| value.to_owned())
        };
        Self {
            commit: known(commit),
            built: known(built),
        }
    }

    /// The commit the binary was built from, when it knows one.
    #[must_use]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// When the binary was built, when it knows.
    #[must_use]
    pub fn built(&self) -> Option<&str> {
        self.built.as_deref()
    }
}

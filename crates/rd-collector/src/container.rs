//! The container formats the LinkGrabber accepts, and how a file name maps to one.
//!
//! Kept in one place because three things dispatch on it — the REST endpoint, the hotfolder
//! and the web interface's file filter — and they used to each carry their own list. A `.dlc`
//! that the picker offered and the filter dropped is what that costs.

/// A container format, identified by the file's extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerFormat {
    /// JDownloader's encrypted container. Needs the online decryption service.
    Dlc,
    /// CryptLoad's container. Unwrapped by the same service, which answers with a DLC.
    Ccf,
    /// RSDF, decrypted locally.
    Rsdf,
    /// A plain text link list.
    Text,
}

impl ContainerFormat {
    /// The format a file name announces, if it announces one.
    #[must_use]
    pub fn from_file_name(name: &str) -> Option<Self> {
        let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
        match extension.as_str() {
            "dlc" => Some(Self::Dlc),
            "ccf" => Some(Self::Ccf),
            "rsdf" => Some(Self::Rsdf),
            "txt" | "text" => Some(Self::Text),
            _ => None,
        }
    }

    /// Whether unlocking this format needs the online decryption service.
    #[must_use]
    pub const fn needs_service(self) -> bool {
        matches!(self, Self::Dlc | Self::Ccf)
    }

    /// What the service is asked to convert from, for the formats that need it.
    #[must_use]
    pub const fn service_source(self) -> &'static str {
        match self {
            Self::Ccf => "ccf",
            _ => "dlc",
        }
    }

    /// The stable name used in responses and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dlc => "dlc",
            Self::Ccf => "ccf",
            Self::Rsdf => "rsdf",
            Self::Text => "text",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContainerFormat;

    #[test]
    fn an_extension_names_its_format_whatever_its_case() {
        assert_eq!(
            ContainerFormat::from_file_name("Season.DLC"),
            Some(ContainerFormat::Dlc)
        );
        assert_eq!(
            ContainerFormat::from_file_name("links.txt"),
            Some(ContainerFormat::Text)
        );
        assert_eq!(
            ContainerFormat::from_file_name("archive.rsdf"),
            Some(ContainerFormat::Rsdf)
        );
        assert_eq!(
            ContainerFormat::from_file_name("bundle.ccf"),
            Some(ContainerFormat::Ccf)
        );
    }

    #[test]
    fn anything_else_is_not_a_container() {
        assert_eq!(ContainerFormat::from_file_name("notes.md"), None);
        assert_eq!(ContainerFormat::from_file_name("no-extension"), None);
        assert_eq!(ContainerFormat::from_file_name("release.nzb"), None);
    }

    #[test]
    fn only_the_two_encrypted_formats_leave_the_machine() {
        assert!(ContainerFormat::Dlc.needs_service());
        assert!(ContainerFormat::Ccf.needs_service());
        assert!(!ContainerFormat::Rsdf.needs_service());
        assert!(!ContainerFormat::Text.needs_service());
        // CCF is unwrapped by asking the same service to read it as a CCF.
        assert_eq!(ContainerFormat::Ccf.service_source(), "ccf");
        assert_eq!(ContainerFormat::Dlc.service_source(), "dlc");
    }
}

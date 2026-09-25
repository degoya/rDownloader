//! Reading a `.crawljob` file.
//!
//! This is what browser extensions and automation tools drop into a folder for JDownloader:
//! `key=value` lines, blank-line-separated blocks, one block per package. Only the keys that
//! mean something to an intake parser are read — `text`, `packageName` and `filename`. The
//! rest (`autoStart`, `downloadFolder`, `extractAfterDownload`, …) describe what the *other*
//! application should do afterwards, and honouring them here would let a dropped file decide
//! where this one writes.

/// One block of a crawljob: the links it carries and the package they belong to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedJob {
    pub urls: Vec<String>,
    pub package_name: Option<String>,
    /// `filename=` applies to a single-link job; with several links it says nothing useful,
    /// so it is only kept when there is exactly one.
    pub file_name: Option<String>,
}

/// Keys that mark a file as a crawljob rather than an arbitrary properties file.
const SIGNATURE_KEYS: &[&str] = &[
    "packagename=",
    "autostart=",
    "downloadfolder=",
    "extractafterdownload=",
    "autoconfirm=",
];

/// Whether this text looks like a crawljob.
///
/// `text=` alone is not enough: plenty of pasted text contains it. A crawljob is recognised
/// by `text=` together with at least one key only this format uses, so an ordinary paste is
/// left to the built-in scanner.
#[must_use]
pub fn claims(input: &str) -> bool {
    let lowered = input.to_lowercase();
    let has_text = lowered
        .lines()
        .any(|line| line.trim_start().starts_with("text="));
    has_text
        && SIGNATURE_KEYS.iter().any(|key| {
            lowered
                .lines()
                .any(|line| line.trim_start().starts_with(key))
        })
}

/// Every block the file carries, in order. A block with no usable link is dropped.
#[must_use]
pub fn jobs_in(input: &str) -> Vec<ParsedJob> {
    let mut jobs = Vec::new();
    let mut current = ParsedJob::default();
    let mut single_name = None;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            finish(&mut jobs, std::mem::take(&mut current), single_name.take());
            continue;
        }
        // A comment, or a line that is not a key at all.
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_lowercase().as_str() {
            "text" => current.urls.extend(links_in(value)),
            "packagename" if !value.is_empty() => current.package_name = Some(value.to_owned()),
            "filename" if !value.is_empty() => single_name = Some(value.to_owned()),
            _ => {}
        }
    }
    finish(&mut jobs, current, single_name);
    jobs
}

fn finish(jobs: &mut Vec<ParsedJob>, mut job: ParsedJob, file_name: Option<String>) {
    if job.urls.is_empty() {
        return;
    }
    if job.urls.len() == 1 {
        job.file_name = file_name;
    }
    jobs.push(job);
}

/// The http(s) links in one `text=` value.
///
/// JDownloader writes several links separated by a literal `\n`, by spaces or by commas
/// depending on what produced the file, so all three are accepted. Anything that is not an
/// http(s) address is dropped rather than proposed: the LinkGrabber would only have to
/// refuse it later, with less to say about why.
fn links_in(value: &str) -> Vec<String> {
    value
        .replace("\\n", " ")
        .replace("\\r", " ")
        .split([' ', '\t', ',', ';', '\n', '\r'])
        .map(str::trim)
        .filter(|token| token.starts_with("http://") || token.starts_with("https://"))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ParsedJob, claims, jobs_in};

    const JOB: &str = "text=https://example.com/one.bin\npackageName=Holiday\nautoStart=TRUE\n";

    #[test]
    fn a_single_block_yields_its_link_and_package() {
        assert!(claims(JOB));
        assert_eq!(
            jobs_in(JOB),
            vec![ParsedJob {
                urls: vec!["https://example.com/one.bin".to_owned()],
                package_name: Some("Holiday".to_owned()),
                file_name: None,
            }]
        );
    }

    #[test]
    fn blocks_are_separated_by_blank_lines() {
        const TWO: &str = "text=https://example.com/a.bin\npackageName=First\nautoStart=TRUE\n\n\
                           text=https://example.com/b.bin\npackageName=Second\nautoStart=TRUE\n";
        let jobs = jobs_in(TWO);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].package_name.as_deref(), Some("First"));
        assert_eq!(jobs[1].urls, vec!["https://example.com/b.bin"]);
    }

    #[test]
    fn several_links_in_one_text_value_are_all_read() {
        const MANY: &str =
            "text=https://example.com/a.bin\\nhttps://example.com/b.bin\npackageName=Set\n";
        let jobs = jobs_in(MANY);
        assert_eq!(jobs[0].urls.len(), 2);
        // A name for one file says nothing about two, so it is not carried over.
        assert_eq!(jobs[0].file_name, None);
    }

    #[test]
    fn a_file_name_applies_only_to_a_single_link() {
        const ONE: &str = "text=https://example.com/a.bin\nfilename=holiday.bin\nautoStart=TRUE\n";
        assert_eq!(jobs_in(ONE)[0].file_name.as_deref(), Some("holiday.bin"));
        const TWO: &str = "text=https://example.com/a.bin https://example.com/b.bin\n\
                           filename=holiday.bin\nautoStart=TRUE\n";
        assert_eq!(jobs_in(TWO)[0].file_name, None);
    }

    #[test]
    fn keys_that_would_direct_this_application_are_ignored() {
        // A dropped file must not be able to say where this installation writes or what it
        // runs afterwards. Those keys belong to the application the file was written for.
        const DIRECTIVE: &str = "text=https://example.com/a.bin\ndownloadFolder=/etc\n\
                                 extractAfterDownload=TRUE\nautoStart=TRUE\n";
        let jobs = jobs_in(DIRECTIVE);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].package_name, None);
    }

    #[test]
    fn a_plain_paste_is_not_claimed() {
        // `text=` on its own turns up in ordinary text; claiming on it alone would hand this
        // parser every paste in the application.
        assert!(!claims("text=https://example.com/one.bin"));
        assert!(!claims("https://example.com/one.bin"));
    }

    #[test]
    fn a_block_without_a_usable_link_is_dropped() {
        const EMPTY: &str = "packageName=Nothing\nautoStart=TRUE\ntext=ftp://example.com/a.bin\n";
        assert!(jobs_in(EMPTY).is_empty());
    }
}

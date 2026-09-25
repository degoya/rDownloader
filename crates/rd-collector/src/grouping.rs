//! JDownloader-style package grouping for LinkGrabber submissions.

use rd_files::parse_archive_volume;

/// One link to group (index into the caller's list).
#[derive(Clone, Debug)]
pub struct GroupInput<'a> {
    pub index: usize,
    pub file_name: Option<&'a str>,
    pub host: &'a str,
    /// A container that expands into a release of its own — an NZB or a torrent whose name
    /// the source stated. It never shares a package with the links submitted beside it.
    pub standalone: bool,
    /// The package this link's source says it belongs to — the folder a crawler found it in
    /// (RD-104-03). Links sharing a hint form one package, in the order they arrived.
    ///
    /// A suggestion and not a decision: an explicit package name still overrides it, exactly
    /// as it overrides every other rule below. Without this the structure a crawler walked
    /// would be flattened into one package named after the first host, and a folder with
    /// three seasons in it would arrive as one heap.
    pub package_hint: Option<&'a str>,
}

/// A proposed package with its member indices in submission order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Group {
    pub name: String,
    pub members: Vec<usize>,
    /// Whether the name is the source's own word rather than something inferred here
    /// (RD-120-17).
    ///
    /// True for an explicit package name and for a `package_hint`: a site rule that read the
    /// release title off the page, or a crawler that read the folder, *stated* the name. False
    /// for the archive stem, the common prefix and the host fallback, which are guesses made
    /// from whatever file names happened to be known at the time.
    ///
    /// The caller needs the distinction because a guess may be made again later and a
    /// statement may not: the online check regroups a batch once the real file names arrive,
    /// and re-deriving a stated name there replaced it with the common stem -- or, when the
    /// package held a link whose service has no resolver and therefore no file name at all,
    /// with that link's host.
    pub named_by_source: bool,
}

/// Groups links into packages: an explicit package name (e.g. from Click'n'Load) keeps the
/// whole submission together; otherwise a standalone container takes a package of its own,
/// multipart archives form one package per base name, and the remaining links share one
/// package named after their common stem or the first host.
#[must_use]
pub fn group_links(
    inputs: &[GroupInput<'_>],
    package_name: Option<&str>,
    fallback: &str,
) -> Vec<Group> {
    if inputs.is_empty() {
        return Vec::new();
    }
    if let Some(name) = package_name.map(str::trim).filter(|name| !name.is_empty()) {
        return vec![Group {
            name: name.to_owned(),
            members: inputs.iter().map(|input| input.index).collect(),
            named_by_source: true,
        }];
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut loose: Vec<&GroupInput<'_>> = Vec::new();
    for input in inputs {
        // A source that already knows where a link belongs is believed before anything is
        // guessed from the file name: the folder a crawler read it out of is a fact, and the
        // archive stem and the common prefix underneath are inferences.
        if let Some(hint) = input
            .package_hint
            .map(str::trim)
            .filter(|hint| !hint.is_empty())
        {
            if let Some(group) = groups.iter_mut().find(|group| group.name == hint) {
                group.members.push(input.index);
            } else {
                groups.push(Group {
                    name: hint.to_owned(),
                    members: vec![input.index],
                    named_by_source: true,
                });
            }
            continue;
        }
        // An NZB or a torrent is one release. Grouped with the others it would inherit a
        // package named after the indexer's host, and a poll that accepted five hits would
        // put five unrelated releases into one package.
        if let Some(name) = input.standalone.then_some(input.file_name).flatten() {
            groups.push(Group {
                name: container_name(name),
                members: vec![input.index],
                named_by_source: false,
            });
            continue;
        }
        let base = input
            .file_name
            .and_then(parse_archive_volume)
            .map(|volume| volume.base);
        match base {
            Some(base) => {
                if let Some(group) = groups
                    .iter_mut()
                    .find(|group| group.name.eq_ignore_ascii_case(&base))
                {
                    group.members.push(input.index);
                } else {
                    groups.push(Group {
                        name: base,
                        members: vec![input.index],
                        named_by_source: false,
                    });
                }
            }
            None => loose.push(input),
        }
    }
    if !loose.is_empty() {
        let names: Vec<&str> = loose.iter().filter_map(|input| input.file_name).collect();
        let name = common_stem(&names)
            .unwrap_or_else(|| loose[0].host.trim_start_matches("www.").to_owned())
            .trim()
            .to_owned();
        let name = if name.is_empty() {
            fallback.to_owned()
        } else {
            name
        };
        groups.push(Group {
            name,
            members: loose.iter().map(|input| input.index).collect(),
            named_by_source: false,
        });
    }
    groups
}

/// The release name behind a container's file name: `Show.S01E01.nzb` is the package
/// `Show.S01E01`, and a name that is nothing but the extension keeps it.
fn container_name(file_name: &str) -> String {
    let trimmed = file_name.trim();
    let stem = trimmed
        .strip_suffix(".nzb")
        .or_else(|| trimmed.strip_suffix(".torrent"))
        .unwrap_or(trimmed)
        .trim();
    if stem.is_empty() {
        trimmed.to_owned()
    } else {
        stem.to_owned()
    }
}

/// Longest common prefix of the names cut back to a separator (`.`, `_`, `-`, space);
/// `None` when shorter than three characters or when only one name is given.
#[must_use]
pub fn common_stem(names: &[&str]) -> Option<String> {
    if names.len() < 2 {
        return None;
    }
    let first = names[0];
    let mut prefix_len = first.len();
    for name in &names[1..] {
        let common = first
            .chars()
            .zip(name.chars())
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        prefix_len = prefix_len.min(common);
    }
    let prefix = &first[..prefix_len];
    let cut = prefix.rfind(['.', '_', '-', ' ']).unwrap_or(prefix.len());
    let stem = prefix[..cut].trim_end_matches(['.', '_', '-', ' ']);
    (stem.chars().count() >= 3).then(|| stem.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{GroupInput, common_stem, group_links};

    fn input<'a>(index: usize, file_name: Option<&'a str>, host: &'a str) -> GroupInput<'a> {
        GroupInput {
            index,
            file_name,
            host,
            standalone: false,
            package_hint: None,
        }
    }

    fn container<'a>(index: usize, file_name: &'a str, host: &'a str) -> GroupInput<'a> {
        GroupInput {
            index,
            file_name: Some(file_name),
            host,
            standalone: true,
            package_hint: None,
        }
    }

    fn crawled<'a>(index: usize, file_name: &'a str, hint: &'a str) -> GroupInput<'a> {
        GroupInput {
            index,
            file_name: Some(file_name),
            host: "8.premiumize.me",
            standalone: false,
            package_hint: Some(hint),
        }
    }

    /// The folder a crawler read a file out of becomes its package (RD-104-03).
    ///
    /// Without this a folder with two seasons in it would arrive as one package named after
    /// the delivery host, because that is what every file behind a cloud folder shares.
    #[test]
    fn a_crawled_folder_keeps_its_structure() {
        let inputs = vec![
            crawled(0, "e01.mkv", "Show/Season 1"),
            crawled(1, "e02.mkv", "Show/Season 1"),
            crawled(2, "e01.mkv", "Show/Season 2"),
            crawled(3, "readme.txt", "Show"),
        ];
        let groups = group_links(&inputs, None, "Links");
        assert_eq!(
            groups
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Show/Season 1", "Show/Season 2", "Show"]
        );
        assert_eq!(groups[0].members, vec![0, 1]);
        assert_eq!(groups[2].members, vec![3]);
    }

    /// RD-110-06: what a site rule read as the package name arrives here as the same
    /// `package_hint` a crawler plugin's answer carries — one mechanic, not two — so the
    /// links of one release page form one package under the name the page gave them
    /// instead of one named after whichever hoster happens to deliver them.
    #[test]
    fn the_package_name_a_rule_read_names_the_package() {
        let inputs = vec![
            crawled(0, "a.part1.rar", "Some.Release.2026.1080p"),
            crawled(1, "b.part2.rar", "Some.Release.2026.1080p"),
        ];
        let groups = group_links(&inputs, None, "Links");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Some.Release.2026.1080p");
        assert_eq!(groups[0].members, vec![0, 1]);
    }

    /// A hint is a suggestion; an explicit package name is not.
    #[test]
    fn an_explicit_package_name_still_overrides_every_hint() {
        let inputs = vec![
            crawled(0, "e01.mkv", "Show/Season 1"),
            crawled(1, "e01.mkv", "Show/Season 2"),
        ];
        let groups = group_links(&inputs, Some("Mine"), "Links");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Mine");
    }

    /// An empty hint is no hint, and falls back to the rules underneath it.
    #[test]
    fn a_blank_hint_does_not_make_a_package_with_no_name() {
        let inputs = vec![
            GroupInput {
                index: 0,
                file_name: Some("a.part1.rar"),
                host: "h.example",
                standalone: false,
                package_hint: Some("  "),
            },
            GroupInput {
                index: 1,
                file_name: Some("a.part2.rar"),
                host: "h.example",
                standalone: false,
                package_hint: None,
            },
        ];
        let groups = group_links(&inputs, None, "Links");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "a");
    }

    /// Every hit of an indexer poll is a release of its own.
    ///
    /// They arrive from one address that says nothing about them — `api.indexer.org/api` —
    /// so grouped by host they would land in one package named after the indexer.
    #[test]
    fn each_container_takes_a_package_named_after_its_release() {
        let groups = group_links(
            &[
                container(0, "Show.S04E05.1080p.WEB-DL-GRP.nzb", "api.indexer.org"),
                container(1, "Other.Show.S01E02.720p.WEB-DL-GRP", "api.indexer.org"),
            ],
            None,
            "links",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Show.S04E05.1080p.WEB-DL-GRP");
        assert_eq!(groups[0].members, [0]);
        assert_eq!(groups[1].name, "Other.Show.S01E02.720p.WEB-DL-GRP");
        assert_eq!(groups[1].members, [1]);
    }

    /// An explicit package name is the caller's decision and still wins.
    #[test]
    fn an_explicit_package_name_keeps_containers_together() {
        let groups = group_links(
            &[
                container(0, "One.nzb", "api.indexer.org"),
                container(1, "Two.nzb", "api.indexer.org"),
            ],
            Some("Both"),
            "links",
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Both");
        assert_eq!(groups[0].members, [0, 1]);
    }

    #[test]
    fn multipart_archives_form_one_package_and_loose_links_share_the_stem() {
        let groups = group_links(
            &[
                input(0, Some("Game.part1.rar"), "ddownload.com"),
                input(1, Some("Game.part2.rar"), "ddownload.com"),
                input(2, Some("Other.Show.S01E01.mkv"), "1fichier.com"),
                input(3, Some("Other.Show.S01E02.mkv"), "1fichier.com"),
            ],
            None,
            "links",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Game");
        assert_eq!(groups[0].members, [0, 1]);
        assert_eq!(groups[1].name, "Other.Show");
        assert_eq!(groups[1].members, [2, 3]);
    }

    #[test]
    fn explicit_package_name_keeps_everything_together() {
        let groups = group_links(
            &[
                input(0, Some("a.part1.rar"), "h"),
                input(1, Some("b.mkv"), "h"),
            ],
            Some(" CNL Package "),
            "links",
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "CNL Package");
        assert_eq!(groups[0].members, [0, 1]);
    }

    /// A stated name and a guessed one are told apart (RD-120-17).
    ///
    /// The caller writes `auto_named` from this, and `auto_named` is what decides whether the
    /// regroup after the online check may derive the name again. Before this flag existed the
    /// two were indistinguishable at the call site, so the release title a site rule had read
    /// off the page was re-derived out of file names and lost.
    #[test]
    fn a_name_the_source_stated_is_marked_as_such() {
        let stated = group_links(
            &[
                crawled(0, "a.rar", "The Economist USA 09.19.2026"),
                crawled(1, "b.rar", "The Economist USA 09.19.2026"),
            ],
            None,
            "links",
        );
        assert_eq!(stated.len(), 1);
        assert_eq!(stated[0].name, "The Economist USA 09.19.2026");
        assert!(stated[0].named_by_source, "the rule read this off the page");

        let explicit = group_links(&[input(0, Some("a.mkv"), "h")], Some("CNL"), "links");
        assert!(explicit[0].named_by_source, "the request said so");

        for guessed in [
            group_links(
                &[
                    input(0, Some("Movie.part1.rar"), "h"),
                    input(1, Some("Movie.part2.rar"), "h"),
                ],
                None,
                "links",
            ),
            group_links(&[input(0, None, "www.1fichier.com")], None, "links"),
        ] {
            assert!(
                !guessed[0].named_by_source,
                "{} was inferred here, not stated",
                guessed[0].name
            );
        }
    }

    #[test]
    fn single_loose_link_uses_the_host() {
        let groups = group_links(&[input(0, None, "www.1fichier.com")], None, "links");
        assert_eq!(groups[0].name, "1fichier.com");
        assert!(common_stem(&["only.one"]).is_none());
        assert_eq!(
            common_stem(&["abc-1.zip", "abc-2.zip"]).as_deref(),
            Some("abc")
        );
        assert!(common_stem(&["ab1", "ab2"]).is_none());
    }
}

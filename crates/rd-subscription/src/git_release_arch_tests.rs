//! The architectures beyond x86 and ARM (RD-1430-07), against the GitLab release list recorded
//! from `gitlab-org/cli`, which ships `s390x` and `ppc64le` builds besides the usual four.

use std::collections::BTreeSet;

use rd_core::{GitArchitecture, GitForge, GitPlatform, GitReleaseOptions};

use super::{GITLAB, architecture_of, checksum_file, parse_releases, selects};

/// Every file name of every release in the recorded GitLab list.
fn gitlab_files() -> Vec<String> {
    parse_releases(GitForge::Gitlab, GITLAB, "gitlab-org/cli")
        .expect("parse")
        .into_iter()
        .flat_map(|release| release.assets)
        .map(|asset| asset.name)
        .collect()
}

#[test]
fn every_spelling_of_the_new_architectures_is_recognised() {
    for (name, expected) in [
        ("tool_linux_s390x.tar.gz", GitArchitecture::S390x),
        ("tool-linux-ppc64le.deb", GitArchitecture::Ppc64le),
        ("tool_1.0_ppc64el.deb", GitArchitecture::Ppc64le),
        (
            "tool-powerpc64le-unknown-linux-gnu.tar.gz",
            GitArchitecture::Ppc64le,
        ),
        ("tool-riscv64-linux.tar.gz", GitArchitecture::Riscv64),
        (
            "tool-riscv64gc-unknown-linux-gnu.tar.xz",
            GitArchitecture::Riscv64,
        ),
    ] {
        assert_eq!(architecture_of(name), Some(expected), "{name}");
    }
    // Big-endian POWER is none of them, and no new spelling is read as a shorter one.
    assert_eq!(architecture_of("tool-linux-ppc64.tar.gz"), None);
    assert_eq!(architecture_of("tool-linux-ppc.tar.gz"), None);
    assert_eq!(architecture_of("tool-s390.tar.gz"), None);
}

#[test]
fn every_build_in_the_gitlab_list_names_its_architecture() {
    let files = gitlab_files();
    let without: BTreeSet<&str> = files
        .iter()
        .filter(|name| architecture_of(&name.to_ascii_lowercase()).is_none())
        .map(String::as_str)
        .collect();
    // Only the checksum lists and the source tarballs name none — neither is a build.
    assert!(!without.is_empty());
    for name in &without {
        assert!(
            checksum_file(name).is_some() || name.ends_with("_source.tar.gz"),
            "{name} names no architecture"
        );
    }
    for (spelling, expected) in [
        ("s390x", GitArchitecture::S390x),
        ("ppc64le", GitArchitecture::Ppc64le),
    ] {
        let builds: Vec<&String> = files
            .iter()
            .filter(|name| name.contains(spelling))
            .collect();
        assert_eq!(
            builds.len(),
            12,
            "four packages of {spelling} in each of three releases"
        );
        for name in builds {
            assert_eq!(architecture_of(name), Some(expected), "{name}");
        }
    }
}

#[test]
fn an_x86_64_filter_takes_none_of_the_other_architectures_builds() {
    let files = gitlab_files();
    let x64 = GitReleaseOptions {
        architectures: vec![GitArchitecture::X86_64],
        ..GitReleaseOptions::default()
    };
    for name in &files {
        let lower = name.to_ascii_lowercase();
        if ["s390x", "ppc64le", "arm64", "armv6", "_386"]
            .iter()
            .any(|other| lower.contains(other))
        {
            assert!(!selects(&x64, name), "{name} taken by an x86_64 filter");
        }
    }

    let linux_x64 = GitReleaseOptions {
        platforms: vec![GitPlatform::Linux],
        ..x64
    };
    let taken: Vec<&String> = files
        .iter()
        .filter(|name| selects(&linux_x64, name))
        .collect();
    assert_eq!(
        taken.len(),
        12,
        "four Linux packages in each of three releases"
    );
    assert!(
        taken.iter().all(|name| name.contains("_linux_amd64.")),
        "{taken:?}"
    );
}

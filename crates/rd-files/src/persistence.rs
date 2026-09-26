//! Does a path survive the container it was configured in?
//!
//! A storage root pointing at a path that is not on a mounted volume is created inside the
//! container's writable layer. Downloads run normally and everything is gone on the next
//! `docker rm`. Nothing in the configuration says so, so the service has to work it out from
//! the mount table.
//!
//! Deliberately biased towards silence: only a *known* overlay filesystem carrying the root
//! counts as ephemeral. Docker with a btrfs or zfs storage driver, rootless Podman and some
//! Kubernetes runtimes give `/` an ordinary filesystem type, and a warning that fires on a
//! bare-metal btrfs install would be worse than one that misses an exotic driver.

use std::path::{Path, PathBuf};

/// Filesystem types that mean "this is the container's own writable layer".
const OVERLAY_TYPES: [&str; 4] = ["overlay", "overlay2", "aufs", "fuse-overlayfs"];

/// Filesystem types that live in memory and are gone after a restart.
const VOLATILE_TYPES: [&str; 2] = ["tmpfs", "ramfs"];

/// Whether writes below a path outlive the container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathPersistence {
    /// On a mount that outlives the container, or not in a container at all.
    Persistent,
    /// In the container's writable layer or on a memory-backed filesystem.
    Ephemeral,
    /// Not enough information — no mount table, or a path that cannot be matched.
    Unknown,
}

/// One line of the mount table.
///
/// The device the line starts with is not kept: only the mount point and the filesystem type
/// decide anything here, and an unread field would be one more thing to keep parsing right.
#[derive(Clone, Debug)]
pub(crate) struct MountEntry {
    pub mount_point: PathBuf,
    pub fs_type: String,
}

/// The mount table, ordered as the kernel reports it.
///
/// Crate-private: it is an input to `PersistenceProbe` and nothing outside this crate has ever
/// built one. The public seam is `PersistenceProbe::detect`, which reads the table itself.
#[derive(Clone, Debug, Default)]
pub(crate) struct MountTable {
    entries: Vec<MountEntry>,
}

impl MountTable {
    /// Parses the `/proc/mounts` format. The seam every test goes through.
    #[must_use]
    pub(crate) fn parse(table: &str) -> Self {
        let entries = table
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                // The device field is required for the line to be a mount line at all, so it
                // is consumed and discarded rather than skipped.
                fields.next()?;
                let mount_point = fields.next()?;
                let fs_type = fields.next()?;
                Some(MountEntry {
                    mount_point: PathBuf::from(unescape(mount_point)),
                    fs_type: fs_type.to_owned(),
                })
            })
            .collect();
        Self { entries }
    }

    /// Reads the running system's mount table; `None` where there is no procfs.
    ///
    /// The target check is `cfg!` and not `#[cfg]` on purpose. An attribute would compile the
    /// call to `parse` out everywhere but Linux, which left `parse` and `unescape` with no
    /// caller on the Windows target — dead code, and an error once that target is linted with
    /// `-D warnings` (RD-109-40). A runtime branch on a compile-time constant costs nothing,
    /// keeps one body instead of two, and keeps the parser under the lint on every target.
    #[must_use]
    fn read() -> Option<Self> {
        if cfg!(target_os = "linux") {
            std::fs::read_to_string("/proc/mounts")
                .ok()
                .map(|table| Self::parse(&table))
        } else {
            None
        }
    }

    /// The mount a write to `path` actually lands on: the longest matching mount point, and
    /// among equal ones the entry mounted last, because that is the one shadowing the others.
    #[must_use]
    fn covering(&self, path: &Path) -> Option<&MountEntry> {
        let mut best: Option<&MountEntry> = None;
        for entry in &self.entries {
            if !covers(&entry.mount_point, path) {
                continue;
            }
            let longer_or_equal = best.is_none_or(|current| {
                entry.mount_point.components().count() >= current.mount_point.components().count()
            });
            if longer_or_equal {
                best = Some(entry);
            }
        }
        best
    }
}

/// Decodes the octal escapes the kernel writes for space, tab, newline and backslash.
fn unescape(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_owned();
    }
    let mut out = String::with_capacity(field.len());
    let mut rest = field;
    while let Some(index) = rest.find('\\') {
        out.push_str(&rest[..index]);
        let escape = &rest[index..];
        let decoded = escape
            .get(1..4)
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match decoded {
            Some(byte) => {
                out.push(char::from(byte));
                rest = &escape[4..];
            }
            None => {
                out.push('\\');
                rest = &escape[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether `mount_point` contains `path`, compared component by component so that
/// `/downloads` does not swallow `/downloads-old`.
fn covers(mount_point: &Path, path: &Path) -> bool {
    let mut expected = mount_point.components();
    let mut actual = path.components();
    loop {
        match (expected.next(), actual.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(left), Some(right)) if left == right => {}
            (Some(_), Some(_)) => return false,
        }
    }
}

/// Classifies paths against one snapshot of the mount table.
#[derive(Clone, Debug)]
pub struct PersistenceProbe {
    table: Option<MountTable>,
    containerized: bool,
}

impl PersistenceProbe {
    /// Reads the mount table and works out whether this process is in a container.
    #[must_use]
    pub fn detect() -> Self {
        Self {
            table: MountTable::read(),
            containerized: containerized(),
        }
    }

    /// Builds a probe from a known table — the constructor tests use.
    ///
    /// Test-only: every production path goes through `detect`, which reads the real table.
    /// Exposing it more widely would make `MountTable` part of this crate's public surface
    /// for no caller.
    #[cfg(all(test, unix))]
    #[must_use]
    pub(crate) fn new(table: Option<MountTable>, containerized: bool) -> Self {
        Self {
            table,
            containerized,
        }
    }

    /// Whether this process appears to run inside a container.
    #[must_use]
    pub fn containerized(&self) -> bool {
        self.containerized
    }

    /// Whether writes below `path` outlive the container.
    #[must_use]
    pub fn classify(&self, path: &Path) -> PathPersistence {
        if !path.is_absolute() {
            return PathPersistence::Unknown;
        }
        let Some(table) = self.table.as_ref() else {
            return PathPersistence::Unknown;
        };
        let Some(entry) = table.covering(path) else {
            return PathPersistence::Unknown;
        };
        if VOLATILE_TYPES.contains(&entry.fs_type.as_str()) {
            return PathPersistence::Ephemeral;
        }
        // Only the container's own root layer is a finding. A host's overlay root is that
        // host's arrangement and none of our business.
        if self.containerized
            && entry.mount_point == Path::new("/")
            && OVERLAY_TYPES.contains(&entry.fs_type.as_str())
        {
            return PathPersistence::Ephemeral;
        }
        PathPersistence::Persistent
    }
}

/// Container detection, from the markers runtimes leave behind.
#[cfg(target_os = "linux")]
fn containerized() -> bool {
    use std::sync::OnceLock;

    static CONTAINERIZED: OnceLock<bool> = OnceLock::new();
    *CONTAINERIZED.get_or_init(|| {
        if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
            return true;
        }
        if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
            return true;
        }
        std::fs::read_to_string("/proc/1/cgroup").is_ok_and(|cgroup| {
            ["docker", "containerd", "libpod", "kubepods"]
                .iter()
                .any(|marker| cgroup.contains(marker))
        })
    })
}

#[cfg(not(target_os = "linux"))]
fn containerized() -> bool {
    false
}

// Unix only: every case is a Unix mount table with Unix-absolute paths, which on Windows are
// relative (no drive) and classify as `Unknown` before the table is read. The table itself exists
// only on Linux.
#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;

    use super::{MountTable, PathPersistence, PersistenceProbe};

    /// A container with a volume at `/downloads` and a bind mount at `/media/movies`.
    const CONTAINER: &str = "\
overlay / overlay rw,lowerdir=/a:/b,upperdir=/c,workdir=/d 0 0
proc /proc proc rw,nosuid 0 0
tmpfs /tmp tmpfs rw,nosuid 0 0
/dev/sdb1 /downloads ext4 rw,relatime 0 0
/dev/sdc1 /media/movies ext4 rw,relatime 0 0
";

    const BARE_METAL: &str = "\
/dev/sda2 / ext4 rw,relatime 0 0
proc /proc proc rw,nosuid 0 0
/dev/sda1 /boot vfat rw,relatime 0 0
";

    fn container() -> PersistenceProbe {
        PersistenceProbe::new(Some(MountTable::parse(CONTAINER)), true)
    }

    #[test]
    fn a_path_on_a_mounted_volume_is_persistent() {
        assert_eq!(
            container().classify(Path::new("/downloads/movies")),
            PathPersistence::Persistent
        );
    }

    #[test]
    fn a_path_only_the_container_rootfs_covers_is_ephemeral() {
        assert_eq!(
            container().classify(Path::new("/media/series")),
            PathPersistence::Ephemeral,
            "/media/movies is mounted, /media/series is not"
        );
    }

    #[test]
    fn the_mount_point_itself_is_persistent() {
        assert_eq!(
            container().classify(Path::new("/downloads")),
            PathPersistence::Persistent
        );
    }

    #[test]
    fn a_sibling_sharing_a_name_prefix_is_not_covered() {
        // `/downloads-old` must not match the `/downloads` mount: prefixes are compared by
        // path component, not by string.
        assert_eq!(
            container().classify(Path::new("/downloads-old")),
            PathPersistence::Ephemeral
        );
    }

    #[test]
    fn tmpfs_is_ephemeral_even_though_it_is_mounted() {
        assert_eq!(
            container().classify(Path::new("/tmp/staging")),
            PathPersistence::Ephemeral
        );
    }

    #[test]
    fn tmpfs_is_ephemeral_on_a_host_too() {
        // Unlike an overlay root, a memory-backed filesystem is volatile wherever it is:
        // downloads written there are gone after a reboot, container or not.
        let probe = PersistenceProbe::new(Some(MountTable::parse(CONTAINER)), false);
        assert_eq!(
            probe.classify(Path::new("/tmp/staging")),
            PathPersistence::Ephemeral
        );
    }

    #[test]
    fn outside_a_container_an_overlay_root_is_left_alone() {
        let probe = PersistenceProbe::new(Some(MountTable::parse(CONTAINER)), false);
        assert_eq!(
            probe.classify(Path::new("/media/series")),
            PathPersistence::Persistent,
            "an overlay root on a host is that host's business"
        );
    }

    #[test]
    fn a_host_root_is_persistent_even_inside_a_container() {
        // A btrfs or zfs storage driver gives `/` a plain filesystem type. Warning there
        // would be worse than staying quiet, so only a known overlay type counts.
        let probe = PersistenceProbe::new(Some(MountTable::parse(BARE_METAL)), true);
        assert_eq!(
            probe.classify(Path::new("/srv/downloads")),
            PathPersistence::Persistent
        );
    }

    #[test]
    fn without_a_mount_table_nothing_is_claimed() {
        let probe = PersistenceProbe::new(None, true);
        assert_eq!(
            probe.classify(Path::new("/downloads")),
            PathPersistence::Unknown
        );
    }

    #[test]
    fn a_later_mount_shadows_an_earlier_one_at_the_same_point() {
        let table = MountTable::parse(
            "overlay / overlay rw 0 0\n\
             /dev/sdb1 /data ext4 rw 0 0\n\
             tmpfs /data tmpfs rw 0 0\n",
        );
        let probe = PersistenceProbe::new(Some(table), true);
        assert_eq!(
            probe.classify(Path::new("/data/x")),
            PathPersistence::Ephemeral,
            "the tmpfs mounted last is what a write actually lands on"
        );
    }

    #[test]
    fn an_escaped_space_in_a_mount_point_is_decoded() {
        let table =
            MountTable::parse("overlay / overlay rw 0 0\n/dev/sdb1 /my\\040volume ext4 rw 0 0\n");
        let probe = PersistenceProbe::new(Some(table), true);
        assert_eq!(
            probe.classify(Path::new("/my volume/downloads")),
            PathPersistence::Persistent
        );
    }

    #[test]
    fn a_relative_path_cannot_be_judged() {
        assert_eq!(
            container().classify(Path::new("downloads")),
            PathPersistence::Unknown
        );
    }
}

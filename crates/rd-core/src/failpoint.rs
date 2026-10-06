//! Deterministic crash points, for proving what survives a process that stops mid-write.
//!
//! A transfer that is interrupted between writing bytes and recording that it wrote them is
//! the case every resumable download has to get right, and it is exactly the case ordinary
//! tests never reach: they run to completion. The only way to cover it is to make the
//! interruption itself addressable, so a test can say "stop here" and then assert what the
//! next start makes of the result.
//!
//! ## Why this and not the `fail` crate
//!
//! `fail` is the obvious dependency and was deliberately not taken. It brings a global
//! registry, a string-parsed action grammar (`return`, `panic`, `sleep`, `pause`, `%`
//! probabilities) and its own synchronisation, none of which this project needs — the crash
//! matrix wants one action, "stop now", fired once, at a named point. What it does need is
//! for the whole mechanism to vanish from release builds, which is far easier to guarantee
//! for thirty lines that are `#[cfg]`-gated than for a dependency.
//!
//! ## Why a process-global plan is safe here
//!
//! Arming a failpoint sets process-global state, which would normally be a hazard in a test
//! binary running cases in parallel threads. It is safe in this repository specifically
//! because `cargo-nextest` runs **each test in its own process**, so a plan armed by one case
//! cannot be observed by another. Under `cargo test`, which shares one process across
//! threads, arming the same point from two cases at once would race — hence
//! [`FailpointGuard`], which scopes an arming to one test and disarms on drop, and hence the
//! rule that crash-matrix cases are run with nextest.
//!
//! ## Cost when disabled
//!
//! Without the `failpoints` feature [`failpoint!`] expands to nothing at all: no branch, no
//! atomic load, no symbol. The production build cannot be slowed down or subverted by a
//! mechanism that is not compiled into it.
//!
//! ## The registry
//!
//! Which points exist, who owns them and what a restart after each has to prove is the table
//! `rd_crash_points::CRASH_POINTS`, kept beside `crates/rd-core/recovery-matrix.md` by a test. It
//! lives in a crate only tests use (RD-1120-12): this one is linked into every service crate and
//! into the plugins, and a new point used to rebuild all of them.

/// Stops execution at a named point when that point is armed.
///
/// Expands to nothing unless the `failpoints` feature is on. When armed, the enclosing
/// function returns the error produced by the second argument — a caller-supplied expression,
/// so each call site returns its own error type without this module knowing any of them.
///
/// ```ignore
/// failpoint!("http.after_chunk_write", || anyhow::anyhow!("crash point"));
/// ```
#[macro_export]
#[cfg(feature = "failpoints")]
macro_rules! failpoint {
    ($name:expr, $error:expr) => {
        if $crate::failpoint::is_armed($name) {
            $crate::failpoint::record($name);
            return Err($error());
        }
    };
}

/// Stops execution at a named point when that point is armed. Compiled out here.
#[macro_export]
#[cfg(not(feature = "failpoints"))]
macro_rules! failpoint {
    ($name:expr, $error:expr) => {};
}

#[cfg(feature = "failpoints")]
mod armed {
    use std::{
        collections::{HashMap, HashSet},
        sync::{Mutex, OnceLock, atomic::AtomicBool, atomic::Ordering},
    };

    /// Fast path: with nothing armed, [`super::is_armed`] never takes the lock.
    static ANY_ARMED: AtomicBool = AtomicBool::new(false);

    /// How a point is armed: how many passes to let through first, then how many to stop.
    #[derive(Clone, Copy)]
    struct Arming {
        skip: u32,
        remaining: u32,
    }

    struct Plan {
        /// Name -> how it is armed.
        armed: HashMap<String, Arming>,
        /// Names that actually fired, so a test can assert its point was reached at all.
        fired: HashSet<String>,
    }

    fn plan() -> &'static Mutex<Plan> {
        static PLAN: OnceLock<Mutex<Plan>> = OnceLock::new();
        PLAN.get_or_init(|| {
            Mutex::new(Plan {
                armed: HashMap::new(),
                fired: HashSet::new(),
            })
        })
    }

    /// Arms `name` for the next `hits` passes.
    pub fn arm(name: &str, hits: u32) {
        arm_after(name, 0, hits);
    }

    /// Arms `name` after letting `skip` passes through, then for `hits` passes.
    ///
    /// The `skip` is what reaches mid-stream state: the first pass through a write loop has
    /// an empty file and no checkpoint behind it, which is a different and easier case than
    /// the third pass, where a resume has to land on an offset that is neither zero nor the
    /// end.
    pub fn arm_after(name: &str, skip: u32, hits: u32) {
        let mut plan = plan().lock().expect("failpoint plan");
        plan.armed.insert(
            name.to_owned(),
            Arming {
                skip,
                remaining: hits,
            },
        );
        ANY_ARMED.store(true, Ordering::SeqCst);
    }

    /// Disarms `name` and forgets whether it fired.
    pub fn disarm(name: &str) {
        let mut plan = plan().lock().expect("failpoint plan");
        plan.armed.remove(name);
        plan.fired.remove(name);
        ANY_ARMED.store(!plan.armed.is_empty(), Ordering::SeqCst);
    }

    /// Disarms everything. For a test harness tearing down between cases.
    ///
    /// No caller in the workspace today (audit 2026-10-06, CR-8); kept on purpose as the reset a
    /// harness that arms several points needs, and it exists only under `failpoints`.
    pub fn disarm_all() {
        let mut plan = plan().lock().expect("failpoint plan");
        plan.armed.clear();
        plan.fired.clear();
        ANY_ARMED.store(false, Ordering::SeqCst);
    }

    /// Whether `name` should fire now, consuming one of its remaining hits.
    ///
    /// Consuming here rather than in [`super::record`] keeps the decision and the bookkeeping
    /// under one lock acquisition, so two threads cannot both see the last remaining hit.
    pub fn is_armed(name: &str) -> bool {
        if !ANY_ARMED.load(Ordering::SeqCst) {
            return false;
        }
        let mut plan = plan().lock().expect("failpoint plan");
        let Some(arming) = plan.armed.get_mut(name) else {
            return false;
        };
        if arming.skip > 0 {
            arming.skip -= 1;
            return false;
        }
        if arming.remaining == 0 {
            return false;
        }
        arming.remaining -= 1;
        if arming.remaining == 0 {
            plan.armed.remove(name);
            ANY_ARMED.store(!plan.armed.is_empty(), Ordering::SeqCst);
        }
        true
    }

    /// Notes that `name` fired.
    pub fn record(name: &str) {
        let mut plan = plan().lock().expect("failpoint plan");
        plan.fired.insert(name.to_owned());
    }

    /// Whether `name` fired since it was armed.
    pub fn fired(name: &str) -> bool {
        plan().lock().expect("failpoint plan").fired.contains(name)
    }
}

#[cfg(feature = "failpoints")]
pub use armed::{arm, arm_after, disarm, disarm_all, fired, is_armed, record};

/// Arms a failpoint for the duration of one test and disarms it on drop.
///
/// The drop matters more than the constructor: a case that fails an assertion after arming
/// would otherwise leave the point armed for whatever runs next in the same process.
#[cfg(feature = "failpoints")]
pub struct FailpointGuard {
    name: String,
}

#[cfg(feature = "failpoints")]
impl FailpointGuard {
    /// Arms `name` for exactly one pass.
    #[must_use]
    pub fn once(name: &str) -> Self {
        arm(name, 1);
        Self {
            name: name.to_owned(),
        }
    }

    /// Arms `name` for exactly one pass, after letting `skip` passes through.
    #[must_use]
    pub fn after(name: &str, skip: u32) -> Self {
        arm_after(name, skip, 1);
        Self {
            name: name.to_owned(),
        }
    }

    /// Whether the point has fired yet.
    #[must_use]
    pub fn fired(&self) -> bool {
        fired(&self.name)
    }
}

#[cfg(feature = "failpoints")]
impl Drop for FailpointGuard {
    fn drop(&mut self) {
        disarm(&self.name);
    }
}

#[cfg(all(test, feature = "failpoints"))]
mod tests {
    use super::*;

    fn guarded(name: &str) -> Result<&'static str, String> {
        failpoint!(name, || "stopped".to_owned());
        Ok("finished")
    }

    /// A point nobody armed must not cost anything or change any behaviour.
    #[test]
    fn an_unarmed_point_does_nothing() {
        assert_eq!(guarded("test.unarmed"), Ok("finished"));
    }

    /// The point of the whole module: stop exactly once, at a named place.
    #[test]
    fn an_armed_point_fires_once_and_then_lets_the_call_through() {
        let guard = FailpointGuard::once("test.fires_once");
        assert_eq!(guarded("test.fires_once"), Err("stopped".to_owned()));
        assert!(guard.fired());
        // Re-armed for one hit only, so the restart the test is simulating runs to the end.
        assert_eq!(guarded("test.fires_once"), Ok("finished"));
    }

    /// A case that fails mid-way must not arm the point for whatever runs next.
    #[test]
    fn the_guard_disarms_when_it_goes_out_of_scope() {
        {
            let _guard = FailpointGuard::once("test.scoped");
        }
        assert_eq!(guarded("test.scoped"), Ok("finished"));
    }

    /// Arming one point must not fire another; the names are the whole addressing scheme.
    #[test]
    fn arming_one_point_leaves_the_others_alone() {
        let _guard = FailpointGuard::once("test.armed_one");
        assert_eq!(guarded("test.armed_other"), Ok("finished"));
    }

    /// A multi-hit arming stops the first two passes and no more.
    #[test]
    fn a_point_can_be_armed_for_several_hits() {
        arm("test.twice", 2);
        assert!(guarded("test.twice").is_err());
        assert!(guarded("test.twice").is_err());
        assert!(guarded("test.twice").is_ok());
        disarm("test.twice");
    }

    /// Reaching mid-stream state: let two passes through, stop on the third.
    #[test]
    fn a_point_can_be_armed_to_fire_after_some_passes() {
        let guard = FailpointGuard::after("test.third_pass", 2);
        assert!(guarded("test.third_pass").is_ok());
        assert!(guarded("test.third_pass").is_ok());
        assert!(guarded("test.third_pass").is_err());
        assert!(guard.fired());
        assert!(guarded("test.third_pass").is_ok());
    }
}

/// The registry and `crates/rd-core/recovery-matrix.md` are one thing described twice; keep them
/// equal.
///
/// A matrix document that drifts from the code is worse than none: it reads as a statement
/// about what is covered, and a reader has no way to tell that it stopped being true.
#[cfg(test)]
mod matrix_document_tests {
    use rd_crash_points::{CRASH_POINTS, crash_point};

    const MATRIX: &str = include_str!("../recovery-matrix.md");

    /// Every registered point has a row, with the invariant it actually claims.
    #[test]
    fn the_matrix_document_lists_every_crash_point() {
        for point in CRASH_POINTS {
            let row = format!(
                "| `{}` | {} | {} |",
                point.name, point.owner, point.invariant
            );
            assert!(
                MATRIX.contains(&row),
                "crates/rd-core/recovery-matrix.md is missing this row:\n{row}"
            );
        }
    }

    /// And no row describes a point that does not exist.
    #[test]
    fn the_matrix_document_lists_no_crash_point_that_was_removed() {
        for line in MATRIX.lines() {
            let Some(rest) = line.strip_prefix("| `") else {
                continue;
            };
            let Some((name, _)) = rest.split_once('`') else {
                continue;
            };
            if !name.contains('.') {
                continue;
            }
            assert!(
                crash_point(name).is_some(),
                "crates/rd-core/recovery-matrix.md lists {name}, which is not registered"
            );
        }
    }
}

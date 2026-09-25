//! The state rules the tray shows: what one health probe means for the service, and the line
//! that says so.
//!
//! Pure rules next to the states they produce, so they are tested on every host while the tray
//! that draws them compiles on Windows and macOS only. The module carries the tray's gate plus
//! `test`; `activity.rs` is the same pattern for the transfer figures.

use std::time::Duration;

use reqwest::StatusCode;
use url::Url;

/// Whether the service the tray points at is answering.
///
/// The tray used to open the browser blind, which lands on an error page whenever the service is
/// still starting — the common case right after login, when both are launched together.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ServerStatus {
    /// Not answering yet, and not long enough to call it unreachable.
    #[default]
    Starting,
    Running,
    Unreachable,
}

/// What one health probe saw, as far as the state rule cares.
///
/// Telling "nothing answered" apart from "something answered badly" is the whole point of this
/// enum. The service binds its listener only once the database is open, the migrations are
/// applied and every plugin is loaded (`rd_api::serve`), so a refused connection is exactly what
/// a service that is still coming up looks like — while a reply that is not a success came from
/// something that is already listening on that address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HealthProbe {
    /// The service answered with a success status.
    Healthy,
    /// Something answered on that address, but not with a success status.
    Unhealthy,
    /// Nothing answered: the connection was refused, the request timed out, or it never
    /// completed for any other reason. From here all of those are one thing — silence.
    Silent,
}

/// How long a silent service may stay silent before it counts as unreachable rather than
/// starting.
///
/// Ninety seconds, because that is what the service's own start now costs: it opens the
/// database, applies the migrations and verifies, compiles and installs **more than forty signed
/// plugin components** before `rd_api::serve` binds the listener, and a cold machine was
/// reported still working at it after a full minute. The twenty seconds this replaces were set
/// when there were a handful of plugins.
///
/// The margin over that minute is deliberately generous, because the two ways of being wrong are
/// not equal: a grace that is too short puts "server not reachable" in front of somebody whose
/// service is perfectly fine, while a grace that is too long only delays a true "not reachable"
/// by a few polls, and nothing acts on that state automatically.
pub(crate) const HEALTH_GRACE: Duration = Duration::from_secs(90);

/// How often the service is asked whether it is up.
///
/// Beside the grace it is measured against rather than in `tray.rs`, so a test can state the
/// relation between the two: the grace has to span enough polls that a slow start is seen
/// changing, not merely waited out.
// The cadence is shared with the transfer poll rather than restated here: two constants that
// "must match" are two constants that will not (RD-109-09).
pub(crate) const HEALTH_INTERVAL: Duration = crate::config::STATUS_POLL_INTERVAL;

/// Reduces one health request to what the state rule cares about.
///
/// `None` is "nothing answered" — a refused connection, a timeout, a name that does not resolve;
/// out here all of those are one thing, silence. `Some(status)` means something on that address
/// replied, and the code says whether it replied well.
///
/// Takes the status rather than the whole response because that is the only part the rule reads,
/// and because a `reqwest::Response` cannot be built by hand without pulling `http` in as a
/// dev-dependency — which would have left this classification untested on every host.
pub(crate) fn health_probe(answer: Option<StatusCode>) -> HealthProbe {
    match answer {
        Some(status) if status.is_success() => HealthProbe::Healthy,
        Some(_) => HealthProbe::Unhealthy,
        None => HealthProbe::Silent,
    }
}

/// What the health poll carries between requests.
///
/// Only `ever_answered`, but it is the part of the poll that has a history: it is what turns the
/// start-up grace off for good once the service has spoken, and it is a one-way latch. Kept here
/// rather than as a local in the spawned loop so the sequence — silent, healthy, silent again —
/// can be played through on a host with no tray.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HealthWatch {
    ever_answered: bool,
}

impl HealthWatch {
    /// Folds one probe into the running state and returns what the tray should show.
    ///
    /// `waited` is the time since the poll started, which only matters while nothing has ever
    /// answered.
    pub(crate) fn observe(&mut self, probe: HealthProbe, waited: Duration) -> ServerStatus {
        self.ever_answered |= probe != HealthProbe::Silent;
        server_status(probe, self.ever_answered, waited, HEALTH_GRACE)
    }
}

/// Decides what one health probe means for the tray.
///
/// A pure function, next to the states it produces, so the rule can be tested on every host —
/// the tray itself compiles on Windows and macOS only.
///
/// `ever_answered` is what separates "not up yet" from "gone": once something has answered on
/// that address the start-up case is over, so silence afterwards is unreachability at once
/// rather than a second helping of grace.
pub(crate) fn server_status(
    probe: HealthProbe,
    ever_answered: bool,
    waited: Duration,
    grace: Duration,
) -> ServerStatus {
    match probe {
        HealthProbe::Healthy => ServerStatus::Running,
        // Something is listening and it is not serving health. That is a service which is there
        // and unhappy — a wrong port, a proxy in front of nothing, an instance shutting down —
        // and calling it "starting" would be a guess about a machine that is already answering.
        HealthProbe::Unhealthy => ServerStatus::Unreachable,
        HealthProbe::Silent if ever_answered || waited > grace => ServerStatus::Unreachable,
        HealthProbe::Silent => ServerStatus::Starting,
    }
}

/// Renders the disabled status line of the tray menu.
///
/// Lives outside the platform-gated `tray` module so that it stays unit
/// testable on every host.
pub(crate) fn status_label(service: Option<&Url>, status: ServerStatus) -> String {
    let product = format!("rDownloader Capture v{}", env!("CARGO_PKG_VERSION"));
    let Some(service) = service else {
        return format!("{product} — not configured");
    };
    let host = service.host_str().unwrap_or("unknown host");
    let endpoint = match service.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let state = match status {
        ServerStatus::Starting => "server starting",
        ServerStatus::Running => "server running",
        ServerStatus::Unreachable => "server not reachable",
    };
    format!("{product} — {endpoint} — {state}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::StatusCode;
    use url::Url;

    use super::{
        HEALTH_GRACE, HEALTH_INTERVAL, HealthProbe, HealthWatch, ServerStatus, health_probe,
        server_status, status_label,
    };

    fn code(status: u16) -> Option<StatusCode> {
        Some(StatusCode::from_u16(status).expect("a status code the tray could actually receive"))
    }

    /// Any success is healthy, anything else that answered is not, and no answer is silence.
    ///
    /// `204` and `301` are the two that a coarser rule gets wrong in opposite directions: the
    /// first is a success the service may legitimately return, the second is something else
    /// answering on the address — a proxy or a captive portal — which is not the service being up.
    #[test]
    fn a_health_answer_is_classified_by_whether_it_succeeded() {
        assert_eq!(health_probe(code(200)), HealthProbe::Healthy);
        assert_eq!(health_probe(code(204)), HealthProbe::Healthy);
        assert_eq!(health_probe(code(301)), HealthProbe::Unhealthy);
        assert_eq!(health_probe(code(401)), HealthProbe::Unhealthy);
        assert_eq!(health_probe(code(503)), HealthProbe::Unhealthy);
        assert_eq!(health_probe(None), HealthProbe::Silent);
    }

    /// The whole life of one tray session, played through in order.
    ///
    /// This is the sequence the poll actually walks and no test could reach before: the service
    /// is silent while it starts, answers, and later goes away. The last step is the one worth
    /// having — after an answer, silence is unreachability at once, with no second helping of
    /// grace, however little time has passed since.
    #[test]
    fn a_service_that_starts_answers_and_dies_walks_the_three_states_in_order() {
        let mut watch = HealthWatch::default();
        assert_eq!(
            watch.observe(HealthProbe::Silent, Duration::from_secs(5)),
            ServerStatus::Starting
        );
        assert_eq!(
            watch.observe(HealthProbe::Healthy, Duration::from_secs(10)),
            ServerStatus::Running
        );
        assert_eq!(
            watch.observe(HealthProbe::Silent, Duration::from_secs(15)),
            ServerStatus::Unreachable,
            "a service that has answered is never starting again"
        );
    }

    /// The latch does not reset, and an unhealthy answer arms it just as a healthy one does.
    ///
    /// Something answering badly is still something answering, so the start-up excuse is spent
    /// from that moment on — which is precisely what a tray pointed at a proxy in front of a
    /// dead service must not keep calling "starting".
    #[test]
    fn once_anything_has_answered_the_start_up_grace_never_comes_back() {
        let mut watch = HealthWatch::default();
        assert_eq!(
            watch.observe(HealthProbe::Unhealthy, Duration::from_secs(1)),
            ServerStatus::Unreachable
        );
        for waited in [2, 3, 4] {
            assert_eq!(
                watch.observe(HealthProbe::Silent, Duration::from_secs(waited)),
                ServerStatus::Unreachable,
                "the grace came back after {waited}s"
            );
        }
    }

    /// A service that stays silent is still starting until the grace is spent, poll by poll.
    ///
    /// The accumulator has to leave the untouched case alone: nothing has answered, so every one
    /// of these ticks is a start-up tick.
    #[test]
    fn a_silent_service_stays_starting_for_every_poll_within_the_grace() {
        let mut watch = HealthWatch::default();
        let mut waited = Duration::ZERO;
        while waited <= HEALTH_GRACE {
            assert_eq!(
                watch.observe(HealthProbe::Silent, waited),
                ServerStatus::Starting,
                "a silent service was given up on after {waited:?}"
            );
            waited += HEALTH_INTERVAL;
        }
        assert_eq!(
            watch.observe(HealthProbe::Silent, waited),
            ServerStatus::Unreachable
        );
    }

    /// The grace is worth several polls, not one or two.
    ///
    /// A grace shorter than a handful of intervals would make the state hinge on which tick
    /// happened to land last, rather than on the service.
    #[test]
    fn the_grace_spans_many_polls() {
        assert!(
            HEALTH_INTERVAL > Duration::ZERO,
            "a poll interval of zero would spin"
        );
        assert!(
            HEALTH_GRACE >= HEALTH_INTERVAL * 10,
            "the start-up grace has to outlast far more than a couple of polls"
        );
    }

    #[test]
    fn status_label_names_the_configured_service() {
        let service = Url::parse("http://192.168.0.5:8710").expect("valid URL");
        assert_eq!(
            status_label(Some(&service), ServerStatus::Running),
            format!(
                "rDownloader Capture v{} — 192.168.0.5:8710 — server running",
                env!("CARGO_PKG_VERSION")
            )
        );
        let implicit_port = Url::parse("https://box.example.com/").expect("valid URL");
        assert_eq!(
            status_label(Some(&implicit_port), ServerStatus::Running),
            format!(
                "rDownloader Capture v{} — box.example.com — server running",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    /// The tray used to open the browser blind, so a service that was still starting looked
    /// broken. The line has to name which of the three it is.
    #[test]
    fn status_label_names_the_server_state() {
        let service = Url::parse("http://127.0.0.1:8710").expect("valid URL");
        assert!(
            status_label(Some(&service), ServerStatus::Starting).ends_with("server starting"),
            "a service that has not answered yet reads as starting"
        );
        assert!(
            status_label(Some(&service), ServerStatus::Unreachable)
                .ends_with("server not reachable")
        );
        assert!(status_label(Some(&service), ServerStatus::Running).ends_with("server running"));
    }

    #[test]
    fn status_label_reports_a_missing_configuration() {
        assert_eq!(
            status_label(None, ServerStatus::Starting),
            format!(
                "rDownloader Capture v{} — not configured",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    /// The report this rule was corrected for: service and agent are started together, the
    /// service needs the better part of a minute for its plugins and migrations, and the tray
    /// used to give up after twenty seconds and say the server was not reachable.
    #[test]
    fn a_service_that_takes_a_minute_to_start_is_still_starting() {
        for waited in [1, 20, 21, 45, 60] {
            assert_eq!(
                server_status(
                    HealthProbe::Silent,
                    false,
                    Duration::from_secs(waited),
                    HEALTH_GRACE
                ),
                ServerStatus::Starting,
                "a silent service is still starting after {waited}s"
            );
        }
        assert!(
            HEALTH_GRACE > Duration::from_secs(60),
            "the grace has to outlast a cold start, which is around a minute"
        );
    }

    /// The message does not disappear, it only stops arriving too early.
    #[test]
    fn a_service_that_never_comes_is_unreachable_once_the_grace_is_spent() {
        assert_eq!(
            server_status(
                HealthProbe::Silent,
                false,
                HEALTH_GRACE + Duration::from_secs(1),
                HEALTH_GRACE
            ),
            ServerStatus::Unreachable
        );
    }

    /// Nothing listening and something answering badly say different things, and only the first
    /// of them is what a service that is coming up looks like.
    #[test]
    fn a_refused_connection_and_an_error_response_are_not_the_same_state() {
        let early = Duration::from_secs(1);
        assert_eq!(
            server_status(HealthProbe::Silent, false, early, HEALTH_GRACE),
            ServerStatus::Starting
        );
        assert_eq!(
            server_status(HealthProbe::Unhealthy, false, early, HEALTH_GRACE),
            ServerStatus::Unreachable,
            "something is listening, so the start-up excuse is gone"
        );
    }

    /// Grace is for a service that has never answered. One that has is not starting any more.
    #[test]
    fn a_service_that_answered_once_is_unreachable_as_soon_as_it_goes_silent() {
        assert_eq!(
            server_status(
                HealthProbe::Silent,
                true,
                Duration::from_secs(1),
                HEALTH_GRACE
            ),
            ServerStatus::Unreachable
        );
    }

    #[test]
    fn a_healthy_answer_is_running_however_long_it_took() {
        for waited in [0, 60, 600] {
            assert_eq!(
                server_status(
                    HealthProbe::Healthy,
                    false,
                    Duration::from_secs(waited),
                    HEALTH_GRACE
                ),
                ServerStatus::Running
            );
        }
    }
}

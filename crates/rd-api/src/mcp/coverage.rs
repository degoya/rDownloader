//! What the interface can do, what the toolbox can do, and why the difference is the shape it
//! is (RD-120-29).
//!
//! ## Why this table exists at all
//!
//! The question that produced it was not a defect report but a question: *are all functions
//! available over MCP, or are newer ones not covered?* Answering it once would have produced a
//! list, and a list of gaps is stale the moment somebody adds a route. So the answer is a
//! table with a **decision** per capability, and [`tests`] holds it against
//! [`crate::openapi_document`] in both directions: an operation that falls into no capability
//! fails the build, and a capability that matches no operation fails it too. Adding a REST
//! route without saying whether the toolbox should have it is therefore not possible, which is
//! the only way a coverage answer stays true.
//!
//! ## Why the unit is a capability and not a route
//!
//! What a person would name — "the settings document", "remote jobs", "solving captchas" — and a
//! capability counts as covered when the toolbox can *do the thing*, not when every route
//! beneath it has its own tool. `list_batches` has no tool; the LinkGrabber capability it
//! belongs to is covered by collect, check and enqueue. Where one question has several routes —
//! the six read-outs of a torrent's panel — the tool takes a `view` and every route it reaches
//! is listed in `TOOL_ALSO_REACHES`, which [`tools_for`] reads too.
//!
//! ## Everything, unless the owner said no (RD-120-32)
//!
//! RD-120-29 built this table on the premise that a toolbox should do "the right thing" rather
//! than everything, and left forty capabilities out on it — "that is done with the mouse",
//! "a position in a list the caller cannot see". The owner's intent was always the opposite:
//! **whatever a person can do in the interface, an agent can do too.** A list the caller cannot
//! see is an argument for a listing tool, not against the tools that act on its rows.
//!
//! What stays out is of two kinds, and each row says which:
//!
//! * **The owner's decision of 2026-09-23**, one reason for all of its rows, [`OWNER_LINE`]: a
//!   tool that hands out a secret, takes one in, gives a consent, or changes something outside
//!   this machine irreversibly is not offered. Nine rows are his list; three more are the parts
//!   of the thirteen RD-120-55 checked that meet one of the marks. Anybody adding a capability
//!   later checks it against those four marks before writing a tool for it.
//! * **Redundant**: a second spelling of something a tool already does, or not a user-facing
//!   capability at all. The reason names the tool that does it.
//!
//! RD-120-32's specification left thirteen rows unclassified. The owner answered on
//! 2026-09-24 that they come in too, unless one meets a mark (RD-120-55); the rows taken by
//! RD-120-55 are those, and the parts that met a mark sit with the owner's rows.

use axum::http::Method;

/// One route prefix a capability claims, and optionally the single method it claims it for.
///
/// Prefix rather than exact path so that `/api/v1/downloads/{id}/torrent/peers` needs no entry
/// of its own once the torrent panel claims `/api/v1/downloads/{id}/torrent`. The longest
/// prefix wins, and among equal prefixes the one naming a method wins over the one that does
/// not — which is what lets `GET /api/v1/site-rules` be covered while `POST` to the same path
/// is not.
pub(crate) struct Claim {
    pub prefix: &'static str,
    /// `None` claims every method on the prefix.
    ///
    /// Spelled as an uppercase name rather than as an [`Method`]: `Method` owns a `Box` for
    /// its extension case, so it carries drop glue, and a `&[Claim]` nested inside a `const`
    /// item then cannot be promoted to `'static` — `error[E0493]: destructor of [Claim; 1]
    /// cannot be evaluated at compile-time`. `scope_policy`'s flat table does not hit this;
    /// this one is a slice of slices, which does.
    pub method: Option<&'static str>,
}

const fn any(prefix: &'static str) -> Claim {
    Claim {
        prefix,
        method: None,
    }
}

const fn only(prefix: &'static str, method: &'static str) -> Claim {
    Claim {
        prefix,
        method: Some(method),
    }
}

/// Whether the toolbox covers a capability, and if not, why not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    /// The toolbox can do this. Which tools is read from [`super::TOOL_POLICY`], not written
    /// here, so the two cannot drift.
    Covered,
    /// Deliberately out, with the reason. Never empty.
    Omitted(&'static str),
}

/// One thing a person can do with rDownloader.
pub(crate) struct Capability {
    /// In the words the interface uses.
    pub name: &'static str,
    /// Where it lives on screen, or `-` when it has no screen at all.
    pub surface: &'static str,
    pub claims: &'static [Claim],
    pub decision: Decision,
}

const fn covered(
    name: &'static str,
    surface: &'static str,
    claims: &'static [Claim],
) -> Capability {
    Capability {
        name,
        surface,
        claims,
        decision: Decision::Covered,
    }
}

const fn omitted(
    name: &'static str,
    surface: &'static str,
    claims: &'static [Claim],
    why: &'static str,
) -> Capability {
    Capability {
        name,
        surface,
        claims,
        decision: Decision::Omitted(why),
    }
}

/// The one reason the owner gave for the nine capabilities he decided to keep out.
///
/// Written once and referenced by every row it keeps out on purpose: the decision is his, taken
/// over the whole list on 2026-09-23 (in his words: we do not want to offer these, it would be
/// too much), and separately argued sentences would read as subagent judgements somebody could
/// reopen one by one. RD-120-55 applied the same line to the thirteen RD-120-32 left open, so
/// three more rows carry it -- the line, not a derivation of its own.
pub(crate) const OWNER_LINE: &str = "Owner's decision, 2026-09-23 (RD-120-32): not offered. \
     A tool that hands out a secret, takes one in, gives a consent, or changes something \
     outside this machine irreversibly is not offered -- not because it could not be built, \
     but because an agent holding it could do what the person meant to do themselves.";

/// Every capability this installation has, and the decision about each.
///
/// Sorted by nothing in particular: covered first, then the omissions grouped by the part of
/// the product they belong to, because that is the order a reader checks them in.
///
/// **A `static` and not a `const`, and that is load-bearing.** A `const` is inlined at every
/// use site, so `COVERAGE.iter()` here and `COVERAGE.iter()` in a test are free to be two
/// different arrays at two different addresses. [`capability_for`] returns a reference into
/// the table and its callers ask `std::ptr::eq` whether it is *this* capability, which is
/// silently always false when the copies differ -- and the failure is not a wrong answer but
/// an empty one: every capability reads as matching no route at all. A `static` has exactly
/// one address, so identity means what it looks like it means.
pub(crate) static COVERAGE: &[Capability] = &[
    // ---- covered ----
    covered(
        "Queue: add, list and control downloads",
        "Downloads",
        &[
            only("/api/v1/downloads", "GET"),
            only("/api/v1/downloads", "POST"),
            any("/api/v1/downloads/bulk"),
            any("/api/v1/downloads/summary"),
        ],
    ),
    covered(
        "Packages in the queue",
        "Downloads",
        &[
            only("/api/v1/packages", "GET"),
            any("/api/v1/packages/delete"),
        ],
    ),
    covered(
        "LinkGrabber: collect, check, enqueue",
        "LinkGrabber",
        &[
            any("/api/v1/collector/batches"),
            any("/api/v1/collector/candidates/check"),
            only("/api/v1/collector/packages", "GET"),
            any("/api/v1/collector/packages/enqueue"),
        ],
    ),
    covered(
        "The settings document",
        "Settings",
        &[any("/api/v1/settings")],
    ),
    covered(
        "Categories",
        "Settings > Routing",
        &[any("/api/v1/categories"), any("/api/v1/categories/{id}")],
    ),
    covered(
        "Routing rules",
        "Settings > Routing",
        &[
            any("/api/v1/category-rules"),
            any("/api/v1/category-rules/{id}"),
        ],
    ),
    covered(
        "Storage roots",
        "Settings > Storage",
        &[any("/api/v1/storage-roots")],
    ),
    covered(
        "Watched folders",
        "Settings > Intake",
        &[any("/api/v1/hotfolders")],
    ),
    covered(
        "Provider accounts",
        "Settings > Accounts",
        &[any("/api/v1/accounts"), any("/api/v1/accounts/{id}")],
    ),
    covered(
        "The provider table",
        "Settings > Accounts",
        &[any("/api/v1/providers")],
    ),
    covered(
        "Proxy profiles",
        "Settings > Network",
        &[any("/api/v1/proxy-profiles")],
    ),
    covered(
        "Usenet servers",
        "Settings > Usenet",
        &[
            any("/api/v1/usenet/servers"),
            any("/api/v1/usenet/servers/{id}"),
        ],
    ),
    covered(
        "Notification targets and rules",
        "Settings > Notifications",
        &[
            any("/api/v1/notifications/targets"),
            any("/api/v1/notifications/targets/{id}"),
            any("/api/v1/notifications/rules"),
        ],
    ),
    covered(
        "Subscriptions",
        "Subscriptions",
        &[
            any("/api/v1/subscriptions"),
            any("/api/v1/subscriptions/{id}"),
        ],
    ),
    covered(
        "Livestream channels",
        "Streams",
        &[any("/api/v1/streams/channels")],
    ),
    covered(
        "Automations",
        "Automation",
        &[
            any("/api/v1/automations"),
            any("/api/v1/automations/{id}"),
            any("/api/v1/automations/{id}/enable"),
        ],
    ),
    // RD-120-28 found the reason this one reads through the route rather than the manifests:
    // a tool answering from a manifest alone cannot see the execution store, so it reported
    // `active: false` for every plugin and would have reported `execution_count: 0` for one
    // that had run a thousand times. The decision was not to patch the field but to stop
    // having a second implementation -- see `tools_config`'s plugins section.
    covered(
        "Installed plugins: switch and uninstall",
        "Settings > Plugins",
        &[only("/api/v1/plugins", "GET"), any("/api/v1/plugins/{id}")],
    ),
    // ---- taken by RD-120-29 ----
    covered(
        "Remote jobs",
        "Remote jobs",
        &[
            only("/api/v1/remote-jobs", "GET"),
            only("/api/v1/remote-jobs/{id}", "DELETE"),
            any("/api/v1/remote-jobs/{id}/choice"),
            any("/api/v1/accounts/{id}/remote-jobs"),
        ],
    ),
    covered(
        "Transfer statistics",
        "Statistics",
        &[any("/api/v1/stats/transfers")],
    ),
    covered("The log store", "Logs", &[any("/api/v1/diagnostics/logs")]),
    covered("The audit log", "Audit", &[any("/api/v1/audit/records")]),
    covered(
        "Clearing logs, audit records and statistics",
        "Settings > System",
        &[
            any("/api/v1/system/data-reset"),
            any("/api/v1/diagnostics/logs/clear"),
            any("/api/v1/audit/records/clear"),
            any("/api/v1/stats/transfers/clear"),
        ],
    ),
    covered(
        "Site rules: read and switch",
        "Settings > Site rules",
        &[
            only("/api/v1/site-rules", "GET"),
            any("/api/v1/site-rules/{id}/enabled"),
            any("/api/v1/site-rule-groups"),
        ],
    ),
    // ---- taken by RD-120-31 ----
    // RD-120-29 left this out because every route took multipart and a tool call carries no
    // file. The routes now take the file as base64 in JSON too, on the same path, so the tools
    // are priced by the same `scope_policy` entries the browser upload is.
    covered(
        "Handing in a container file",
        ".torrent / .nzb / .dlc drop",
        &[
            any("/api/v1/containers/import"),
            any("/api/v1/dlc/import"),
            any("/api/v1/torrents/import"),
            only("/api/v1/nzb/imports", "POST"),
        ],
    ),
    // ---- taken by RD-120-32: everything the interface does ----
    covered(
        "LinkGrabber: candidate-level handling",
        "LinkGrabber",
        &[
            any("/api/v1/collector/candidates"),
            any("/api/v1/collector/entries"),
        ],
    ),
    covered(
        "Mirror groups",
        "LinkGrabber",
        &[
            any("/api/v1/collector/candidates/{id}/mirror"),
            any("/api/v1/collector/mirror-preference"),
        ],
    ),
    covered(
        "Reviewing an NZB before it is queued",
        "NZB import",
        &[any("/api/v1/nzb/imports")],
    ),
    covered(
        "NZB import files and enqueue",
        "NZB import",
        &[any("/api/v1/nzb/imports/{id}")],
    ),
    covered(
        "LinkGrabber: package editing and ordering",
        "LinkGrabber",
        &[
            only("/api/v1/collector/packages", "PATCH"),
            any("/api/v1/collector/packages/bulk"),
            any("/api/v1/collector/packages/regroup"),
            any("/api/v1/collector/packages/reorder"),
            any("/api/v1/collector/packages/{id}"),
        ],
    ),
    covered(
        "Ordering the queue by hand",
        "Downloads",
        &[
            any("/api/v1/downloads/reorder"),
            any("/api/v1/packages/reorder"),
        ],
    ),
    covered(
        "Renaming and retargeting queued work",
        "Downloads",
        &[
            only("/api/v1/downloads/{id}", "PATCH"),
            any("/api/v1/packages/{id}"),
            any("/api/v1/packages/{id}/folder"),
            any("/api/v1/packages/bulk"),
        ],
    ),
    covered(
        "Clearing finished work in one sweep",
        "Downloads",
        &[any("/api/v1/packages/clear")],
    ),
    covered(
        "Unpacking on demand",
        "Downloads",
        &[
            any("/api/v1/downloads/extract"),
            any("/api/v1/packages/extract"),
            any("/api/v1/packages/{id}/extract"),
        ],
    ),
    covered(
        "Torrent detail and seeding",
        "Downloads > torrent panel",
        &[
            any("/api/v1/downloads/{id}/torrent"),
            any("/api/v1/downloads/{id}/seeding"),
            any("/api/v1/categories/{id}/seeding"),
            any("/api/v1/torrents/capabilities"),
            any("/api/v1/torrents/network"),
        ],
    ),
    covered(
        "Post-processing inventory and queue",
        "Settings > Post-processing",
        &[
            any("/api/v1/postprocess/"),
            any("/api/v1/packages/{id}/postprocess"),
            any("/api/v1/categories/{id}/postprocess"),
            any("/api/v1/nzb/imports/{id}/postprocess"),
        ],
    ),
    covered(
        "Managed external tools",
        "Settings > Tools",
        &[any("/api/v1/system/tools"), any("/api/v1/system/media")],
    ),
    covered(
        "Storage capacity",
        "Settings > Storage",
        &[any("/api/v1/storage/capacity")],
    ),
    // The licence list beneath it is claimed too: reading the page is the capability, and a
    // thousand dependency rows are not an answer an agent needs a tool of its own for.
    covered(
        "About rDownloader",
        "Settings > About",
        &[any("/api/v1/system/about")],
    ),
    covered(
        "Writing a site rule",
        "Settings > Site rules",
        &[
            only("/api/v1/site-rules", "POST"),
            only("/api/v1/site-rules/{id}", "PUT"),
            only("/api/v1/site-rules/{id}", "DELETE"),
            any("/api/v1/site-rules/test"),
        ],
    ),
    // ---- deliberately out: the owner's decision of 2026-09-23 ----
    omitted(
        "Deleting a remote job at the provider",
        "Remote jobs",
        &[any("/api/v1/remote-jobs/{id}/discard")],
        OWNER_LINE,
    ),
    omitted(
        "Signing in, sessions, second factor and API tokens",
        "Login, Settings > Security",
        &[
            any("/api/v1/auth/"),
            any("/api/v1/sessions"),
            any("/api/v1/mfa"),
            any("/api/v1/api-tokens"),
            any("/api/v1/setup/"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Signing in at a provider",
        "Settings > Accounts",
        &[
            any("/api/v1/accounts/{id}/auth"),
            any("/api/v1/accounts/{id}/browser-session"),
            any("/api/v1/oauth/callback"),
            any("/api/v1/auth-profiles"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Trying a stored credential or destination",
        "several forms",
        &[
            any("/api/v1/accounts/{id}/test"),
            any("/api/v1/usenet/servers/{id}/test"),
            any("/api/v1/remote-credentials/{id}/test"),
            any("/api/v1/notifications/targets/{id}/test"),
            any("/api/v1/captcha-config/test"),
            any("/api/v1/auth-profiles/{id}/test"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Remote logins and trusted host keys",
        "Settings > Remote",
        &[any("/api/v1/remote-credentials")],
        OWNER_LINE,
    ),
    omitted(
        "Solving captchas",
        "captcha dialog",
        &[
            any("/api/v1/captchas"),
            any("/api/v1/captcha-config"),
            any("/api/v1/captcha-answerers"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Consent to replay a paid link",
        "LinkGrabber",
        &[
            any("/api/v1/collector/candidates/{id}/replay-consent"),
            any("/api/v1/collector/candidates/{id}/replay-preview"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Import and export of a whole area",
        "Settings > Backup",
        &[
            any("/api/v1/settings/export"),
            any("/api/v1/settings/import"),
            any("/api/v1/settings/reset"),
            any("/api/v1/routing/"),
            any("/api/v1/site-rules/export"),
            any("/api/v1/site-rules/import"),
            any("/api/v1/automations/export"),
            any("/api/v1/automations/import"),
            any("/api/v1/streams/export"),
            any("/api/v1/streams/import"),
            any("/api/v1/subscriptions/export"),
            any("/api/v1/subscriptions/import"),
            any("/api/v1/audit/export"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Plugin trust and installation",
        "Settings > Plugins",
        &[
            any("/api/v1/plugins/install"),
            any("/api/v1/plugins/keys"),
            any("/api/v1/plugins/revocations"),
        ],
        OWNER_LINE,
    ),
    // The same line, applied by RD-120-55 to the thirteen RD-120-32 left unclassified: the part
    // of three of them that meets one of the four marks.
    omitted(
        "Probing an indexer's capabilities",
        "Subscriptions",
        &[
            any("/api/v1/subscriptions/caps"),
            any("/api/v1/subscriptions/{id}/caps"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Approving and fetching a diagnostic bundle",
        "Logs",
        &[
            any("/api/v1/diagnostics/bundle"),
            any("/api/v1/diagnostics/bundles"),
        ],
        OWNER_LINE,
    ),
    omitted(
        "Reconnecting on demand",
        "Settings > Network",
        &[only("/api/v1/reconnect", "POST")],
        OWNER_LINE,
    ),
    // A consequence of the decision above rather than a decision of its own: picking a stored
    // browser profile names one, and the profiles are listed by "Signing in at a provider".
    omitted(
        "Choosing a stored browser profile for queued work",
        "Downloads, LinkGrabber",
        &[
            any("/api/v1/downloads/{id}/auth-profile"),
            any("/api/v1/collector/candidates/{id}/auth-profile"),
        ],
        "Each route names one of the stored browser profiles, and listing those is part of \
         signing in at a provider, which the owner decided on 2026-09-23 to keep out. A tool \
         here would take an id no tool can supply -- the gap RD-120-32 exists to close, not \
         one to open.",
    ),
    // ---- deliberately out: redundant, or not a user-facing capability ----
    omitted(
        "The desktop capture agent",
        "the agent, not the web UI",
        &[any("/api/v1/capture/")],
        "Not a user-facing capability but the agent's own contract, priced with its own \
         capture: scope. No api: token reaches it, so a tool over it could not be called.",
    ),
    omitted(
        "Controlling one download by its own route",
        "Downloads",
        &[
            any("/api/v1/downloads/{id}/pause"),
            any("/api/v1/downloads/{id}/resume"),
            any("/api/v1/downloads/{id}/cancel"),
            any("/api/v1/downloads/{id}/reset"),
            only("/api/v1/downloads/{id}", "DELETE"),
        ],
        "control_downloads already does all five for one id or many, over the bulk route. A \
         second spelling of the same act is one more thing for a model to choose between and \
         nothing it could not do before.",
    ),
    omitted(
        "The live rate series",
        "Downloads chart",
        &[any("/api/v1/downloads/rates")],
        "A chart's data series, sampled per second. get_status_summary answers how fast the \
         queue is going in one number, and get_transfer_stats answers it over time.",
    ),
    omitted(
        "Bandwidth budgets and quiet hours",
        "Settings > Bandwidth",
        &[any("/api/v1/bandwidth/")],
        "The limit in force is in the settings document, which update_settings writes. \
         Profiles and the weekly schedule are a calendar grid, and a schedule edited by \
         something that cannot see it is how a quiet hour lands on the wrong day.",
    ),
    omitted(
        "The health probe",
        "-",
        &[any("/api/v1/health")],
        "Public by design: a load balancer asks it without a token, so no permission prices \
         it, and mcp::tool_scope refuses a tool priced by a public route rather than making \
         it free. What it answers -- the service is up, its name and version -- is what the \
         MCP initialize handshake already carries in its server_info.",
    ),
    // ---- taken by RD-120-55: the thirteen RD-120-32 left unclassified ----
    // Checked one by one against the four marks of the owner's line. Where a part of one meets
    // a mark, that part is its own row under the owner's decisions above, and what is left is
    // here. The verdict per capability is in the RD-120-55 job file.
    covered(
        "Which providers can take a remote job",
        "Remote jobs",
        &[any("/api/v1/remote-jobs/providers")],
    ),
    covered(
        "Power actions",
        "Settings > Power",
        &[any("/api/v1/power/")],
    ),
    covered(
        "Plugin execution history",
        "Settings > Plugins",
        &[any("/api/v1/plugins/{id}/executions")],
    ),
    covered(
        "Plugin message catalogues",
        "the interface itself",
        &[any("/api/v1/plugins/i18n")],
    ),
    covered(
        "Automation history, vocabulary and dry run",
        "Automation",
        &[
            any("/api/v1/automations/runs"),
            any("/api/v1/automations/vocabulary"),
            any("/api/v1/automations/dry-run"),
            any("/api/v1/automations/{id}/versions"),
        ],
    ),
    covered(
        "Notification history and the destination catalogue",
        "Settings > Notifications",
        &[
            any("/api/v1/notifications/deliveries"),
            any("/api/v1/notifications/destinations"),
        ],
    ),
    covered(
        "Clearing the notification history",
        "Settings > Notifications",
        &[any("/api/v1/notifications/deliveries/clear")],
    ),
    covered(
        "Subscription items, runs and forced polls",
        "Subscriptions",
        &[
            any("/api/v1/subscriptions/items"),
            any("/api/v1/subscriptions/review-summary"),
            any("/api/v1/subscriptions/{id}/disable"),
            any("/api/v1/subscriptions/{id}/enable"),
            any("/api/v1/subscriptions/{id}/history"),
            any("/api/v1/subscriptions/{id}/items"),
            any("/api/v1/subscriptions/{id}/poll"),
            any("/api/v1/subscriptions/{id}/runs"),
        ],
    ),
    covered(
        "Stream schedules, runs and recording now",
        "Streams",
        &[
            any("/api/v1/streams/schedules"),
            any("/api/v1/streams/runs"),
            any("/api/v1/streams/record"),
        ],
    ),
    covered(
        "The diagnostic bundle: preview",
        "Logs",
        &[any("/api/v1/diagnostics/bundle/preview")],
    ),
    covered("Metrics", "-", &[any("/api/v1/metrics")]),
    covered(
        "Reconnect status",
        "Settings > Network",
        &[only("/api/v1/reconnect", "GET")],
    ),
    covered(
        "The hosters one account covers",
        "Settings > Accounts",
        &[any("/api/v1/accounts/{id}/hosters")],
    ),
    covered(
        "Trying a routing regular expression",
        "Settings > Routing",
        &[any("/api/v1/category-rules/test-regex")],
    ),
];

/// The capability a `(path, method)` belongs to, by longest claim.
///
/// Returns `None` only for an operation no capability claims, which the tests below do not allow
/// to exist.
pub(crate) fn capability_for(path: &str, method: &Method) -> Option<&'static Capability> {
    let mut best: Option<(&'static Capability, usize, bool)> = None;
    for capability in COVERAGE {
        for claim in capability.claims {
            if !path.starts_with(claim.prefix) {
                continue;
            }
            let specific = match claim.method {
                Some(claimed) => {
                    if claimed != method.as_str() {
                        continue;
                    }
                    true
                }
                None => false,
            };
            let better = match best {
                None => true,
                Some((_, length, was_specific)) => {
                    (claim.prefix.len(), specific) > (length, was_specific)
                }
            };
            if better {
                best = Some((capability, claim.prefix.len(), specific));
            }
        }
    }
    best.map(|(capability, _, _)| capability)
}

/// The tools that reach a capability, read from [`super::TOOL_POLICY`] and
/// [`super::TOOL_ALSO_REACHES`] rather than written down a second time.
///
/// `list_configuration` is one tool over six routes, so its sections are folded in here too;
/// otherwise the provider table would read as uncovered while a tool plainly lists it.
pub(crate) fn tools_for(capability: &Capability) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    let mut note = |name: &'static str, path: &str, method: &Method| {
        if let Some(found) = capability_for(path, method)
            && std::ptr::eq(found, capability)
            && !names.contains(&name)
        {
            names.push(name);
        }
    };
    for entry in super::TOOL_POLICY.iter().chain(super::TOOL_ALSO_REACHES) {
        note(entry.tool, entry.path, &entry.method);
    }
    for &(_, path) in super::CONFIG_SECTION_ROUTES {
        note("list_configuration", path, &Method::GET);
    }
    names.sort_unstable();
    names
}

/// The markers the generated table sits between in `docs/mcp-coverage.md`.
const BEGIN: &str = "<!-- BEGIN generated: scripts/mcp-coverage.sh -->";
const END: &str = "<!-- END generated -->";

/// The comparison, as `docs/mcp-coverage.md` carries it.
///
/// Generated rather than written, because a hand-kept table is wrong by the time it is
/// committed. Every column comes from a source that is itself checked: the operation counts
/// from [`crate::openapi_document`], the tool names from [`super::TOOL_POLICY`], the decision
/// and its reason from [`COVERAGE`].
fn markdown(documented: &[(String, Method)]) -> String {
    let mut out = String::new();
    let counted = |capability: &Capability| -> usize {
        documented
            .iter()
            .filter(|(path, method)| {
                capability_for(path, method).is_some_and(|found| std::ptr::eq(found, capability))
            })
            .count()
    };

    let covered_count = COVERAGE
        .iter()
        .filter(|capability| capability.decision == Decision::Covered)
        .count();
    let owner_count = COVERAGE
        .iter()
        .filter(|capability| capability.decision == Decision::Omitted(OWNER_LINE))
        .count();
    out.push_str(&format!(
        "{BEGIN}\n\n**{} capabilities, {} covered by a tool, {} deliberately out ({} of them on \
         the owner's line of 2026-09-23).** {} REST operations, {} MCP tools. Regenerate \
         with `scripts/mcp-coverage.sh`; `mcp::coverage` fails the build if an operation \
         belongs to no capability.\n\n",
        COVERAGE.len(),
        covered_count,
        COVERAGE.len() - covered_count,
        owner_count,
        documented.len(),
        super::TOOL_POLICY.len(),
    ));

    out.push_str("### Covered\n\n| Capability | Surface | REST ops | MCP tools |\n| --- | --- | --: | --- |\n");
    for capability in COVERAGE {
        if capability.decision != Decision::Covered {
            continue;
        }
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            capability.name,
            capability.surface,
            counted(capability),
            tools_for(capability)
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }

    out.push_str("\n### Deliberately out\n\n| Capability | Surface | REST ops | Why not |\n| --- | --- | --: | --- |\n");
    for capability in COVERAGE {
        let Decision::Omitted(why) = capability.decision else {
            continue;
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            capability.name,
            capability.surface,
            counted(capability),
            why.split_whitespace().collect::<Vec<_>>().join(" "),
        ));
    }
    out.push_str(&format!("\n{END}"));
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{COVERAGE, Decision, Method, OWNER_LINE, capability_for, tools_for};

    /// Every `(path, method)` the OpenAPI document carries.
    ///
    /// Read out of the serialised document for the same reason `scope_policy` reads it there:
    /// the JSON is the contract, and it does not move when utoipa reshapes its types.
    pub(super) fn documented() -> BTreeSet<(String, Method)> {
        let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
        let paths = document
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("the document has paths");
        let mut operations = BTreeSet::new();
        for (path, item) in paths {
            for method in item.as_object().expect("path item").keys() {
                let Ok(method) = Method::from_bytes(method.to_uppercase().as_bytes()) else {
                    continue;
                };
                operations.insert((path.clone(), method));
            }
        }
        assert!(
            operations.len() > 200,
            "only {} operations were found; the document did not serialise as expected",
            operations.len()
        );
        operations
    }

    /// No gap stays uncommented, and the build is where that is enforced.
    ///
    /// This is the whole point of the table. A coverage answer written once decays the moment
    /// somebody adds a route; a route that falls into no capability fails here instead, so
    /// adding one without saying whether the toolbox should have it is not possible.
    #[test]
    fn every_documented_operation_belongs_to_a_capability() {
        let orphans: Vec<String> = documented()
            .into_iter()
            .filter(|(path, method)| capability_for(path, method).is_none())
            .map(|(path, method)| format!("{method} {path}"))
            .collect();
        assert!(
            orphans.is_empty(),
            "these operations belong to no capability, so nothing says whether MCP should \
             cover them:\n  {}",
            orphans.join("\n  ")
        );
    }

    /// And a capability that claims nothing real is a decision about something that is gone.
    #[test]
    fn every_capability_claims_at_least_one_operation() {
        let documented = documented();
        let empty: Vec<&str> = COVERAGE
            .iter()
            .filter(|capability| {
                !documented.iter().any(|(path, method)| {
                    capability_for(path, method)
                        .is_some_and(|found| std::ptr::eq(found, *capability))
                })
            })
            .map(|capability| capability.name)
            .collect();
        assert!(
            empty.is_empty(),
            "these capabilities match no operation the API has: {empty:?}"
        );
    }

    /// `Covered` means a tool exists, and `Omitted` means none does — checked, not asserted.
    ///
    /// Written this way round on purpose: the decision is the thing a person reads, and it is
    /// held against `TOOL_POLICY`, which is held against `scope_policy`, which is held against
    /// the document. A tool quietly added to an omitted capability fails here rather than
    /// leaving the reason standing as a lie.
    #[test]
    fn the_decision_and_the_tools_agree() {
        for capability in COVERAGE {
            let tools = tools_for(capability);
            match capability.decision {
                Decision::Covered => assert!(
                    !tools.is_empty(),
                    "{} is marked covered but no tool reaches it",
                    capability.name
                ),
                Decision::Omitted(why) => {
                    assert!(
                        tools.is_empty(),
                        "{} is marked deliberately out but these tools reach it: {tools:?}",
                        capability.name
                    );
                    assert!(
                        why.len() > 40,
                        "{} is left out without a reason worth reading",
                        capability.name
                    );
                }
            }
        }
    }

    /// Two capabilities claiming one operation equally would make the winner arbitrary.
    #[test]
    fn no_two_capabilities_claim_the_same_route_the_same_way() {
        let mut seen: Vec<(&str, Option<&str>)> = Vec::new();
        for capability in COVERAGE {
            for claim in capability.claims {
                let key = (claim.prefix, claim.method);
                assert!(
                    !seen.contains(&key),
                    "{} claims {} {:?}, which another capability already claims",
                    capability.name,
                    claim.prefix,
                    claim.method
                );
                seen.push(key);
            }
        }
    }

    /// The findings the job reports, pinned so a later change has to face them.
    #[test]
    fn the_capabilities_rd_120_29_decided_stay_decided() {
        let by_name = |name: &str| {
            COVERAGE
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("no capability called {name}"))
        };
        assert_eq!(by_name("Remote jobs").decision, Decision::Covered);
        assert_eq!(by_name("Transfer statistics").decision, Decision::Covered);
        assert_eq!(by_name("The log store").decision, Decision::Covered);
        assert_eq!(by_name("The audit log").decision, Decision::Covered);
        assert_eq!(
            by_name("Clearing logs, audit records and statistics").decision,
            Decision::Covered
        );
        assert_eq!(
            by_name("Site rules: read and switch").decision,
            Decision::Covered
        );
        assert!(matches!(
            by_name("Deleting a remote job at the provider").decision,
            Decision::Omitted(_)
        ));
        // Out under RD-120-29, in since RD-120-31 gave the import routes a JSON body.
        assert_eq!(
            by_name("Handing in a container file").decision,
            Decision::Covered
        );
    }

    /// RD-120-32's three sorts, pinned: group 1 and 2 in, the owner's nine out on his line.
    #[test]
    fn the_capabilities_rd_120_32_decided_stay_decided() {
        let by_name = |name: &str| {
            COVERAGE
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("no capability called {name}"))
        };
        for name in [
            "LinkGrabber: candidate-level handling",
            "Mirror groups",
            "Reviewing an NZB before it is queued",
            "NZB import files and enqueue",
            "LinkGrabber: package editing and ordering",
            "Ordering the queue by hand",
            "Renaming and retargeting queued work",
            "Clearing finished work in one sweep",
            "Unpacking on demand",
            "Torrent detail and seeding",
            "Post-processing inventory and queue",
            "Managed external tools",
            "Storage capacity",
            "Writing a site rule",
        ] {
            assert_eq!(by_name(name).decision, Decision::Covered, "{name}");
        }
        let owner: Vec<&str> = COVERAGE
            .iter()
            .filter(|capability| capability.decision == Decision::Omitted(OWNER_LINE))
            .map(|capability| capability.name)
            .collect();
        assert_eq!(
            owner,
            [
                "Deleting a remote job at the provider",
                "Signing in, sessions, second factor and API tokens",
                "Signing in at a provider",
                "Trying a stored credential or destination",
                "Remote logins and trusted host keys",
                "Solving captchas",
                "Consent to replay a paid link",
                "Import and export of a whole area",
                "Plugin trust and installation",
                // RD-120-55: the parts of three of the thirteen that meet one of the marks.
                "Probing an indexer's capabilities",
                "Approving and fetching a diagnostic bundle",
                "Reconnecting on demand",
            ],
            "the owner decided nine capabilities on 2026-09-23, and RD-120-55 applied the same \
             line to three more, with one reason for all of them"
        );
    }

    /// RD-120-55's verdicts, pinned: the thirteen are in, except the parts that meet a mark.
    #[test]
    fn the_capabilities_rd_120_55_decided_stay_decided() {
        let by_name = |name: &str| {
            COVERAGE
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("no capability called {name}"))
        };
        for name in [
            "Which providers can take a remote job",
            "Power actions",
            "Plugin execution history",
            "Plugin message catalogues",
            "Automation history, vocabulary and dry run",
            "Notification history and the destination catalogue",
            "Subscription items, runs and forced polls",
            "Stream schedules, runs and recording now",
            "The diagnostic bundle: preview",
            "Metrics",
            "Reconnect status",
            "The hosters one account covers",
            "Trying a routing regular expression",
        ] {
            assert_eq!(by_name(name).decision, Decision::Covered, "{name}");
        }
        for name in [
            "Probing an indexer's capabilities",
            "Approving and fetching a diagnostic bundle",
            "Reconnecting on demand",
        ] {
            assert_eq!(
                by_name(name).decision,
                Decision::Omitted(OWNER_LINE),
                "{name}"
            );
        }
        // Not a mark but a route no tool can be priced by; the reason says which tool answers.
        assert!(matches!(
            by_name("The health probe").decision,
            Decision::Omitted(why) if why != OWNER_LINE
        ));
    }
}

/// The table in `docs/mcp-coverage.md` and this module are one thing described twice; keep them
/// equal.
///
/// Same arrangement as `rd_core::failpoint` and `docs/recovery-matrix.md`, and for the same
/// reason: a comparison table that has drifted from the code reads as a statement about what
/// is covered, and a reader has no way to tell it stopped being true. The table is generated
/// by [`markdown`] and spliced in by `scripts/mcp-coverage.sh`; this fails the build when the
/// two disagree, naming the script rather than asking anyone to edit the table by hand.
#[cfg(test)]
mod doc_tests {
    use super::{BEGIN, COVERAGE, Decision, END, markdown};

    const DOC_FILE: &str = "docs/mcp-coverage.md";
    const DOC: &str = include_str!("../../../../docs/mcp-coverage.md");

    fn generated() -> String {
        markdown(&super::tests::documented().into_iter().collect::<Vec<_>>())
    }

    fn block(text: &str) -> &str {
        let start = text.find(BEGIN).expect("the page has a BEGIN marker");
        let end = text.find(END).expect("the page has an END marker") + END.len();
        &text[start..end]
    }

    #[test]
    fn the_doc_carries_the_generated_table() {
        let generated = generated();
        let normalised = DOC.replace("\r\n", "\n");
        assert_eq!(
            block(&normalised),
            generated.trim_end(),
            "{DOC_FILE} is out of date; run scripts/mcp-coverage.sh"
        );
    }

    /// Every decision is readable on the page, not only in this source.
    ///
    /// The deliverable the owner asked for is the decision per gap, and the place it is read is
    /// the page. A reason that exists only as a Rust doc comment is not delivered.
    #[test]
    fn every_reason_reaches_the_doc() {
        for capability in COVERAGE {
            let Decision::Omitted(why) = capability.decision else {
                continue;
            };
            let one_line: String = why.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                DOC.contains(&one_line),
                "the reason for leaving out {} is not in {DOC_FILE}",
                capability.name
            );
        }
    }

    /// Writes the table into the page. Run through `scripts/mcp-coverage.sh`, never in CI.
    #[test]
    #[ignore = "rewrites docs/mcp-coverage.md; scripts/mcp-coverage.sh runs it"]
    fn write_the_doc_table() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DOC_FILE);
        let current = std::fs::read_to_string(&path).expect("the page is readable");
        let updated = current.replace(block(&current), generated().trim_end());
        std::fs::write(&path, updated).expect("the page is writable");
        println!("wrote the comparison into {DOC_FILE}");
    }
}

# Documentation

Every document in this repository, with what it is for. The documentation lives next to the
code on purpose: a change that alters behaviour updates the page that describes it in the same
commit, which a wiki kept elsewhere cannot promise. There *is* a wiki — the
[user handbook](https://github.com/degoya/rDownloader/wiki), for people who use and operate
rDownloader rather than change it — and because it cannot make that promise per commit, it makes
a smaller one per tag: every tagged release brings it up to that release. [`README.md`](../README.md) at the root is
the overview; [`CHANGELOG.md`](../CHANGELOG.md) records what changed and when.

## Running it

| Document | What it covers |
| --- | --- |
| [`README.md`](../README.md) | The public front page: what rDownloader is, the supported sources, the quick start, the platforms |
| [`development.md`](development.md) | The features in detail, running from source, building for every platform, provider-account setup for the bundled plugins, portable start and autostart, the capture agent, `rdownloader://` links, the CLI client, the automation-client adapters, the MCP server, metrics and the quality checks |
| [`docker/README.md`](../docker/README.md) | The container image, Compose, the Synology walkthrough, the volume rules |
| [`reverse-proxy.md`](reverse-proxy.md) | Running behind nginx, Caddy or Traefik, and the forwarded-header contract |
| [`external-tools.md`](external-tools.md) | `yt-dlp`, `ffmpeg` and the other helper binaries: the tested versions, the managed downloads, the platform differences |
| [`postprocessing.md`](postprocessing.md) | Repair, unpack and cleanup levels, user scripts, and what was and was not taken over from SABnzbd |
| [`auth-profiles.md`](auth-profiles.md) | Authentication profiles: which credential reaches which domain, and where the boundaries are |
| [`observability.md`](observability.md) | The Prometheus metrics: the scrape scope, every family and its labels, why the label set is closed, the persistent transfer statistics with their retention, the append-only audit log, and the trace context with its optional OTLP export |
| [`compatibility.md`](compatibility.md) | The SABnzbd and qBittorrent adapters for Sonarr, Radarr and similar tools, endpoint by endpoint |
| [`diagnostics.md`](diagnostics.md) | The structured log store and its viewer, what is redacted before storage, retention, and the user-approved diagnostic bundle |
| [`accessibility.md`](accessibility.md) | The WCAG 2.2 AA target, what was verified and how, and what was not |
| [`../SECURITY.md`](../SECURITY.md) | Reporting a vulnerability, the supported versions, and the things that look like findings but are documented behaviour |
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | How the public repository relates to the development repository, how a pull request is applied and credited, and the conventions a contribution meets |
| [`../CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) | The Contributor Covenant 2.1 and how to report a violation |

## Extending it

| Document | What it covers |
| --- | --- |
| [`plugins.md`](plugins.md) | The plugin package: manifest field by field, every plugin type, capabilities, keys and trust, packaging, conformance, distribution |
| [`site-rules.md`](site-rules.md) | The rules that recognise release pages: the signed release file and its import, every field of a rule with a commented example, what refuses one, duplicating a rule and how the file is signed |
| [`../sdk/README.md`](../sdk/README.md) | Writing a plugin: the scaffolds, the toolchain, translations, the CI workflow template |
| [`../extension/README.md`](../extension/README.md) | The browser extension: what it captures, how it is built and packaged |
| [`../scripts/README.md`](../scripts/README.md) | The build, check, packaging, release and worktree scripts, and the machine limits they encode |

## How it is built

| Document | What it covers |
| --- | --- |
| [`../design.md`](../design.md) | The visual language, the interaction rules and the row and list conventions of the interface |
| [`architecture.md`](architecture.md) | One section per crate with its job history, and the build, test, verification and documentation rules with the reasoning behind each |
| [`adr/`](adr/README.md) | Architecture decision records, one per decision that was hard to reverse |
| [`recovery-matrix.md`](recovery-matrix.md) | What a crash or restart in the middle of an operation is proven to leave intact, and the test that proves each row |
| [`feature-list.md`](feature-list.md) | The complete feature and technology inventory |
| [`mcp-coverage.md`](mcp-coverage.md) | What the interface can do against what the MCP toolbox can do, generated from the source, with the reason for every capability left out |
| [`../testfile/README.md`](../testfile/README.md) | Test files with a documented origin for manual download smoke tests (German) |

## Where it is going

| Document | What it covers |
| --- | --- |
| [`../ROADMAP.md`](../ROADMAP.md) | The coming milestones in short — a plan, not a promise |

The public repository is an export of each release; development happens in a private one. What
the export leaves out is the working material of that development: the detailed roadmap with
one planning file per job, the working card for coding agents, the audits of release 1.0.8, an
early specification snapshot and loose notes. The positioning, website structure and launch
content for the project site live with the site itself, in the website repository
(`docs/rdownloader-marketing-website-package/` there).

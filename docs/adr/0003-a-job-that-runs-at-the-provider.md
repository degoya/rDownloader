# ADR 0003 — A job that runs at the provider

- **Status:** Accepted
- **Date:** 2026-09-15
- **Job:** RD-107-06
- **Supersedes:** —

## Context

RD-106-03 delivered Real-Debrid's hoster half — the device sign-in, the account status, the
unrestriction — and stopped in front of the other half. A magnet handed to Real-Debrid does not
answer with a file. It answers with an `id`, and behind that id runs something of the provider's
own: `magnet_conversion` → `waiting_files_selection` → `queued` → `downloading` → `downloaded`.
It takes minutes on a good day and hours on a bad one, it stops half-way through and refuses to
continue until a person has said which files they want, and the account keeps it afterwards
whether anybody wanted that or not.

The obvious home for it was the tenth world. RD-107-06 ruled that out before this record was
written, and the reasons are restated here because the rest of the argument stands on them:

1. **`crawl` is one call under one fuel and time budget.** A crawler that waited for a torrent
   would spend its budget and be stopped — on every attempt, for ever.
2. **The file selection needs a person between two calls.** `torrents/selectFiles` comes
   strictly before `torrents/info` carries any link at all. `crawl` has no gap in which anybody
   could be asked.
3. **`torrents/addMagnet` is not idempotent.** It answers with a *new* id every time. A crawler
   re-run after a restart or a timeout leaves a second torrent in somebody's account.
4. **Deleting is an act, not a side effect.** `torrents/delete` has to be confirmed, and a
   crawler has no surface on which to ask.

What is left is the finding underneath, and it is not about Real-Debrid:

**No world in `rdownloader:plugin` carries a job that outlives the call that started it.**
`resolver` answers with one file. `crawler` answers with a list, in one call. `intake` reads a
document that was already in hand. `transfer` carries bytes for as long as a download lasts and
dies with it. `auth` and `oauth` come closest — they poll, they survive restarts, they ask a
person something half-way through — but what they carry is a sign-in, keyed one-per-account, with
no artifacts at the end and nothing to delete at the provider.

Premiumize's `/transfer/create`, AllDebrid's `/magnet/upload` and Debrid-Link's `/seedbox/add`
are the same mechanism with different spelling: hand over a magnet, get an id, poll a status,
end with a list of links. Three more provider jobs stand behind this one, and they would each
arrive at the same wall.

So the decision is not "how do we add a magnet to Real-Debrid". It is: **where does a job that
runs at somebody else's provider live, from now on — what part of it is the plugin's and what
part is the host's.**

## Options

### 1. An eleventh world, `remote-job`, with the durable state in the host — chosen

`interface remote-job` and `world remote-job-plugin`: seven short, individually bounded calls —
`claims`, `identify`, `submit`, `adopt`, `poll`, `choose`, `discard` — none of which waits for
anything. The plugin knows the provider's API and nothing else. Everything that has to *last* —
the row, the remote id, the clock, the person's answer, the restart — belongs to the host.

- **It is the only split in which no call has to wait.** Every function here returns on the
  provider's next response. The waiting lives in `next_poll_at` on a database row, which costs
  no fuel, survives a restart, and is the same mechanism `auth_flows` has used since RD-090-13.
  A world whose calls are short can keep the fuel and timeout limits that make the sandbox
  worth having; a world that had to wait could not.
- **It is the only split in which the duplicate is preventable.** The reason `addMagnet` is
  dangerous is that the caller cannot tell "I already did this" from "I have not done this
  yet". The plugin cannot answer that — a guest is instantiated fresh for every call and
  remembers nothing — so the answer has to be written down somewhere that survives, which is
  the host. `identify` gives the host a content key derived locally, without a request;
  `UNIQUE(account_id, content_key)` on the row makes a second submit of the same magnet on the
  same account impossible before any network call happens; and `adopt` closes the one window
  the row cannot — a crash between the request going out and the id coming back — by asking the
  provider what it already holds for that key instead of guessing.
- **Asking a person is a state, not a callback.** `awaiting-choice` is a value `poll` returns,
  the host writes it to the row, and the interface reads it. The plugin is not blocked while
  somebody decides, because nothing of the plugin is running; the next thing that happens is a
  `choose` call, minutes or days later, into a fresh instance. This is exactly how
  `auth-state::user-action` already works, and it works for the same reason.
- **Deleting is a separate function and nothing calls it by itself.** `discard` exists, and the
  host calls it only from an explicit, confirmed request. Removing the local package does not
  reach it. A provider job that nobody deleted is still in the account, which is the correct
  outcome: rDownloader did not put it there on its own and does not take it away on its own.
- **It generalises without being generic.** Nothing in the interface says "torrent". A source
  is a magnet or a container of bytes, the states are the five things any long remote job can
  be in, and the artifacts at the end are addresses. Premiumize, AllDebrid and Debrid-Link fit
  it without a contract change — each is one more plugin, not one more world.
- Costs an interface, a world, ten byte-identical copies under `sdk/templates/*/wit/`, one
  `PluginType`, one table, and one sweep in the host.

### 2. Native code in the host, one module per provider — rejected

The fastest route to a working Real-Debrid torrent, and the same mistake the plugin platform
was built in 0.7 to stop making. Each further provider becomes a compile-time decision and a
release; `rd-provider-registry`'s rule that nothing is compiled in — a provider exists exactly
while its plugin does — would hold for every provider except the four that matter most here.
It also puts a client for four third-party APIs inside the process instead of inside the
sandbox, for the one operation that hands a stranger's identifier to somebody's paid account.

Worth naming honestly, because it will look attractive again: the *state machine* really does
belong in the host, and this option is right about that. What it gets wrong is taking the
provider's API with it.

### 3. Extend `crawler` into a multi-call protocol — rejected

Give `crawler` a second entry point so a crawl can answer "not yet, ask me again" and carry a
token forward. On paper the smallest contract change, since a torrent does end as a list of
files, which is what a crawler returns.

It fails on all four of the grounds RD-107-06 already set out, and on a fifth. Changing what
`crawl` may return changes the world every crawler satisfies, so **every `.rdplug` of type
`crawler` already signed and installed stops instantiating** — six of them are bundled and
third-party ones cannot be rebuilt by this project. That is precisely the class of change ADR
0001 reserved for a version bump, and paying it to make a folder lister into something that is
not a folder lister is the worst available trade.

There is also a plainer objection. A crawler is *safe to retry*: it reads. A remote job is not:
it creates something in an account and can be charged for. Folding the two together would mean
the host could no longer tell, from the type alone, whether re-running something was free.

### 4. A `transfer` backend, with the provider as the protocol — rejected

`transfer-plugin` already owns something long-lived: `run` carries bytes for the whole life of
a download, reports progress and can be resumed. Making `magnet:` a scheme and Real-Debrid a
backend is a real design, not a strawman.

Three things break. A transfer's lifetime is a *download's* lifetime — the queue starts it,
watches it, and stops it — whereas a remote job exists before any download does and keeps
existing after every download it produced has finished. A transfer has a sink, a file it is
writing; what the first hours of a remote job produce is nothing at all. And a transfer that
was cancelled is simply stopped, while a remote job that is cancelled leaves a torrent in an
account that only an explicit `discard` removes. Encoding "ask a person which files" in a
`transfer` would also mean inventing a second selection mechanism inside a world that has no
vocabulary for one.

### 5. A pending state in `resolver` — rejected

`resolve` a magnet, answer `transient(seconds)` while the provider works, answer with the file
when it is done. Zero contract change: the machinery to retry a transient failure on a timer
already exists in the scheduler.

It is the option that would quietly do the most damage. A `transient` failure means *try the
same call again*, and trying `resolve(magnet)` again is another `addMagnet` — the duplicate
this whole record exists to prevent, delivered by the retry path rather than by a crawler.
There is nowhere to keep the remote id between attempts, nowhere to put the file list, and no
way to ask anybody anything. It would work in a demo and leave a hundred dead torrents in a
real account.

## Decision

**Option 1.** `interface remote-job` and `world remote-job-plugin` join `rdownloader:plugin`,
with `plugin_type = "remote-job"` as the eleventh type. The durable state is the host's, in one
table with one row per remote job.

Four properties are part of the decision rather than of the implementation:

- **The remote id is written before anything else happens, and the content key before that.**
  The order is the whole idempotency argument, so it is stated as a rule and not left to a
  sequence of statements: the host writes a row holding the account, the plugin, the source and
  `identify`'s content key *before* it calls `submit`, and writes the id `submit` answers with
  as the very next thing it does. A unique index on `(account_id, content_key)` means a second
  attempt at the same magnet on the same account cannot become a second row, and a row in
  `submitting` that comes back from a restart is offered to `adopt` before it is ever offered to
  `submit` again. If `adopt` finds nothing and the second `submit` also leaves no id, the row
  fails with a code that tells the person to look at their account, rather than trying a third
  time. Two attempts can be reasoned about; an unbounded retry against a non-idempotent
  endpoint cannot.
- **The plugin owns the provider, the host owns the clock.** `preparing` and `working` may
  carry a suggested wait, and the host treats it as a suggestion — it clamps it into its own
  bounds and it is the host that decides when the next `poll` happens. A plugin that could set
  the interval could spend an account's whole request budget, which for Real-Debrid is 250 a
  minute shared with the resolver that is unrestricting the links this very job produced.
- **`awaiting-choice` is a question, and a question that is not answered stays open.** The host
  does not choose everything, does not choose the largest file, and does not time the person
  out into a default. The one thing it does on their behalf is refuse to call `choose` with an
  empty list, because at Real-Debrid that is not "select nothing" but an error, and at other
  providers it silently means "select all" — two different wrong answers to a question nobody
  asked.
- **Nothing deletes at the provider implicitly.** `discard` is reached from one explicit,
  confirmed request and from no other path: not from removing the local package, not from a
  failed job, not from a cleanup sweep. What a `discard` did is recorded, so an account that
  lost a torrent can be shown which request removed it.

## `api_version` stays at `0.6.0`

The same answer ADR 0001 gave and ADR 0002 confirmed, for the same reason: this is additive.
A new interface and a new world take nothing away from an existing world, so every plugin built
against the contract without `remote-job` in it still satisfies the world it declares. No
exported function's signature changes, no record loses a field, nothing is renamed — which is
the list ADR 0001 reserved a bump for. In the other direction an older host refuses a
`remote-job` package outright: `PluginType::RemoteJob` does not deserialize there, so the
manifest is rejected as an unknown plugin type rather than half-loaded.

## Consequences

- The eleventh `plugin_type` is `remote-job`, validated as the other extension types are: an
  `[extension]` section, no `[provider]`, no `[transfer]`, and at least one
  `capabilities.net_http` domain — a remote job that cannot reach its provider is a manifest
  mistake and not a plugin that submits to nowhere.
- `[extension] claims` names the **provider slug** whose account the job runs on, exactly as a
  crawler's does. The plugin never learns which account it is; its requests carry
  `{{secret:<reference>}}` and the host expands them towards the declared domains and nowhere
  else.
- A remote job's artifacts are *addresses*, not files. At Real-Debrid they are still restricted
  links, so what the remote job hands over goes to the resolver that already exists — the
  `remote-job` plugin and the `realdebrid` resolver are siblings, and the second unrestricts
  what the first produced. That is why `ready` returns URLs and not bytes, and it is what keeps
  the finished download an ordinary resumable queue job.
- Premiumize, AllDebrid and Debrid-Link each become one more plugin against this world and no
  contract change. That is the test this record was written to pass, and the reason it was
  written before the Real-Debrid implementation rather than after it.
- What is **not** decided here: whether a remote job may be started by anything other than a
  person pasting a magnet — a subscription, a hot folder, an automation action. The mechanism
  does not forbid it; nothing in this job wires it up, and the place to decide is the intake
  path, not the contract.
- **Addendum, RD-130-11 (2026-09-25):** `check-cached` is the one function of this world that
  writes no row. It asks whether the provider holds a source ready *before* anybody hands it
  over, and it is read-only at the provider: it creates nothing, so there is nothing to make
  idempotent, nothing to adopt after a crash and nothing to discard. The argument above is about
  calls that change an account; this one changes none, and `submit` stays the only call that
  puts anything in one. Its companion `cache-kinds` reaches nothing at all, like `claims`.
- What is **not** claimed here: a run against a real Real-Debrid account. There is none in this
  checkout. The evidence is the unit and contract level, and the acceptance criterion that asks
  for end to end is recorded as unsatisfied rather than as satisfied by a mock.

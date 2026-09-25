# ADR 0017 — The rule format keeps its two measured limits: no negative filter, no loop

- **Status:** Accepted
- **Date:** 2026-09-22
- **Job:** RD-120-12 (RD-110-35 until 2026-09-22)
- **Supersedes:** — (ADR 0013 stands; this record refines the first of its reopening conditions)

## Context

RD-110-04 and RD-110-05 defined the site-rule format and its executor: seven step kinds, three
bolts, stable refusal codes, and a leaf crate that takes four small traits from its caller.
Measuring real pages then produced two limits, each behind a page somebody actually fetched:

1. **A rule cannot say "this host, no."** There is no filter step, and the Rust `regex` crate has
   no lookahead, so a negative host pattern is not available either. Found at `scene-rls.net`
   (RD-110-10).
2. **A rule cannot walk a list.** `Variables::expand` (`crates/rd-siterules/src/exec/value.rs`)
   replaces `${name}` with `Value::first`, and `regex.pattern` is a literal rather than a
   template. Found at `serienjunkies.org` (RD-110-13, ADR 0013).

Both were written down as open questions for whatever succeeded RD-110-04, in
`docs/site-rules.md` and in ADR 0013's consequences. This record is that decision. It is about
the *format*, not about any one service: ADR 0013 decided `serienjunkies.org`, and that decision
is unchanged.

Nothing below asks for a JavaScript interpreter or a browser. That line is drawn in `AGENTS.md`,
in `docs/site-rules.md` and in ADR 0010 and ADR 0012, and this record does not reopen it.

## Decision

### A. No filter step. The limit is narrower than it was recorded, and the real part is covered.

**The page that produced the limit no longer says what it was recorded as saying.** RD-110-10
read `scene-rls.net` as mixing the site's own links into the hoster block. RD-110-11 re-measured
it over eight release pages and that did **not** hold — the centred `h2` carries hoster addresses
only. What holds is weaker and was measured across ten containers: exactly two of them carry
exactly one address that is not a file. GetComics puts a READ-ONLINE button in its
`aio-button-center` block, and one scene-rls page of eight put the site's own NFO viewer in the
centred `h2`. A format change is being asked for on behalf of two stray addresses.

**The positive form of the filter is already in the format, and rules already use it.** `regex`
with `all` takes only what its pattern matches, so a pattern that names the accepted hosts in an
alternation *is* a keep-filter, and `satdl` already constrains a pattern to one host
(`https?://satdl\.com/product/[0-9]+/…`). What the format lacks is only the *negative* form —
"every address except this host" — which Rust `regex` cannot express without lookahead. The
negative form is also the weaker of the two: naming the hosts a rule accepts is a statement a
reviewer can check against the page, while naming the one it rejects silently accepts everything
nobody thought of.

This is not a complete answer and should not be read as one. A release board links to whatever
hoster the uploader chose, so an exhaustive alternation would be brittle there. For those pages
the container stays the filter and the crawl verdict stays the net — which is the division of
labour `docs/site-rules.md` already describes.

**A `drop`/`keep` step would take the stray address out of sight, and the sight is the point.**
Since RD-110-07 such an address is fetched, seen to answer with a page, refused with
`collector.crawl_not_a_file` and **counted**: the person reads "1 of 6 found links was not a
file". That count is also how a changed page announces itself. A rule-level drop removes the
address and the announcement together, and the day the drop pattern matches a hoster link instead
of the NFO viewer, nothing says so — the package is simply one link short. A positive `regex`
cannot fail that way: a pattern that stops matching refuses the whole run with
`site_rules.structure`, which is exactly the verdict RD-110-09 built.

**An eighth step kind is permanent, signed surface.** The step kinds are `format_version` 1 and
they travel in a signed pack. The format's value is that it is small enough for one person to
read and audit in an afternoon. Growing it for a case no measured page needs spends that.

### B. No loop step, deferred against a named condition.

**Its only measured page does not become reachable.** ADR 0013 measured that
`serienjunkies.org` wants a browser fingerprint in the request body — `fphash` is
`Fingerprint2.x64hash128(…)` over canvas, WebGL, fonts, audio, screen, timezone and plugins —
and lists that as a *separate* reopening condition beside the loop. Build the loop and the page
is still refused, for a reason ADR 0010 and ADR 0012 already decided not to fake. The loop would
ship without delivering the page that asked for it.

**The existing budget does not fit the shape anyway.** `Limits::max_pages` is 24 and `max_depth`
is 6. The one measured list is 39 releases and 78 release-and-hoster pairs; a loop issuing one
request per element refuses at the 24th. So the loop is not one step but a step *plus* a decision
about how much of this process one crawl may occupy — and there is no evidence behind that second
decision either, because there is no page to measure it against.

**Cheap to add is an argument for adding it later, not now.** `Step::Redirect`
(`crates/rd-siterules/src/exec/steps.rs`) already walks a list and issues one request per element,
so the machinery exists; the job file and ADR 0013 both say so and both are right. That cuts both
ways. What is small to write when a page needs it does not have to be written before one does.

**What it would actually cost is the contract, not the code.** A loop needs a body with its own
variable scope, an iteration ceiling of its own beside the run's, a stated answer on nesting, and
`regex.pattern` promoted from a literal to a template — which means `check_template` after
expansion rather than `check_pattern` at load, i.e. a pattern that is only known to compile at run
time, which is a new class of run-time refusal. And it still would not give "one package per
item": `package` is one source read once, the third thing ADR 0013 lists, and a loop does not
touch it.

## Consequences

- **The format does not change.** `FORMAT_VERSION` stays 1, the seven step kinds stay seven, and
  `crates/rd-siterules/resources/site-rules.json` is untouched and keeps its signature —
  `tests/embedded_pack.rs` is unaffected. No code was written for this decision.
- **Migration `0088`, reserved for this job in case stored user rules needed a format version,
  stays unused.** A gap is the convention here.
- **`docs/site-rules.md` states both limits as decided rather than open**, under "Writing a rule
  for a real page", so the next rule writer meets a decision instead of a question.
- **ADR 0013 stands unedited.** Its reopening condition 1 — "the rule format gains a step that
  repeats other steps for every value of a list, and a way to carry a value into a pattern" —
  remains correct; this record only says that the format will not gain them on that page's
  account, because that page needs condition 2 as well.
- **No new refusal code**, because nothing was shipped that can fail.

### What reopens each

**A.** A page, measured and not supposed, whose own links sit inside the smallest container that
can be cut around the hoster links, in numbers rather than as one stray, and whose accepted hosts
cannot be named in advance because the page links to arbitrary hosters. Then the step to add is
`keep` — an allow pattern over a list — and not `drop`, for the reason above: a rule should say
what it wants, and a pattern that suddenly matches nothing should refuse loudly.

**B.** A page, measured and not supposed, that needs one request per element of a list and
nothing else this format refuses: no browser fingerprint, no managed challenge, no JavaScript
program. Its element count has to fit `max_pages`, or the proposal has to raise `max_pages` with
its own reasoning. The shape then is a `for-each` over a variable with a fixed body, every request
inside it counted against the same `max_pages` and `max_total_time` the rest of the run spends, a
separate ceiling on iterations, no nesting, and `regex.pattern` made a template.

Either way the successor job carries the page with it. That is the standard RD-110-10 set and
this record keeps: the format grows for a page somebody fetched, never for one somebody expects.

/**
 * Which LinkGrabber states a link may be queued from.
 *
 * Mirrors `LinkCandidateState::ENQUEUEABLE` in `crates/rd-core/src/collector.rs`, which the
 * single-link endpoint and the package claim both derive from. Four places in this UI used to
 * carry their own slightly different copy of this list, and every one of them was missing at
 * least one state — which is how a package whose check had failed ended up with its "add to
 * downloader" button greyed out.
 */
export const ENQUEUEABLE_STATES = [
  'online',
  'duplicate',
  'offline',
  'unsupported',
  'error'
] as const

/**
 * States where the check reached no conclusion about the link.
 *
 * Deliberately separate from `offline`: a hoster saying a file is gone and a check that never
 * got an answer are different things, and showing both as "offline" told the user the file was
 * missing when the account was the problem.
 */
const UNVERIFIED_STATES = ['unsupported', 'error'] as const

/**
 * `unresolvable` appears in neither list, and that is the whole of its behaviour.
 *
 * It is not enqueueable, because the address was reached and answered with a page: queueing it
 * can only store somebody's error page under a file's name (RD-110-07). And it is not
 * "unverified" either, which is the opposite mistake — nothing about it is unverified, it was
 * checked and the answer was conclusive.
 */
export function isEnqueueable(state: string): boolean {
  return (ENQUEUEABLE_STATES as readonly string[]).includes(state)
}

export function isUnverified(state: string): boolean {
  return (UNVERIFIED_STATES as readonly string[]).includes(state)
}

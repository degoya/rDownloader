import type { SubscriptionRun } from '@/api/types'

/**
 * Matches `FIRST_POLL_PAGES * DEFAULT_LIMIT` in `crates/rd-subscription/src/indexer.rs`: the most
 * a subscription's first poll reads, which has no archive to meet yet.
 */
export const INDEXER_FIRST_POLL_ITEMS = 500

/** Matches `MAX_INDEXER_ITEMS` there: the page bound of every later poll, in entries. */
export const INDEXER_POLL_BOUND = 2000

/**
 * Whether an indexer check left a gap behind it (RD-1150-05).
 *
 * A check pages on until a page ends in an entry the subscription already has; that page comes
 * along whole, so a check that closed its gap always found at least one entry it did not archive
 * again. A gap is the other shape: more than a first check can read, and every entry new — the
 * check reached its page bound without meeting anything it knew, and what lies behind the bound
 * may be missing. A failed check says nothing either way.
 */
export function indexerPollGap(run: Pick<SubscriptionRun, 'found' | 'accepted' | 'skipped' | 'error'> | undefined): boolean {
  if (!run || run.error) return false
  return run.found > INDEXER_FIRST_POLL_ITEMS && run.accepted + run.skipped >= run.found
}

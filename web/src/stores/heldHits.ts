import type { SubscriptionItem } from '@/api/types'

/** What a re-read under the reader's hand may change, and what it holds back (RD-1250-13). */
export interface HeldMerge {
  /** The rows on screen: the same ones in the same order, each with its fresh content. */
  items: SubscriptionItem[]
  /** The ids the re-read found that are not on screen. */
  held: string[]
}

/**
 * Takes a re-read into a list somebody is working through without moving a row under them.
 *
 * A row on screen keeps its place and takes the fresh content; a row the re-read no longer
 * has leaves only when that is certain — this client decided it, or the read reached the end
 * of the list, so a missing row is no longer pending. A full read that ends before a row may
 * simply have been pushed past its window by the hits held back, so such a row stays. A hit the
 * re-read found that is not on screen is not inserted: it is counted, and the reader decides
 * when it comes in.
 */
export function mergeUnderHand(
  shown: SubscriptionItem[],
  fresh: SubscriptionItem[],
  options: { complete: boolean, decided: ReadonlySet<string> }
): HeldMerge {
  const byId = new Map(fresh.map(item => [item.id, item]))
  const items = shown.flatMap(item => {
    const current = byId.get(item.id)
    if (current) return [current]
    return options.complete || options.decided.has(item.id) ? [] : [item]
  })
  const kept = new Set(shown.map(item => item.id))
  return { items, held: fresh.filter(item => !kept.has(item.id)).map(item => item.id) }
}

/**
 * The shapes and helpers of the subscriptions store's hit lists (`subscriptionsItems.ts`), apart
 * so that file stays one closure under the length limit (RD-1280-08).
 */
import type { SubscriptionItem } from '@/api/types'

export type SubscriptionItemFilter = 'pending' | 'queued' | 'dismissed' | 'skipped' | 'all'

export interface ItemQuery {
  state: SubscriptionItemFilter
  page: number
  /**
   * How many pages from `page` on the list holds (RD-130-13). One for the paged list; the card
   * carousel grows it with `loadMoreItems`, and a re-read keeps it, so a decision or a poll does
   * not throw the carousel back to its first fifty hits.
   */
  pages: number
}

/** Why a change reached the hit lists, as `noteChange` needs it (RD-110-30). */
export interface ItemsChange {
  /** The stream said events were lost, so every wanted list may be out of date. */
  lost: boolean
  /** The subscription whose finished poll wrote rows, if any. */
  wroteRowsFor: string | null
}

/**
 * How far "all" reaches in a subscription's review list (RD-1250-13): every pending hit found up
 * to `until`, the newest one when the list was last drawn, and how many of them there were.
 */
export interface ReviewWatermark {
  until: string | null
  count: number
}

/**
 * The `newest_discovered_at` of a page or a review count, checked: it is data from the network,
 * and a malformed one must leave the watermark empty rather than take the reader along.
 */
export function newestOf(value: object): string | null {
  const newest = 'newest_discovered_at' in value ? value.newest_discovered_at : null
  return typeof newest === 'string' ? newest : null
}

export const ITEM_PAGE_SIZE = 50
/** The most hits `items/page` answers with in one request (`subscription_handlers.rs`). */
export const ITEM_READ_MAX = 200

/**
 * The hits in order, each once. Pages read one after another are not one snapshot: a hit that
 * arrives in between moves every later one down by one, and the first hit of the next page is
 * then the last of the previous — a repeated key the carousel's `v-for` must not see.
 */
export function withoutRepeats(items: SubscriptionItem[]): SubscriptionItem[] {
  const seen = new Set<string>()
  return items.filter(item => {
    if (seen.has(item.id)) return false
    seen.add(item.id)
    return true
  })
}

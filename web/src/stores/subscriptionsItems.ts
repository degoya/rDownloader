import { ref } from 'vue'

import { api, errorMessage, responseError } from '@/api/client'
import type { SubscriptionItem, SubscriptionItemPage, SubscriptionReviewSummary } from '@/api/types'

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
interface ItemsChange {
  /** The stream said events were lost, so every wanted list may be out of date. */
  lost: boolean
  /** The subscription whose finished poll wrote rows, if any. */
  wroteRowsFor: string | null
}

const ITEM_PAGE_SIZE = 50
/** The most hits `items/page` answers with in one request (`subscription_handlers.rs`). */
const ITEM_READ_MAX = 200

/**
 * The hits in order, each once. Pages read one after another are not one snapshot: a hit that
 * arrives in between moves every later one down by one, and the first hit of the next page is
 * then the last of the previous — a repeated key the carousel's `v-for` must not see.
 */
function withoutRepeats(items: SubscriptionItem[]): SubscriptionItem[] {
  const seen = new Set<string>()
  return items.filter(item => {
    if (seen.has(item.id)) return false
    seen.add(item.id)
    return true
  })
}

/**
 * The subscriptions store's hit lists and review counts (RD-140-27): which lists are wanted,
 * reading and extending them, and re-reading the ones a change made stale. The store owns the
 * event stream and hands every change to `noteChange`, so it stays one surface.
 */
export function useSubscriptionItems() {
  const items = ref<Record<string, SubscriptionItem[]>>({})
  const itemPages = ref<Record<string, SubscriptionItemPage>>({})
  const itemQueries = ref<Record<string, ItemQuery>>({})
  /**
   * Why a subscription's hit list is not on screen: the failure of its last page read, or
   * `null` (RD-109-28).
   *
   * Kept here rather than in the view because a list is re-read from two sides — the view when
   * somebody opens a group, and the event stream when the hits change — and an error owned by
   * the view would survive a reload it never started.
   */
  const itemErrors = ref<Record<string, string | null>>({})
  const reviewSummary = ref<SubscriptionReviewSummary>({ pending_total: 0, subscriptions: [] })

  /**
   * The page each subscription's list was last *asked* for, recorded when the request goes out
   * (RD-109-28).
   *
   * The reload branch used to ask whether a list had already arrived, which is a different
   * question: a group opened while a check was still running had its request in flight and
   * nothing in `items` yet, so the event that ended the check skipped it, the request answered
   * with the page from before the hits were written, and the list stayed empty for the rest of
   * the session while the badge climbed. What matters is whether a list is *wanted*.
   */
  const wantedItems = new Map<string, ItemQuery>()
  /**
   * The newest page request per subscription, so a slower older answer cannot overwrite it.
   *
   * Two reads of the same list can be in flight at once — one from a click, one from the event
   * stream — and without this the last answer to arrive won, even when it was the older read.
   */
  const latestItemRequest = new Map<string, number>()
  let itemRequestSeq = 0
  /** The subscriptions whose carousel is reading its next page (`loadMoreItems`). */
  const moreInFlight = new Set<string>()
  /**
   * How many `subscription.changed` events have been seen, which of them last invalidated each
   * list, and which each list was last read for (RD-110-30).
   *
   * A list is stale when it was invalidated by a later change than the one it was read for,
   * and the invalidation names its cause. RD-109-28 re-read a list when the summary's pending
   * count disagreed with the page's, which is a symptom: a dismissal and a new hit in the same
   * window leave the count where it was and the list old. What invalidates a list now is a
   * finished poll that created rows for its subscription; a review count that moved between two
   * reads of the summary, since the anonymous write event names no subscription and the counts
   * are the closest the client gets to its cause; and a stream marker, which says events were
   * lost and everything may be out of date. The generation is what keeps the two reads one event
   * triggers from becoming two reads of the same list: a page read started for a change already
   * reflects the counts read for it.
   */
  let changeGeneration = 0
  const itemReadGeneration = new Map<string, number>()
  const itemChangedGeneration = new Map<string, number>()
  /** The pending count per subscription as the summary last reported it; `null` before the first read. */
  let reportedPending: Map<string, number> | null = null
  /** The newest summary request, so an older answer cannot move the counts backwards. */
  let latestSummaryRequest = 0
  /**
   * True once a view has asked for the review summary, which is what makes the counts worth
   * re-reading on an event.
   *
   * Not `reviewSummary.subscriptions.length`: with nothing pending yet that list is empty, so
   * the very first hit never moved the badge.
   */
  let reviewSummaryWanted = false

  /**
   * Returns the failure message, so the caller can tell an empty archive from a failed read.
   *
   * `pages` consecutive pages from `page` on; more than the server answers at once
   * (`ITEM_READ_MAX`) is read in several requests, one after the other.
   */
  async function loadItems(
    id: string,
    state: SubscriptionItemFilter = itemQueries.value[id]?.state ?? 'pending',
    page = itemQueries.value[id]?.page ?? 1,
    pages = 1
  ): Promise<string | null> {
    const safePage = Math.max(1, page)
    const safePages = Math.max(1, pages)
    wantedItems.set(id, { state, page: safePage, pages: safePages })
    itemReadGeneration.set(id, changeGeneration)
    const token = ++itemRequestSeq
    latestItemRequest.set(id, token)
    const offset = (safePage - 1) * ITEM_PAGE_SIZE
    const wanted = safePages * ITEM_PAGE_SIZE
    const collected: SubscriptionItem[] = []
    let latest: SubscriptionItemPage | null = null
    while (collected.length < wanted) {
      const limit = Math.min(ITEM_READ_MAX, wanted - collected.length)
      // A connection that never carried a response rejects instead of answering, and an
      // unhandled rejection here left the group locked in its loading state for good.
      const response = await api.GET('/api/v1/subscriptions/{id}/items/page', {
        params: {
          path: { id },
          query: { state, limit, offset: offset + collected.length }
        }
      }).catch(() => null)
      // A newer read of the same list went out while this one was on its way; that one owns the
      // outcome, error included, and this answer is by definition the older state.
      if (latestItemRequest.get(id) !== token) return null
      if (!response?.data) {
        // Untranslated browser prose has no place on screen, so a rejection becomes the generic
        // failure; what matters is that the group says it could not read rather than showing none.
        const message = response ? responseError(response) : errorMessage(undefined)
        itemErrors.value = { ...itemErrors.value, [id]: message }
        return message
      }
      // The page asked for may no longer exist (RD-110-30): every hit rejected from page two left
      // the page at two, and the hits that arrived afterwards fit on page one — read at two, the
      // list was empty under a badge that counted them. The last page that exists is read instead;
      // the recursion ends because the page only ever decreases.
      const lastPage = Math.max(1, Math.ceil(response.data.total / ITEM_PAGE_SIZE))
      if (!latest && safePage > lastPage) return loadItems(id, state, lastPage, safePages)
      latest = response.data
      collected.push(...response.data.items)
      if (response.data.items.length < limit) break
    }
    if (!latest) return null
    itemQueries.value = { ...itemQueries.value, [id]: { state, page: safePage, pages: safePages } }
    itemPages.value = { ...itemPages.value, [id]: latest }
    items.value = { ...items.value, [id]: withoutRepeats(collected) }
    itemErrors.value = { ...itemErrors.value, [id]: null }
    return null
  }

  /** The list as it is now, with as many pages as it already holds — for the re-reads. */
  function reloadItems(id: string): Promise<string | null> {
    const query = itemQueries.value[id]
    return loadItems(id, query?.state, query?.page, query?.pages ?? 1)
  }

  /**
   * Appends the next page of one subscription's hits, for the card carousel (RD-130-13).
   *
   * The carousel is itself the way through the hits, so it holds all of them rather than one
   * page under a pagination bar, and reads them fifty at a time as the reader gets close to the
   * end of what it has. One read per subscription at a time: the carousel asks again when the
   * answer has not taken it far enough. An answer is dropped when the list it was meant to extend
   * was replaced while it was on its way — a full re-read owns the list, and a page counted from
   * the old list's length could skip a hit or repeat one.
   */
  async function loadMoreItems(id: string): Promise<string | null> {
    const query = itemQueries.value[id]
    const current = items.value[id]
    if (!query || !current || moreInFlight.has(id)) return null
    if (current.length >= (itemPages.value[id]?.total ?? 0)) return null
    moreInFlight.add(id)
    try {
      const response = await api.GET('/api/v1/subscriptions/{id}/items/page', {
        params: {
          path: { id },
          query: {
            state: query.state,
            limit: ITEM_PAGE_SIZE,
            offset: (query.page - 1) * ITEM_PAGE_SIZE + current.length
          }
        }
      }).catch(() => null)
      if (items.value[id] !== current) return null
      if (!response?.data) {
        const message = response ? responseError(response) : errorMessage(undefined)
        itemErrors.value = { ...itemErrors.value, [id]: message }
        return message
      }
      const extended = { ...query, pages: query.pages + 1 }
      wantedItems.set(id, extended)
      itemQueries.value = { ...itemQueries.value, [id]: extended }
      itemPages.value = { ...itemPages.value, [id]: response.data }
      items.value = { ...items.value, [id]: withoutRepeats([...current, ...response.data.items]) }
      return null
    } finally {
      moreInFlight.delete(id)
    }
  }

  /** Records that the hits of one subscription changed with the given event. */
  function invalidateItems(id: string, generation: number): void {
    if ((itemChangedGeneration.get(id) ?? -1) < generation) itemChangedGeneration.set(id, generation)
  }

  /**
   * Re-reads every wanted list that a change has invalidated since it was last read
   * (RD-110-30).
   *
   * Only wanted lists: a group nobody has opened stays lazy and reads the current state on its
   * first expand, so an invalidation for it costs nothing until then. A wanted list is re-read
   * whatever its filter, because a poll or a decision changes the archive as much as the review.
   */
  function reloadStaleItems(): void {
    for (const [id, wanted] of wantedItems) {
      if ((itemChangedGeneration.get(id) ?? -1) <= (itemReadGeneration.get(id) ?? -1)) continue
      void loadItems(id, wanted.state, wanted.page, wanted.pages)
    }
  }

  /**
   * Marks the lists whose review count moved since the summary was last read (RD-110-30).
   *
   * The anonymous write event names nothing, so the counts are the closest the client gets to
   * what it changed: a subscription whose pending count moved had a hit added or decided. The
   * first read only sets the baseline. Read back from the store rather than the response, and
   * checked: the summary is data from the network, and a malformed body must not take the event
   * handler down with it.
   */
  function noteReportedPending(generation: number): void {
    const counted = reviewSummary.value.subscriptions
    if (!Array.isArray(counted)) return
    const current = new Map(counted.map(entry => [entry.subscription_id, entry.pending]))
    const previous = reportedPending
    reportedPending = current
    if (!previous) return
    for (const id of new Set([...previous.keys(), ...current.keys()])) {
      if ((previous.get(id) ?? 0) !== (current.get(id) ?? 0)) invalidateItems(id, generation)
    }
  }

  async function loadReviewSummary(): Promise<string | null> {
    reviewSummaryWanted = true
    const generation = changeGeneration
    const token = ++latestSummaryRequest
    const response = await api.GET('/api/v1/subscriptions/review-summary')
    // A newer read went out while this one was on its way; its answer is the older state.
    if (latestSummaryRequest !== token) return null
    if (!response.data) return responseError(response)
    reviewSummary.value = response.data
    noteReportedPending(generation)
    reloadStaleItems()
    return null
  }

  /**
   * Takes one `subscription.changed` event into the hit lists (RD-110-30): a stream marker
   * invalidates every wanted list, a finished poll that wrote rows its own; the stale ones are
   * re-read, and the review counts too whenever a view showed them.
   */
  function noteChange(change: ItemsChange): void {
    changeGeneration += 1
    if (change.lost) {
      for (const id of wantedItems.keys()) invalidateItems(id, changeGeneration)
    }
    if (change.wroteRowsFor) invalidateItems(change.wroteRowsFor, changeGeneration)
    reloadStaleItems()
    if (reviewSummaryWanted) void loadReviewSummary()
  }

  return {
    items,
    itemPages,
    itemQueries,
    itemErrors,
    reviewSummary,
    loadItems,
    reloadItems,
    loadMoreItems,
    loadReviewSummary,
    noteChange
  }
}

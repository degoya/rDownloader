import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { SubscriptionItem } from '@/api/types'

import { useSubscriptionsStore } from './subscriptions'

/**
 * How a hit list is kept in step with the count beside it (RD-109-28).
 *
 * The badge is read from `review-summary`, the rows from `items/page`, and only the badge ever
 * followed the event stream — so a subscription could announce thirty-two hits above a list
 * that stayed empty until the page was reloaded by hand. These cases pin the two together.
 */
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PUT: vi.fn(), POST: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The subscription could not be reached'),
  errorMessage: vi.fn(() => 'The request failed')
}))

let subscriptionEvent: ((event: MessageEvent<string>) => void) | null = null
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    subscriptionEvent = handlers['subscription.changed'] ?? null
    return () => { subscriptionEvent = null }
  }
}))

function hit(id: string): SubscriptionItem {
  return { id, title: id, state: 'pending' } as SubscriptionItem
}

function page(items: SubscriptionItem[]): unknown {
  return {
    data: {
      items,
      total: items.length,
      counts: { pending: items.length, queued: 0, dismissed: 0, skipped: 0 },
      run_total: 0
    }
  }
}

function summary(pending: number, id = 'sub-1'): unknown {
  return { data: { pending_total: pending, subscriptions: [{ subscription_id: id, pending }] } }
}

/** A promise the test resolves by hand, so two reads can be held open at once. */
function deferred<T>(): { promise: Promise<T>, resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => {}
  const promise = new Promise<T>(settle => { resolve = settle })
  return { promise, resolve }
}

/** Lets every already-resolved promise in the chain run. */
async function flush(): Promise<void> {
  for (let round = 0; round < 8; round += 1) await Promise.resolve()
}

function itemPageCalls(): string[] {
  return vi.mocked(api.GET).mock.calls
    .filter(call => call[0] === '/api/v1/subscriptions/{id}/items/page')
    .map(call => (call[1] as { params: { path: { id: string } } }).params.path.id)
}

function changed(payload: Record<string, unknown>): void {
  subscriptionEvent?.({ data: JSON.stringify({ payload }) } as MessageEvent<string>)
}

beforeEach(() => {
  setActivePinia(createPinia())
  vi.clearAllMocks()
  subscriptionEvent = null
})

describe('subscriptions store: hits follow the same event as the count', () => {
  it('does not let the page read that started before the hits were written win', async () => {
    // The race from the report: a group is opened while the check is still running, so its
    // request reads the state from before the hits exist and answers only after the event that
    // said the check had finished.
    const stale = deferred<unknown>()
    const fresh = deferred<unknown>()
    let pageCall = 0
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions') return { data: [] }
      if (path === '/api/v1/subscriptions/review-summary') return summary(2)
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        pageCall += 1
        return pageCall === 1 ? stale.promise : fresh.promise
      }
      return { data: [] }
    }) as never)

    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadReviewSummary()
    const first = store.loadItems('sub-1')
    await flush()
    expect(pageCall).toBe(1)

    // The check ends while that first read is still outstanding. The old gate asked whether a
    // list had already arrived and skipped this, which is what left the group empty.
    changed({ poll: 'finished', subscription_id: 'sub-1', found: 2, accepted: 2, skipped: 0 })
    await flush()
    expect(pageCall).toBe(2)

    // Now the stale read answers — with the empty page it read before the hits were written.
    stale.resolve(page([]))
    await first
    await flush()
    expect(store.items['sub-1']).toBeUndefined()

    fresh.resolve(page([hit('item-1'), hit('item-2')]))
    await flush()
    expect(store.items['sub-1']?.map(item => item.id)).toEqual(['item-1', 'item-2'])
    expect(store.reviewSummary.pending_total).toBe(2)
  })

  it('re-reads exactly the subscription whose count the summary moved', async () => {
    let pending = 1
    let hits = [hit('item-1')]
    vi.mocked(api.GET).mockImplementation((async (path: string, options?: {
      params: { path?: { id: string } }
    }) => {
      if (path === '/api/v1/subscriptions') return { data: [] }
      if (path === '/api/v1/subscriptions/review-summary') {
        return { data: {
          pending_total: pending,
          subscriptions: [
            { subscription_id: 'sub-1', pending },
            { subscription_id: 'sub-2', pending: 5 }
          ]
        } }
      }
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        return options?.params.path?.id === 'sub-1' ? page(hits) : page([])
      }
      return { data: [] }
    }) as never)

    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadItems('sub-1')
    await store.loadReviewSummary()
    await flush()
    expect(itemPageCalls()).toEqual(['sub-1'])

    // A hit arrives. The event carries nothing but the fact that something changed; the counts
    // are read back, and the list that disagrees with them is read back with it.
    pending = 2
    hits = [hit('item-1'), hit('item-2')]
    changed({})
    await flush()

    expect(itemPageCalls()).toEqual(['sub-1', 'sub-1'])
    expect(store.items['sub-1']?.map(item => item.id)).toEqual(['item-1', 'item-2'])
    // `sub-2` has five pending hits and no list anybody asked for: nothing is fetched for it.
    expect(itemPageCalls()).not.toContain('sub-2')
  })

  it('leaves a list alone while it still matches the count', async () => {
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions') return { data: [] }
      if (path === '/api/v1/subscriptions/review-summary') return summary(1)
      if (path === '/api/v1/subscriptions/{id}/items/page') return page([hit('item-1')])
      return { data: [] }
    }) as never)

    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadItems('sub-1')
    await store.loadReviewSummary()
    changed({})
    await flush()

    expect(itemPageCalls()).toEqual(['sub-1'])
  })

  it('reports a failed page read instead of an empty list, and clears it on the next read', async () => {
    let fail = true
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        return fail ? { error: { code: 'unreachable' } } : page([hit('item-1')])
      }
      return { data: [] }
    }) as never)

    const store = useSubscriptionsStore()
    expect(await store.loadItems('sub-1')).toBe('The subscription could not be reached')
    expect(store.itemErrors['sub-1']).toBe('The subscription could not be reached')
    expect(store.items['sub-1']).toBeUndefined()

    fail = false
    expect(await store.loadItems('sub-1')).toBeNull()
    expect(store.itemErrors['sub-1']).toBeNull()
  })

  it('turns a rejected page request into a stated failure rather than a hung read', async () => {
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions/{id}/items/page') throw new Error('network down')
      return { data: [] }
    }) as never)

    const store = useSubscriptionsStore()
    expect(await store.loadItems('sub-1')).toBe('The request failed')
    expect(store.itemErrors['sub-1']).toBe('The request failed')
  })
})

/**
 * The report behind RD-110-30: every hit rejected, new ones arriving, the drawer showing their
 * number over empty groups until the page was reloaded by hand. RD-109-28 had tied a list's
 * re-read to a comparison of numbers — the page's pending count against the summary's — and
 * left the *page* the list was read at alone. These cases pin the re-read to its cause.
 */
describe('subscriptions store: a list is re-read for the cause of its change (RD-110-30)', () => {
  /** A server whose page endpoint and summary read one archive, as the real one does. */
  function serve(archive: { hits: SubscriptionItem[] }): void {
    vi.mocked(api.GET).mockImplementation((async (path: string, options?: {
      params: { path?: { id: string }, query?: { offset: number } }
    }) => {
      const pending = archive.hits.filter(item => item.state === 'pending')
      if (path === '/api/v1/subscriptions') return { data: [] }
      if (path === '/api/v1/subscriptions/review-summary') return summary(pending.length)
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        const offset = options?.params.query?.offset ?? 0
        return { data: {
          items: pending.slice(offset, offset + 50),
          total: pending.length,
          counts: { pending: pending.length, queued: 0, dismissed: 0, skipped: 0 },
          run_total: 0
        } }
      }
      return { data: [] }
    }) as never)
    vi.mocked(api.PUT).mockImplementation((async () => {
      for (const item of archive.hits) if (item.state === 'pending') item.state = 'dismissed'
      return { data: { matched: 0, updated: 0, failed: 0 } }
    }) as never)
  }

  it('replays the report: all hits rejected from the last page, a new hit arrives, the list shows it', async () => {
    const archive = { hits: Array.from({ length: 75 }, (_, index) => hit(`item-${index}`)) }
    serve(archive)
    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadReviewSummary()
    // Reading page two, the reader rejects everything.
    await store.loadItems('sub-1', 'pending', 2)
    expect(store.items['sub-1']).toHaveLength(25)
    await store.setPendingItemStates('sub-1', 'dismissed')
    // The write's own event, as the server sends it.
    changed({})
    await flush()
    expect(store.items['sub-1']).toEqual([])
    expect(store.reviewSummary.pending_total).toBe(0)

    // A check running elsewhere writes one hit and then records its run.
    archive.hits.push(hit('item-new'))
    changed({})
    changed({ poll: 'finished', subscription_id: 'sub-1', found: 1, accepted: 1, skipped: 0 })
    await flush()
    expect(store.reviewSummary.pending_total).toBe(1)
    expect(store.items['sub-1']?.map(item => item.id)).toEqual(['item-new'])

    // Opening the group again reads what the badge counts, not a page that no longer exists.
    await store.loadItems('sub-1', 'pending', store.itemQueries['sub-1']?.page ?? 1)
    expect(store.items['sub-1']?.map(item => item.id)).toEqual(['item-new'])
    expect(store.itemQueries['sub-1']?.page).toBe(1)
  })

  it('re-reads a list whose poll created hits although its pending count did not move', async () => {
    const archive = { hits: [hit('item-1')] }
    serve(archive)
    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadReviewSummary()
    await store.loadItems('sub-1')
    expect(itemPageCalls()).toEqual(['sub-1'])

    // Decided elsewhere and replaced by a new hit inside the same window: the count stays one.
    archive.hits = [{ ...hit('item-1'), state: 'dismissed' }, hit('item-2')]
    changed({ poll: 'finished', subscription_id: 'sub-1', found: 2, accepted: 1, skipped: 0 })
    await flush()

    expect(store.reviewSummary.pending_total).toBe(1)
    expect(itemPageCalls()).toEqual(['sub-1', 'sub-1'])
    expect(store.items['sub-1']?.map(item => item.id)).toEqual(['item-2'])
  })

  it('loads a never-opened list on its first expand, current, and not before', async () => {
    const archive = { hits: [hit('item-1')] }
    serve(archive)
    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadReviewSummary()

    archive.hits.push(hit('item-2'))
    changed({})
    changed({ poll: 'finished', subscription_id: 'sub-1', found: 2, accepted: 1, skipped: 0 })
    await flush()
    // Nobody has asked for the list: the count moved, the list is not fetched.
    expect(store.reviewSummary.pending_total).toBe(2)
    expect(itemPageCalls()).toEqual([])

    await store.loadItems('sub-1', 'pending', 1)
    expect(itemPageCalls()).toEqual(['sub-1'])
    expect(store.items['sub-1']?.map(item => item.id)).toEqual(['item-1', 'item-2'])
  })

  it('does not re-read a list for an event without hit relevance', async () => {
    const archive = { hits: [hit('item-1')] }
    serve(archive)
    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadReviewSummary()
    await store.loadItems('sub-1')

    // A subscription renamed or disabled: the write's event names nothing and moves no count.
    changed({})
    await flush()
    // A check of this subscription that offered a hundred entries and created none.
    changed({ poll: 'finished', subscription_id: 'sub-1', found: 100, accepted: 0, skipped: 0 })
    await flush()
    // A check of another subscription, which did create hits — of its own.
    changed({ poll: 'finished', subscription_id: 'sub-2', found: 3, accepted: 3, skipped: 0 })
    await flush()

    expect(itemPageCalls()).toEqual(['sub-1'])
  })

  it('treats the stream marker as a change of unknown extent and re-reads every wanted list', async () => {
    const archive = { hits: [hit('item-1')] }
    serve(archive)
    const store = useSubscriptionsStore()
    store.connectEvents()
    await store.loadReviewSummary()
    await store.loadItems('sub-1')

    // `stream.lagged` reaches every subscriber with its own body, which carries no `payload`.
    subscriptionEvent?.({ data: JSON.stringify({ dropped: 7 }) } as MessageEvent<string>)
    await flush()

    expect(itemPageCalls()).toEqual(['sub-1', 'sub-1'])
  })
})

/**
 * The card carousel holds every hit and reads them fifty at a time (RD-130-13). What the store
 * owes it: the next page appended, not a replacement; a re-read that keeps what was read, in
 * requests the server answers (at most 200 each); and no answer that extends a list which was
 * replaced while it was on its way.
 */
describe('subscriptions store: the carousel reads on', () => {
  const pageReads: Array<{ offset: number, limit: number }> = []

  function serve(archive: { hits: SubscriptionItem[] }, hold?: Promise<unknown>): void {
    pageReads.length = 0
    vi.mocked(api.GET).mockImplementation((async (path: string, options?: {
      params: { path?: { id: string }, query?: { offset: number, limit: number } }
    }) => {
      const pending = archive.hits.filter(item => item.state === 'pending')
      if (path === '/api/v1/subscriptions/review-summary') return summary(pending.length)
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        const offset = options?.params.query?.offset ?? 0
        // The server clamps, as `subscription_handlers.rs` does.
        const limit = Math.min(200, options?.params.query?.limit ?? 50)
        pageReads.push({ offset, limit: options?.params.query?.limit ?? 50 })
        if (hold && offset > 0) await hold
        return { data: {
          items: pending.slice(offset, offset + limit),
          total: pending.length,
          counts: { pending: pending.length, queued: 0, dismissed: 0, skipped: 0 },
          run_total: 0
        } }
      }
      return { data: [] }
    }) as never)
    vi.mocked(api.PUT).mockResolvedValue({ data: undefined } as never)
  }

  function archiveOf(count: number): { hits: SubscriptionItem[] } {
    return { hits: Array.from({ length: count }, (_, index) => hit(`item-${index}`)) }
  }

  it('appends the next fifty and stops at the end', async () => {
    serve(archiveOf(120))
    const store = useSubscriptionsStore()
    await store.loadItems('sub-1', 'pending', 1)
    expect(store.items['sub-1']).toHaveLength(50)

    await store.loadMoreItems('sub-1')
    await store.loadMoreItems('sub-1')
    expect(store.items['sub-1']).toHaveLength(120)
    expect(store.items['sub-1']?.[50]?.id).toBe('item-50')
    expect(store.itemQueries['sub-1']).toEqual({ state: 'pending', page: 1, pages: 3 })

    // Everything is read; a further ask costs no request.
    await store.loadMoreItems('sub-1')
    expect(pageReads.map(read => read.offset)).toEqual([0, 50, 100])
  })

  it('keeps what the carousel has read across a decision, in requests of at most 200', async () => {
    const archive = archiveOf(300)
    serve(archive)
    const store = useSubscriptionsStore()
    await store.loadItems('sub-1', 'pending', 1)
    for (let page = 0; page < 4; page += 1) await store.loadMoreItems('sub-1')
    expect(store.items['sub-1']).toHaveLength(250)

    pageReads.length = 0
    const first = archive.hits[0] as SubscriptionItem
    first.state = 'dismissed'
    await store.setItemState('item-0', 'sub-1', 'dismissed')

    // Five pages were held, so five are read back: 200, then the 50 after them.
    expect(pageReads).toEqual([{ offset: 0, limit: 200 }, { offset: 200, limit: 50 }])
    expect(store.items['sub-1']).toHaveLength(250)
    expect(store.items['sub-1']?.[0]?.id).toBe('item-1')
  })

  it('reads a single page again when a view asks for one, whatever the carousel had read', async () => {
    serve(archiveOf(120))
    const store = useSubscriptionsStore()
    await store.loadItems('sub-1', 'pending', 1)
    await store.loadMoreItems('sub-1')
    await store.loadItems('sub-1', 'pending', 1)
    expect(store.items['sub-1']).toHaveLength(50)
    expect(store.itemQueries['sub-1']?.pages).toBe(1)
  })

  it('shows every hit once when a new one moved the next page down', async () => {
    const archive = archiveOf(80)
    serve(archive)
    const store = useSubscriptionsStore()
    await store.loadItems('sub-1', 'pending', 1)
    // A hit arrives at the head: what was item-49 is now at offset 50.
    archive.hits.unshift(hit('item-new'))
    await store.loadMoreItems('sub-1')
    const ids = store.items['sub-1']?.map(item => item.id) ?? []
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids).toContain('item-79')
  })

  it('drops a next page whose list was read anew while it was on its way', async () => {
    const hold = deferred<unknown>()
    serve(archiveOf(120), hold.promise)
    const store = useSubscriptionsStore()
    await store.loadItems('sub-1', 'pending', 1)
    const more = store.loadMoreItems('sub-1')
    await flush()
    // A second ask while the first is out is not a second request.
    await store.loadMoreItems('sub-1')
    expect(pageReads.filter(read => read.offset === 50)).toHaveLength(1)

    await store.loadItems('sub-1', 'pending', 1)
    hold.resolve(undefined)
    await more
    expect(store.items['sub-1']).toHaveLength(50)
    expect(store.itemQueries['sub-1']?.pages).toBe(1)
  })
})

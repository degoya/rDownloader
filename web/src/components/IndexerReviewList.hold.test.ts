/**
 * RD-1250-13: the review drawer's list does not move under the reader's hand.
 *
 * Owner, 2026-10-11: a check that wrote hits while the drawer was open put them in front of the
 * rows being read, and "dismiss all" took them along unseen. While the drawer is open the hits
 * are still read, but held back and counted until "show" or closing the drawer brings them in.
 * Owner, same day: "all" is every page of the list as it was drawn, never a hit held back — the
 * drawer sends the list's watermark, which moves only when the list is drawn again.
 * Owner, 2026-10-21: only an open group holds still, a page turned in it included; a folded one
 * and a new one follow the counts, and the hint is a small "new" in the group's header.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, eventStreamMock, toastMock } from '@/test/mocks'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Subscription, SubscriptionItem } from '@/api/types'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import { mountComponent, passthrough, settle, uiStubs } from '@/test/mount'

import IndexerReviewList from './IndexerReviewList.vue'

vi.mock('@/api/client', () => apiClientMock({ responseError: vi.fn(() => 'failed'), errorMessage: vi.fn() }))
let subscriptionEvent: ((event: MessageEvent<string>) => void) | null = null
vi.mock('@/composables/useEventStream', () => eventStreamMock({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    subscriptionEvent = handlers['subscription.changed'] ?? null
    return () => { subscriptionEvent = null }
  }
}))
vi.mock('@/composables/useConfirm', () => confirmMock())
vi.mock('@nuxt/ui/composables', () => toastMock())

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, linkgrabber, subscriptions } } })
const components = {
  ...uiStubs,
  UButton: { props: ['label', 'disabled', 'loading'], template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>' },
  UBadge: passthrough,
  UIcon: passthrough,
  UDrawer: {
    props: ['open'],
    emits: ['update:open'],
    template: '<div v-if="open"><button aria-label="Close" @click="$emit(\'update:open\', false)">Close</button><slot name="body" /></div>'
  },
  UPagination: { emits: ['update:page'], template: '<button @click="$emit(\'update:page\', 2)">Next page</button>' }
}

const rows = [
  { id: 'sub-1', name: 'Weekly documentaries', kind: 'indexer', enabled: true },
  { id: 'sub-2', name: 'Nature 2160p', kind: 'indexer', enabled: true }
] as Subscription[]

const EARLIER = '2026-10-11T08:00:00Z'
const LATER = '2026-10-11T09:00:00Z'

function hit(id: string, title: string, discovered_at = EARLIER): SubscriptionItem {
  return { id, title, state: 'pending', discovered_at } as SubscriptionItem
}

/** What the fake server holds; the newest hit first, as the real list is ordered. */
let items: Record<string, SubscriptionItem[]> = {}
const pending = (id: string) => (items[id] ?? []).filter(item => item.state === 'pending')
const newest = (id: string) => pending(id)[0]?.discovered_at ?? null

beforeEach(() => {
  setActivePinia(createPinia())
  vi.clearAllMocks()
  items = { 'sub-1': [hit('item-1', 'Deep Ocean S01E01'), hit('item-2', 'Deep Ocean S01E02')], 'sub-2': [] }
  vi.mocked(api.GET).mockImplementation((async (path: string, options?: {
    params: { path: { id: string }, query: { limit: number, offset: number, until?: string } }
  }) => {
    if (path === '/api/v1/subscriptions') return { data: rows }
    if (path === '/api/v1/subscriptions/review-summary') {
      const counted = rows.map(row => ({
        subscription_id: row.id, pending: pending(row.id).length, newest_discovered_at: newest(row.id)
      }))
      return { data: { pending_total: counted.reduce((sum, entry) => sum + entry.pending, 0), subscriptions: counted } }
    }
    const id = options?.params.path.id ?? ''
    const until = options?.params.query.until
    const open = pending(id).filter(item => !until || item.discovered_at <= until)
    const offset = options?.params.query.offset ?? 0
    const page = open.slice(offset, offset + (options?.params.query.limit ?? 50))
    return { data: { items: page, total: open.length, newest_discovered_at: open[0]?.discovered_at ?? null } }
  }) as never)
  vi.mocked(api.PUT).mockImplementation((async (_path: string, options: {
    params: { path: { id: string } }, body: { state: SubscriptionItem['state'], until?: string }
  }) => {
    if (_path === '/api/v1/subscriptions/items/{id}') {
      const found = Object.values(items).flat().find(item => item.id === options.params.path.id)
      if (found) found.state = options.body.state
      return { data: found }
    }
    const until = options.body.until
    const chosen = pending(options.params.path.id).filter(item => !until || item.discovered_at <= until)
    for (const found of chosen) found.state = options.body.state
    return { data: { matched: chosen.length, updated: chosen.length, failed: 0 } }
  }) as never)
})

async function openGroup(first = 'Deep Ocean S01E01'): Promise<void> {
  mountComponent(IndexerReviewList, { i18n, stubs: components, keepPinia: true })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
  await fireEvent.click(screen.getByRole('button', { name: 'Review' }))
  await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
  await fireEvent.click(screen.getByLabelText('Weekly documentaries'))
  await waitFor(() => expect(screen.getByText(first)).toBeTruthy())
}

/** A check that ran elsewhere writes a hit in front; the event says only that something changed. */
async function arrives(id: string, item: SubscriptionItem): Promise<void> {
  items[id]?.unshift(item)
  subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
  await waitFor(() => expect(screen.getByTestId('indexer-review-held')).toBeTruthy())
}

describe('IndexerReviewList: the open drawer holds what arrives', () => {
  it('counts a hit that arrives while the drawer is open and draws it on "show"', async () => {
    await openGroup()
    await arrives('sub-1', hit('item-5', 'Deep Ocean S01E03'))
    expect(screen.getByRole('button', { name: '+1 new' })).toBeTruthy()
    expect(screen.queryByText('Deep Ocean S01E03')).toBeNull()
    expect(screen.getAllByRole('button', { name: 'Dismiss' })).toHaveLength(2)

    await fireEvent.click(screen.getByRole('button', { name: '+1 new' }))
    await waitFor(() => expect(screen.getByText('Deep Ocean S01E03')).toBeTruthy())
    expect(screen.getAllByRole('button', { name: 'Dismiss' })).toHaveLength(3)
    expect(screen.queryByTestId('indexer-review-held')).toBeNull()
  })

  it('dismisses up to the watermark of the list as drawn, not the hit held back', async () => {
    await openGroup()
    await arrives('sub-1', hit('item-5', 'Deep Ocean S01E03', LATER))
    await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss all' })[0] as HTMLElement)

    await waitFor(() => expect(pending('sub-1').map(item => item.id)).toEqual(['item-5']))
    expect(untilSent(0)).toBe(EARLIER)
    // Nothing is left on screen to hold still: the open group shows the held hit at once.
    await waitFor(() => expect(screen.getByText('Deep Ocean S01E03')).toBeTruthy())
    expect(screen.queryByTestId('indexer-review-held')).toBeNull()
  })

  it('moves the watermark on "show", not on the re-read that found the hit', async () => {
    await openGroup()
    await arrives('sub-1', hit('item-5', 'Deep Ocean S01E03', LATER))
    expect(useSubscriptionsStore().watermarks['sub-1']).toEqual({ until: EARLIER, count: 2 })
    await fireEvent.click(screen.getByRole('button', { name: '+1 new' }))
    await waitFor(() => expect(screen.getByText('Deep Ocean S01E03')).toBeTruthy())

    await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss all' })[0] as HTMLElement)
    await waitFor(() => expect(pending('sub-1')).toEqual([]))
    expect(untilSent(0)).toBe(LATER)
  })

  it('lets a folded group follow the counts and decide all of them', async () => {
    await openGroup()
    await fireEvent.click(screen.getByLabelText('Weekly documentaries'))
    items['sub-1']?.unshift(hit('item-5', 'Deep Ocean S01E03', LATER))
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
    await waitFor(() => expect(screen.getByText('3 hits')).toBeTruthy())
    expect(screen.queryByTestId('indexer-review-held')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss all' }))

    await waitFor(() => expect(pending('sub-1')).toEqual([]))
    expect(untilSent(0)).toBe(LATER)
  })

  it('takes the held hits in when the drawer closes', async () => {
    await openGroup()
    await arrives('sub-1', hit('item-5', 'Deep Ocean S01E03'))
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }))

    await waitFor(() => expect(useStoreItems('sub-1')).toEqual(['item-5', 'item-1', 'item-2']))
  })

  it('adds a group whose first hit arrives while the drawer is open folded, at the bottom', async () => {
    await openGroup()
    items['sub-2']?.unshift(hit('item-9', 'Alpine Winter', LATER))
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(screen.getByText('Nature 2160p')).toBeTruthy())
    const names = screen.getAllByRole('button', { expanded: false }).map(button => button.getAttribute('aria-label'))
    expect(names).toEqual(['Nature 2160p'])
    expect(screen.queryByTestId('indexer-review-held')).toBeNull()
    // The open group kept its rows and its place.
    expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy()
  })

  it('keeps the hits held back when a page is turned', async () => {
    items['sub-1'] = Array.from({ length: 60 }, (_, index) => hit(`item-${index}`, `Episode ${index}`))
    await openGroup('Episode 0')
    await arrives('sub-1', hit('item-new', 'Deep Ocean S02E01', LATER))
    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))

    await waitFor(() => expect(screen.getByText('Episode 59')).toBeTruthy())
    expect(screen.getByRole('button', { name: '+1 new' })).toBeTruthy()
    const pageRead = vi.mocked(api.GET).mock.calls.at(-1)?.[1] as unknown as { params: { query: { offset: number, until?: string } } }
    expect(pageRead.params.query).toMatchObject({ offset: 50, until: EARLIER })
    expect(useSubscriptionsStore().watermarks['sub-1']).toEqual({ until: EARLIER, count: 60 })
  })

  // Owner, 2026-10-27: deciding the last rows one by one left an open, empty group under "+N new".
  it('draws the held hits when single decisions empty the open list', async () => {
    await openGroup()
    await arrives('sub-1', hit('item-5', 'Deep Ocean S01E03', LATER))
    for (const title of ['Deep Ocean S01E01', 'Deep Ocean S01E02']) {
      await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss' })[0] as HTMLElement)
      await waitFor(() => expect(screen.queryByText(title)).toBeNull())
    }

    await waitFor(() => expect(screen.getByText('Deep Ocean S01E03')).toBeTruthy())
    expect(screen.queryByTestId('indexer-review-held')).toBeNull()
  })

  it('folds a group that ran out of hits, so its "all" decides them when it comes back', async () => {
    await openGroup()
    await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss all' })[0] as HTMLElement)
    await waitFor(() => expect(screen.queryByText('Weekly documentaries')).toBeNull())
    await waitFor(() => expect(useStoreItems('sub-1')).toEqual([]))
    await settle()
    items['sub-1']?.unshift(hit('item-5', 'Deep Ocean S01E03', LATER))
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
    await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss all' }))

    await waitFor(() => expect(pending('sub-1')).toEqual([]))
    expect(untilSent(1)).toBe(LATER)
  })
})

function untilSent(call: number): string | undefined {
  return (vi.mocked(api.PUT).mock.calls[call]![1] as unknown as { body: { until?: string } }).body.until
}

function useStoreItems(id: string): string[] {
  return (useSubscriptionsStore().items[id] ?? []).map(item => item.id)
}

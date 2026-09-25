import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Subscription, SubscriptionItem } from '@/api/types'
import type { ConfirmOptions } from '@/composables/useConfirm'
import commonDe from '@/locales/de/common.json'
import linkgrabberDe from '@/locales/de/linkgrabber.json'
import subscriptionsDe from '@/locales/de/subscriptions.json'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { useSubscriptionsStore } from '@/stores/subscriptions'

import IndexerReviewList from './IndexerReviewList.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PUT: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The subscription could not be reached'),
  errorMessage: vi.fn()
}))
let subscriptionEvent: ((event: MessageEvent<string>) => void) | null = null
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    subscriptionEvent = handlers['subscription.changed'] ?? null
    return () => { subscriptionEvent = null }
  }
}))

/** Both bulk actions ask first; the tests drive the answer. */
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const toasted = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toasted }) }))

const i18n = createI18n({
  legacy: false,
  locale: 'en',
  messages: {
    en: { common, linkgrabber, subscriptions },
    de: { common: commonDe, linkgrabber: linkgrabberDe, subscriptions: subscriptionsDe }
  }
})

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UButton: {
    props: ['label', 'disabled', 'loading'],
    template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>'
  },
  UBadge: passthrough,
  UIcon: passthrough,
  // Renders its body only while open, which is the half of the drawer these cases are about:
  // nothing must reach the screen until somebody asks for it.
  UDrawer: {
    props: ['open'],
    emits: ['update:open'],
    template: '<div v-if="open"><button aria-label="Close" @click="$emit(\'update:open\', false)">Close</button><slot name="body" /></div>'
  },
  UPagination: {
    props: ['page'],
    emits: ['update:page'],
    template: '<button aria-label="Next page" data-testid="pagination" @click="$emit(\'update:page\', page + 1)">Next page</button>'
  }
}

function subscription(id: string, name: string, kind: Subscription['kind'] = 'indexer'): Subscription {
  return { id, name, kind, enabled: true } as Subscription
}

function hit(id: string, title: string, state: SubscriptionItem['state'] = 'pending'): SubscriptionItem {
  return { id, title, state } as SubscriptionItem
}

const rows = [
  subscription('sub-1', 'Weekly documentaries'),
  subscription('sub-2', 'Nature 2160p'),
  subscription('sub-3', 'Nothing pending'),
  // Not an indexer: a media channel has no review step and is not this box's business.
  subscription('sub-4', 'A channel', 'media')
]

/** What the fake server holds. Its PUT moves an item out of `pending`, as the real one does. */
let items: Record<string, SubscriptionItem[]> = {}

beforeEach(() => {
  setActivePinia(createPinia())
  vi.clearAllMocks()
  i18n.global.locale.value = 'en'
  confirmed.mockImplementation(async () => true)
  subscriptionEvent = null
  items = {
    'sub-1': [hit('item-1', 'Deep Ocean S01E01'), hit('item-2', 'Deep Ocean S01E02')],
    'sub-2': [hit('item-3', 'Alpine Winter')],
    'sub-3': [hit('item-4', 'Already settled', 'dismissed')]
  }
  vi.mocked(api.GET).mockImplementation((async (path: string, options?: {
    params: { path?: { id: string }, query?: { state: SubscriptionItem['state'], limit: number, offset: number } }
  }) => {
    if (path === '/api/v1/subscriptions') return { data: rows }
    if (path === '/api/v1/subscriptions/review-summary') {
      const subscriptions = rows
        .filter(row => row.kind === 'indexer')
        .map(row => ({
          subscription_id: row.id,
          pending: (items[row.id] ?? []).filter(item => item.state === 'pending').length
        }))
      return { data: {
        pending_total: subscriptions.reduce((sum, entry) => sum + entry.pending, 0),
        subscriptions
      } }
    }
    if (path === '/api/v1/subscriptions/{id}/items/page') {
      const id = options?.params.path?.id ?? ''
      const pending = (items[id] ?? []).filter(item => item.state === 'pending')
      const offset = options?.params.query?.offset ?? 0
      const limit = Math.min(200, options?.params.query?.limit ?? 50)
      return { data: {
        items: pending.slice(offset, offset + limit),
        total: pending.length,
        counts: { pending: pending.length, queued: 0, dismissed: 0, skipped: 0 },
        run_total: 0
      } }
    }
    return { data: [] }
  }) as never)
  vi.mocked(api.PUT).mockImplementation((async (
    path: string,
    options: { params: { path: { id: string } }, body: { state: SubscriptionItem['state'] } }
  ) => {
    if (path === '/api/v1/subscriptions/{id}/items/pending') {
      const bucket = items[options.params.path.id] ?? []
      const matched = bucket.filter(item => item.state === 'pending').length
      for (const found of bucket) if (found.state === 'pending') found.state = options.body.state
      return { data: { matched, updated: matched, failed: 0 } }
    }
    for (const bucket of Object.values(items)) {
      const found = bucket.find(item => item.id === options.params.path.id)
      if (found) found.state = options.body.state
    }
    return { data: undefined }
  }) as never)
  vi.mocked(api.POST).mockResolvedValue({ data: undefined } as never)
})

/**
 * Renders the section, opens the drawer, then opens the first subscription group.
 *
 * The drawer is opened by hand because nothing opens it on its own any more: a panel that
 * unfolded itself on arrival is exactly what moved the rows underneath it about.
 */
async function openList(expectedItem = 'Deep Ocean S01E01'): Promise<void> {
  render(IndexerReviewList, { global: { plugins: [i18n], components } })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
  await fireEvent.click(screen.getByRole('button', { name: 'Review' }))
  await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
  await fireEvent.click(screen.getByLabelText('Weekly documentaries'))
  await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
  await waitFor(() => expect(screen.getByText(expectedItem)).toBeTruthy())
}

/**
 * Waits for the first subscription fetch to have *answered*, not merely to have been sent.
 *
 * The store's `settled` flag is what turns over when the list arrives, so a test that asserts
 * the box is absent cannot pass simply because nothing has been rendered yet.
 */
async function settled(): Promise<void> {
  const store = useSubscriptionsStore()
  await waitFor(() => expect(store.loading).toBe(false))
  await nextTick()
  await nextTick()
}

function polledIds(): string[] {
  return vi.mocked(api.POST).mock.calls
    .filter(call => call[0] === '/api/v1/subscriptions/{id}/poll')
    .map(call => (call[1] as { params: { path: { id: string } } }).params.path.id)
}

describe('IndexerReviewList', () => {
  it('gives every subscription with open hits a section of its own', async () => {
    await openList()
    expect(screen.getByText('Weekly documentaries')).toBeTruthy()
    expect(screen.getByText('Nature 2160p')).toBeTruthy()
    expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy()
    expect(screen.queryByText('Alpine Winter')).toBeNull()
    await fireEvent.click(screen.getByLabelText('Nature 2160p'))
    await waitFor(() => expect(screen.getByText('Alpine Winter')).toBeTruthy())
  })

  it('draws each group the way its own subscription asks: a list beside a card slider (RD-120-37)', async () => {
    const cards = rows[1] as Subscription
    cards.view = 'cards'
    cards.card_ratio = '1:1'
    try {
      await openList()
      await fireEvent.click(screen.getByLabelText('Nature 2160p'))
      await waitFor(() => expect(screen.getByTestId('subscription-slider')).toBeTruthy())
      // The list group stays a list, the card group is a slider, both open at once.
      expect(screen.getByTestId('subscription-list').textContent).toContain('Deep Ocean S01E01')
      expect(screen.getAllByTestId('subscription-card').map(card => card.getAttribute('aria-label')))
        .toEqual(['Alpine Winter'])
      expect(screen.getByTestId('subscription-slider').getAttribute('aria-label')).toBe('Hits of Nature 2160p')
      // The subscription's card ratio reaches every card of its slider (RD-120-42).
      expect(screen.getAllByTestId('card-tile').map(tile => tile.dataset.ratio)).toEqual(['1:1'])

      // The same decision from a card reaches the same endpoint the list uses.
      const slider = screen.getByTestId('subscription-slider')
      const queue = [...slider.querySelectorAll('button')].find(button => button.textContent?.includes('Queue'))
      await fireEvent.click(queue as HTMLButtonElement)
      await waitFor(() => expect(vi.mocked(api.PUT).mock.calls.some(call =>
        (call[1] as { params: { path: { id: string } } }).params.path.id === 'item-3')).toBe(true))
    } finally {
      cards.view = 'list'
      delete cards.card_ratio
    }
  })

  it('puts every hit of a card subscription in its slider, with no pagination bar (RD-130-13)', async () => {
    const cards = rows[1] as Subscription
    cards.view = 'cards'
    items['sub-2'] = Array.from({ length: 120 }, (_, index) => hit(`card-${index}`, `Card.${index}.S01E01-GRP`))
    try {
      await openList()
      await fireEvent.click(screen.getByLabelText('Nature 2160p'))
      await waitFor(() => expect(screen.getByTestId('subscription-slider')).toBeTruthy())
      // The slider counts all 120 although it has read fifty, and nothing pages under it.
      expect(screen.getByTestId('slider-counter').textContent?.trim()).toBe('Page 1 of 120')
      expect(screen.queryByTestId('pagination')).toBeNull()

      // jsdom shows one card per page: on page 50 the next page reaches past the fifty read.
      const slider = screen.getByTestId('subscription-slider')
      const next = [...slider.querySelectorAll('button')].find(button => button.getAttribute('aria-label') === 'Next page')
      for (let page = 1; page < 50; page += 1) await fireEvent.click(next as HTMLButtonElement)
      await waitFor(() => expect(vi.mocked(api.GET).mock.calls.some(call =>
        call[0] === '/api/v1/subscriptions/{id}/items/page'
        && (call[1] as { params: { query: { offset: number } } }).params.query.offset === 50)).toBe(true))
      await fireEvent.click(next as HTMLButtonElement)
      await fireEvent.click(next as HTMLButtonElement)
      await waitFor(() => expect(screen.getByTestId('slider-counter').textContent?.trim()).toBe('Page 52 of 120'))
      expect(screen.getAllByTestId('subscription-card')).toHaveLength(1)
    } finally {
      cards.view = 'list'
    }
  })

  it('keeps the pagination bar for a list subscription', async () => {
    items['sub-1'] = Array.from({ length: 75 }, (_, index) => hit(`item-${index}`, `Release ${index}`))
    await openList('Release 0')
    expect(screen.getByTestId('pagination')).toBeTruthy()
  })

  it('leaves out a subscription with nothing left to decide', async () => {
    await openList()
    expect(screen.queryByText('Nothing pending')).toBeNull()
    expect(screen.queryByText('Already settled')).toBeNull()
  })

  it('keeps the other sections as they were when one hit is decided', async () => {
    await openList()
    // Open and collapse the second section; deciding in the first must not reopen it.
    await fireEvent.click(screen.getByLabelText('Nature 2160p'))
    await waitFor(() => expect(screen.getByText('Alpine Winter')).toBeTruthy())
    await fireEvent.click(screen.getByLabelText('Nature 2160p'))
    await waitFor(() => expect(screen.queryByText('Alpine Winter')).toBeNull())

    await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss' })[0] as HTMLElement)
    await waitFor(() => expect(screen.queryByText('Deep Ocean S01E01')).toBeNull())

    expect(screen.queryByText('Alpine Winter')).toBeNull()
    expect(screen.getByText('Nature 2160p')).toBeTruthy()
  })

  it('queues every hit of one subscription and touches no other', async () => {
    await openList()
    await fireEvent.click(screen.getAllByRole('button', { name: 'Queue all' })[0] as HTMLElement)
    await waitFor(() => expect(screen.queryByText('Deep Ocean S01E01')).toBeNull())

    const bulkCalls = vi.mocked(api.PUT).mock.calls
      .filter(call => call[0] === '/api/v1/subscriptions/{id}/items/pending')
    expect(bulkCalls).toHaveLength(1)
    const request = bulkCalls[0]?.[1] as unknown as { params: { path: { id: string } } }
    expect(request.params.path.id).toBe('sub-1')
    expect(screen.getByText('Nature 2160p')).toBeTruthy()
  })

  it('names the count and the subscription before queueing everything', async () => {
    await openList()
    await fireEvent.click(screen.getAllByRole('button', { name: 'Queue all' })[0] as HTMLElement)
    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(confirmed.mock.calls[0]?.[0]).toMatchObject({
      title: 'Queue all hits from “Weekly documentaries”?',
      description: '2 hits will be queued.'
    })
  })

  it('queues all open hits across pages and confirms the server total', async () => {
    items['sub-1'] = Array.from({ length: 75 }, (_, index) =>
      hit(`item-${index}`, `Release ${index}`))
    await openList('Release 0')
    await fireEvent.click(screen.getAllByRole('button', { name: 'Queue all' })[0] as HTMLElement)

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(confirmed.mock.calls[0]?.[0].description).toBe('75 hits will be queued.')
    expect(vi.mocked(api.PUT).mock.calls
      .filter(call => call[0] === '/api/v1/subscriptions/{id}/items/pending')).toHaveLength(1)
  })

  it('loads later open hits through server-side pagination', async () => {
    items['sub-1'] = Array.from({ length: 75 }, (_, index) =>
      hit(`item-${index}`, `Release ${index}`))
    await openList('Release 0')
    expect(screen.queryByText('Release 50')).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))

    await waitFor(() => expect(screen.getByText('Release 50')).toBeTruthy())
    const pageCall = vi.mocked(api.GET).mock.calls.find(call =>
      call[0] === '/api/v1/subscriptions/{id}/items/page'
      && (call[1] as { params: { query: { offset: number } } }).params.query.offset === 50)
    expect(pageCall).toBeTruthy()
  })

  it('checks every indexer subscription with one press, and no other kind (RD-106-17)', async () => {
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    // The box — and with it the button — arrives with the first answer (RD-107-12).
    await waitFor(() => expect(screen.getByRole('button', { name: 'Check all' })).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Check all' }))

    await waitFor(() => expect(polledIds()).toEqual(['sub-1', 'sub-2', 'sub-3']))
    // The box is open, so the hits a check turns up have somewhere to appear.
    expect(screen.getByText('Weekly documentaries')).toBeTruthy()
    // Started, deliberately not finished: the server answers before the poll has run.
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Check of 3 subscriptions started'))
  })

  it('retranslates a visible check notice when the interface language changes', async () => {
    i18n.global.locale.value = 'de'
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await waitFor(() => expect(screen.getByRole('button', { name: 'Alle prüfen' })).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Alle prüfen' }))
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Prüfung für 3 Abos gestartet'))

    i18n.global.locale.value = 'en'
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Check of 3 subscriptions started'))
  })

  it('uses the singular check notice for one indexer subscription', async () => {
    i18n.global.locale.value = 'de'
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions') return { data: [rows[0]] }
      if (path === '/api/v1/subscriptions/review-summary') {
        return { data: {
          pending_total: 2,
          subscriptions: [{ subscription_id: 'sub-1', pending: 2 }]
        } }
      }
      return { data: [] }
    }) as never)
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await waitFor(() => expect(screen.getByRole('button', { name: 'Alle prüfen' })).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Alle prüfen' }))

    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Prüfung für 1 Abo gestartet'))
  })

  it('says how many checks could not be started', async () => {
    vi.mocked(api.POST).mockImplementation((async (_path: string, options: { params: { path: { id: string } } }) =>
      options.params.path.id === 'sub-2' ? { error: { code: 'unreachable' } } : { data: undefined }
    ) as never)
    await openList()

    await fireEvent.click(screen.getByRole('button', { name: 'Check all' }))

    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('1 check could not be started'))
  })

  it('does nothing when the confirmation is declined', async () => {
    confirmed.mockImplementation(async () => false)
    await openList()
    await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss all' })[0] as HTMLElement)
    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(vi.mocked(api.PUT)).not.toHaveBeenCalled()
    expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy()
  })

  it('suppresses the review hint at zero and shows nothing of the groups', async () => {
    items = { 'sub-1': [], 'sub-2': [], 'sub-3': [] }
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await waitFor(() => expect(screen.getByText('Indexer subscriptions')).toBeTruthy())
    expect(screen.queryByText('Weekly documentaries')).toBeNull()
    expect(screen.queryByText('Hits still waiting for a decision')).toBeNull()
  })

  /**
   * At zero the drawer would open on an empty room, and opening it costs a page load per
   * subscription to be told so. The badge, the hint and the button answer the same question,
   * so they appear together (RD-109-29). "Check all" stays: it is what produces hits in the
   * first place, and it is useless only if there are no indexer subscriptions at all — which
   * is when the whole box is absent.
   */
  it('offers no way into the drawer while there is nothing to review (RD-109-29)', async () => {
    items = { 'sub-1': [], 'sub-2': [], 'sub-3': [] }
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await settled()

    expect(screen.getByText('Indexer subscriptions')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Check all' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Review' })).toBeNull()
  })

  it('brings the button back when a hit arrives, without a reload (RD-109-29)', async () => {
    items = { 'sub-1': [], 'sub-2': [], 'sub-3': [] }
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await settled()
    expect(screen.queryByRole('button', { name: 'Review' })).toBeNull()

    // A check running elsewhere writes a hit; the event says only that something changed, and
    // the summary is re-read. The button has to follow it, or the hits stay unreachable until
    // somebody reloads the page by hand — the failure RD-109-28 fixed for the rows.
    items['sub-1'] = [hit('item-1', 'Deep Ocean S01E01')]
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
  })

  /**
   * The drawer stays shut until somebody opens it, and a poll finishing does not open it either.
   *
   * This reverses RD-106-18, which had the panel unfold itself as soon as there were hits. That
   * was right while it was a box in the page and wrong the moment it held enough to be worth
   * reading: every hit that arrived grew it and pushed the LinkGrabber's own rows down the page,
   * so a row somebody was reading moved out from under the pointer. The count in the header is
   * what announces the hits now.
   */
  it('never opens itself, not on arrival and not when a poll finds something', async () => {
    render(IndexerReviewList, { global: { plugins: [i18n], components } })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
    expect(screen.queryByText('Weekly documentaries')).toBeNull()

    subscriptionEvent?.({
      data: JSON.stringify({ payload: {
        poll: 'finished',
        subscription_id: 'sub-1',
        found: 1,
        accepted: 1,
        skipped: 0
      } })
    } as MessageEvent<string>)
    await settled()
    expect(screen.queryByText('Weekly documentaries')).toBeNull()
  })

  it('is absent entirely when no indexer subscription is set up (RD-107-12)', async () => {
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      // A media channel and nothing else: a kind this box has no business showing.
      if (path === '/api/v1/subscriptions') return { data: [subscription('sub-4', 'A channel', 'media')] }
      if (path === '/api/v1/subscriptions/review-summary') return { data: { pending_total: 0, subscriptions: [] } }
      return { data: [] }
    }) as never)
    const { container } = render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await settled()

    expect(screen.queryByText('Indexer subscriptions')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Check all' })).toBeNull()
    // Not merely empty: no frame, no title, no button.
    expect(container.querySelector('section')).toBeNull()
  })

  it('shows the box with at least one indexer subscription, and its two buttons (RD-106-18)', async () => {
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await settled()

    expect(screen.getByText('Indexer subscriptions')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Check all' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy()
    // What RD-106-18 wanted — the hits being noticed — is the header's business now; the groups
    // themselves wait behind the button.
    expect(screen.queryByText('Weekly documentaries')).toBeNull()
  })

  it('stays hidden while the first fetch is outstanding, rather than flashing', async () => {
    let release: () => void = () => {}
    const listed = new Promise<void>(resolve => { release = resolve })
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions') {
        await listed
        return { data: rows }
      }
      if (path === '/api/v1/subscriptions/review-summary') return { data: { pending_total: 0, subscriptions: [] } }
      return { data: [] }
    }) as never)
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await nextTick()
    await nextTick()
    expect(screen.queryByText('Indexer subscriptions')).toBeNull()

    release()
    await waitFor(() => expect(screen.getByText('Indexer subscriptions')).toBeTruthy())
  })

  it('appears without a reload once the first indexer subscription exists', async () => {
    let live: Subscription[] = []
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions') return { data: live }
      if (path === '/api/v1/subscriptions/review-summary') return { data: { pending_total: 0, subscriptions: [] } }
      return { data: [] }
    }) as never)
    render(IndexerReviewList, { global: { plugins: [i18n], components } })

    await settled()
    expect(screen.queryByText('Indexer subscriptions')).toBeNull()

    live = [subscription('sub-1', 'Weekly documentaries')]
    // The subscription was written elsewhere; the store re-reads the list on the event.
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(screen.getByText('Indexer subscriptions')).toBeTruthy(), { timeout: 2000 })
  })
})

/**
 * The badge counts hits, the group lists them, and until RD-109-28 only the badge followed the
 * event stream: a subscription announced thirty-two hits over a group that opened empty and
 * stayed empty until the page was reloaded by hand.
 */
describe('IndexerReviewList: the rows follow the same event as the count', () => {
  it('shows a hit that arrives while the group is open, without a reload', async () => {
    await openList()
    expect(screen.getByText('2 hits')).toBeTruthy()

    // Written by a check that ran elsewhere; the event says only that something changed.
    items['sub-1']?.push(hit('item-5', 'Deep Ocean S01E03'))
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(screen.getByText('Deep Ocean S01E03')).toBeTruthy())
    expect(screen.getByText('3 hits')).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Dismiss' })).toHaveLength(3)
  })

  it('fills a group that was opened while the check was still running', async () => {
    // The race from the report: the badge is already 2 because the hits are written, but the
    // page read the click sent out crossed that write and answers with nothing.
    let staleRead: (value: unknown) => void = () => {}
    const stale = new Promise(resolve => { staleRead = resolve })
    let pageReads = 0
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/subscriptions') return { data: rows }
      if (path === '/api/v1/subscriptions/review-summary') {
        return { data: { pending_total: 2, subscriptions: [{ subscription_id: 'sub-1', pending: 2 }] } }
      }
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        pageReads += 1
        if (pageReads === 1) return stale
        return { data: {
          items: items['sub-1'],
          total: 2,
          counts: { pending: 2, queued: 0, dismissed: 0, skipped: 0 },
          run_total: 0
        } }
      }
      return { data: [] }
    }) as never)

    render(IndexerReviewList, { global: { plugins: [i18n], components } })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Review' }))
    await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
    await fireEvent.click(screen.getByLabelText('Weekly documentaries'))
    await waitFor(() => expect(pageReads).toBe(1))

    // The check ends while that read is still out, and only then does it answer — empty.
    subscriptionEvent?.({
      data: JSON.stringify({ payload: { poll: 'finished', subscription_id: 'sub-1', found: 2, accepted: 2, skipped: 0 } })
    } as MessageEvent<string>)
    staleRead({ data: {
      items: [],
      total: 0,
      counts: { pending: 0, queued: 0, dismissed: 0, skipped: 0 },
      run_total: 0
    } })

    // Nothing is collapsed and reopened: the group fills where it stands.
    await waitFor(() => expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy())
    expect(screen.getByText('Deep Ocean S01E02')).toBeTruthy()
  })

  /**
   * The report behind RD-110-30, replayed: every hit rejected from the second page, the drawer
   * closed, a new hit written by a check, the drawer opened again. The badge counted the hit
   * and the group stayed empty until the page was reloaded by hand, because the list kept
   * reading the page it had been rejected from — page two of a subscription with one hit.
   */
  it('shows the hits that arrive after every one was rejected from the last page (RD-110-30)', async () => {
    items['sub-1'] = Array.from({ length: 75 }, (_, index) =>
      hit(`item-${index}`, `Release ${index}`))
    await openList('Release 0')
    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))
    await waitFor(() => expect(screen.getByText('Release 50')).toBeTruthy())

    await fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss all' })[0] as HTMLElement)
    await waitFor(() => expect(screen.queryByText('Weekly documentaries')).toBeNull())
    // The write's own event, as the server sends it.
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    await waitFor(() => expect(screen.queryByText('Nature 2160p')).toBeNull())

    // A check running elsewhere writes one hit and then records its run.
    items['sub-1']?.push(hit('item-new', 'Deep Ocean S02E01'))
    subscriptionEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
    subscriptionEvent?.({
      data: JSON.stringify({ payload: { poll: 'finished', subscription_id: 'sub-1', found: 1, accepted: 1, skipped: 0 } })
    } as MessageEvent<string>)

    await fireEvent.click(screen.getByRole('button', { name: 'Review' }))
    await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
    await fireEvent.click(screen.getByLabelText('Weekly documentaries'))
    await waitFor(() => expect(screen.getByText('Deep Ocean S02E01')).toBeTruthy())
    // One row, decidable — not the dismissed ones and not page two of a list with one hit.
    expect(screen.getAllByRole('button', { name: 'Dismiss' })).toHaveLength(1)
  })

  it('says a page read failed instead of showing an empty group, and can be asked again', async () => {
    let broken = true
    const working = vi.mocked(api.GET).getMockImplementation() as unknown as
      (path: string, options?: unknown) => Promise<unknown>
    vi.mocked(api.GET).mockImplementation((async (path: string, options?: unknown) => {
      if (broken && path === '/api/v1/subscriptions/{id}/items/page') {
        return { error: { code: 'unreachable' } }
      }
      return working(path, options)
    }) as never)

    render(IndexerReviewList, { global: { plugins: [i18n], components } })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Review' }))
    await waitFor(() => expect(screen.getByText('Weekly documentaries')).toBeTruthy())
    await fireEvent.click(screen.getByLabelText('Weekly documentaries'))

    await waitFor(() => expect(screen.getByRole('alert').textContent)
      .toContain('The subscription could not be reached'))
    expect(screen.queryByText('Deep Ocean S01E01')).toBeNull()

    broken = false
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }))

    await waitFor(() => expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy())
    expect(screen.queryByRole('alert')).toBeNull()
  })
})

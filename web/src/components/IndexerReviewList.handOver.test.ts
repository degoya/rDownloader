/**
 * RD-1460-03: a hit of an indexer subscription goes straight to a remote-job provider.
 *
 * Owner, 2026-11-22: a button on the hit, as the LinkGrabber hands an NZB over. Offered on an NZB
 * hit only, one entry per account whose provider takes NZB files, in the list and in the cards;
 * the hit leaves the review box as after "queue", a refusal leaves it and names it, and the
 * settings switch takes the offer away.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, eventStreamMock, toastMock } from '@/test/mocks'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import { handOverSubscriptionItem } from '@/api/subscriptionHandOver'
import type { Subscription, SubscriptionItem } from '@/api/types'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent, passthrough, uiStubs } from '@/test/mount'
import { setShowNzbHandOver } from '@/utils/nzbHandOver'

import IndexerReviewList from './IndexerReviewList.vue'

vi.mock('@/api/client', () => apiClientMock({ responseError: vi.fn(() => 'failed'), errorMessage: vi.fn() }))
vi.mock('@/api/subscriptionHandOver', () => ({ handOverSubscriptionItem: vi.fn() }))
vi.mock('@/composables/useEventStream', () => eventStreamMock())
vi.mock('@/composables/useConfirm', () => confirmMock())
const toasted = vi.hoisted(() => vi.fn())
vi.mock('@nuxt/ui/composables', () => toastMock({ add: toasted }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, linkgrabber, subscriptions } } })
const components = {
  ...uiStubs,
  UButton: { props: ['label', 'disabled', 'loading'], template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>' },
  UBadge: passthrough,
  UIcon: passthrough,
  UDrawer: { props: ['open'], template: '<div v-if="open"><slot name="body" /></div>' },
  // Every entry as a button beside the trigger, which is what choosing one comes down to.
  UDropdownMenu: {
    props: ['items'],
    template: '<div><slot /><button v-for="entry in items" :key="entry.label" @click="entry.onSelect()">{{ entry.label }}</button></div>'
  }
}

let view: Subscription['view'] = 'list'
let items: SubscriptionItem[] = []

function hit(id: string, title: string, media_type: string | null): SubscriptionItem {
  return { id, title, state: 'pending', media_type, discovered_at: '2026-11-22T08:00:00Z' } as SubscriptionItem
}

const pending = () => items.filter(item => item.state === 'pending')

beforeEach(() => {
  setActivePinia(createPinia())
  vi.clearAllMocks()
  view = 'list'
  items = [
    hit('item-nzb', 'Deep Ocean S01E01', 'application/x-nzb'),
    hit('item-torrent', 'Alpine Winter', 'application/x-bittorrent')
  ]
  vi.mocked(api.GET).mockImplementation((async (path: string) => {
    if (path === '/api/v1/subscriptions') {
      return { data: [{ id: 'sub-1', name: 'Weekly documentaries', kind: 'indexer', enabled: true, view }] }
    }
    if (path === '/api/v1/subscriptions/review-summary') {
      return { data: { pending_total: pending().length, subscriptions: [{ subscription_id: 'sub-1', pending: pending().length }] } }
    }
    if (path === '/api/v1/subscriptions/{id}/items/page') {
      return { data: {
        items: pending(),
        total: pending().length,
        counts: { pending: pending().length, queued: 0, dismissed: 0, skipped: 0 },
        run_total: 0,
        newest_discovered_at: '2026-11-22T08:00:00Z'
      } }
    }
    if (path === '/api/v1/accounts') return { data: [{ id: 'acc-torbox', label: 'Main', provider: 'torbox', enabled: true }] }
    if (path === '/api/v1/remote-jobs/providers') return { data: ['torbox'] }
    if (path === '/api/v1/providers') return { data: [{ slug: 'torbox', display_name: 'TorBox', credentials: 'api_key', kind: 'remote' }] }
    return { data: [] }
  }) as never)
  vi.mocked(handOverSubscriptionItem).mockImplementation(async (itemId: string) => {
    const found = items.find(item => item.id === itemId)
    if (found) found.state = 'queued'
    return { ok: true, data: { item_id: itemId, state: 'queued', job: {} as never, already_running: false } }
  })
})

afterEach(() => setShowNzbHandOver({}))

async function openGroup(): Promise<void> {
  mountComponent(IndexerReviewList, { i18n, stubs: components, keepPinia: true })
  await waitFor(() => expect(screen.getByRole('button', { name: 'Review' })).toBeTruthy())
  await fireEvent.click(screen.getByRole('button', { name: 'Review' }))
  await waitFor(() => expect(screen.getByLabelText('Weekly documentaries')).toBeTruthy())
  await fireEvent.click(screen.getByLabelText('Weekly documentaries'))
  await waitFor(() => expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy())
}

describe('IndexerReviewList: hand a hit to a provider (RD-1460-03)', () => {
  it('offers the NZB hit to the account that takes NZBs, and the hit leaves once it went', async () => {
    await openGroup()

    await waitFor(() => expect(screen.getAllByTestId('subscription-item-hand-over')).toHaveLength(1))
    await fireEvent.click(screen.getByRole('button', { name: 'Main · TorBox' }))

    await waitFor(() => expect(handOverSubscriptionItem).toHaveBeenCalledWith('item-nzb', 'acc-torbox'))
    await waitFor(() => expect(screen.queryByText('Deep Ocean S01E01')).toBeNull())
    expect(screen.getByText('Alpine Winter')).toBeTruthy()
    await waitFor(() => expect(toasted).toHaveBeenCalledWith(expect.objectContaining({ title: '“Deep Ocean S01E01” handed to TorBox', color: 'success' })))
  })

  it('keeps a hit the provider refused, and names it with the reason', async () => {
    vi.mocked(handOverSubscriptionItem).mockResolvedValue({
      ok: false,
      status: 502,
      message: { code: 'provider.refused', message: 'The provider refused the NZB', params: {} }
    })
    await openGroup()

    await waitFor(() => expect(screen.getByRole('button', { name: 'Main · TorBox' })).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: 'Main · TorBox' }))

    await waitFor(() => expect(toasted).toHaveBeenCalledWith(expect.objectContaining({
      title: '“Deep Ocean S01E01” could not be handed over',
      description: 'The provider refused the NZB',
      color: 'error'
    })))
    expect(screen.getByText('Deep Ocean S01E01')).toBeTruthy()
  })

  it('offers it on the cards too, on the NZB hit alone', async () => {
    view = 'cards'
    await openGroup()

    await waitFor(() => expect(screen.getAllByTestId('subscription-item-hand-over')).toHaveLength(1))
  })

  it('takes the offer away when the settings switch is off', async () => {
    setShowNzbHandOver({ nzb_hand_over_indexers_enabled: false })
    await openGroup()

    expect(screen.queryByTestId('subscription-item-hand-over')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Main · TorBox' })).toBeNull()
  })
})

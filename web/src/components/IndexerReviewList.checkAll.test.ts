/**
 * RD-1460-03: "check all" asks only the indexer subscriptions that are switched on.
 *
 * Owner, 2026-11-22: pressing it polled the switched-off subscriptions too. A switched-off one
 * stays out, the notice counts only those asked, and with every one switched off the button is
 * off. A single subscription's own "check now" is the subscriptions page's and stays as it is.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, eventStreamMock, toastMock } from '@/test/mocks'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Subscription } from '@/api/types'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import { mountComponent, passthrough, settle, uiStubs } from '@/test/mount'

import IndexerReviewList from './IndexerReviewList.vue'

vi.mock('@/api/client', () => apiClientMock({ responseError: vi.fn(() => 'failed'), errorMessage: vi.fn() }))
vi.mock('@/composables/useEventStream', () => eventStreamMock())
vi.mock('@/composables/useConfirm', () => confirmMock())
vi.mock('@nuxt/ui/composables', () => toastMock())

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, linkgrabber, subscriptions } } })
const components = {
  ...uiStubs,
  UButton: { props: ['label', 'disabled', 'loading'], template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>' },
  UBadge: passthrough,
  UIcon: passthrough,
  UDrawer: { props: ['open'], template: '<div v-if="open"><slot name="body" /></div>' }
}

let rows: Subscription[] = []

function indexer(id: string, enabled: boolean): Subscription {
  return { id, name: `Indexer ${id}`, kind: 'indexer', enabled } as Subscription
}

beforeEach(() => {
  setActivePinia(createPinia())
  vi.clearAllMocks()
  rows = [
    indexer('on-1', true),
    indexer('off-1', false),
    indexer('on-2', true),
    // Switched on, but no indexer: not this box's business at all.
    { id: 'media-1', name: 'A channel', kind: 'media', enabled: true } as Subscription
  ]
  vi.mocked(api.GET).mockImplementation((async (path: string) => {
    if (path === '/api/v1/subscriptions') return { data: rows }
    if (path === '/api/v1/subscriptions/review-summary') return { data: { pending_total: 0, subscriptions: [] } }
    return { data: [] }
  }) as never)
  vi.mocked(api.POST).mockResolvedValue({ data: undefined } as never)
})

function polledIds(): string[] {
  return vi.mocked(api.POST).mock.calls
    .filter(call => call[0] === '/api/v1/subscriptions/{id}/poll')
    .map(call => (call[1] as { params: { path: { id: string } } }).params.path.id)
}

async function checkAllButton(): Promise<HTMLButtonElement> {
  mountComponent(IndexerReviewList, { i18n, stubs: components, keepPinia: true })
  await waitFor(() => expect(screen.getByTestId('indexer-check-all')).toBeTruthy())
  return screen.getByTestId('indexer-check-all') as HTMLButtonElement
}

describe('IndexerReviewList: check all (RD-1460-03)', () => {
  it('polls only the switched-on indexer subscriptions and counts only those', async () => {
    const button = await checkAllButton()
    expect(button.disabled).toBe(false)

    await fireEvent.click(button)

    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Check of 2 subscriptions started'))
    expect(polledIds()).toEqual(['on-1', 'on-2'])
  })

  it('is off, and asks nothing, while every indexer subscription is switched off', async () => {
    rows = [indexer('off-1', false), indexer('off-2', false)]
    const button = await checkAllButton()

    expect(button.disabled).toBe(true)
    expect(button.getAttribute('title')).toBe('Every indexer subscription is switched off – switch one on to check it here.')
    await fireEvent.click(button)
    await settle()
    expect(polledIds()).toEqual([])
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('comes back on when a subscription is switched on, without a reload', async () => {
    rows = [indexer('off-1', false)]
    const button = await checkAllButton()
    expect(button.disabled).toBe(true)

    useSubscriptionsStore().subscriptions = [indexer('off-1', true)]

    await waitFor(() => expect((screen.getByTestId('indexer-check-all') as HTMLButtonElement).disabled).toBe(false))
    await fireEvent.click(screen.getByTestId('indexer-check-all'))
    await waitFor(() => expect(polledIds()).toEqual(['off-1']))
  })
})

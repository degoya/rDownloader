/**
 * The archive of an expanded subscription: long names (RD-1150-03), queueing again (RD-1150-04).
 *
 * A long release name ran the archive off the card's right edge, the history header with it.
 * jsdom lays nothing out, so the width is asserted by the classes that decide it: the archive
 * is a shrinkable flex item of the subscription row (`min-w-0`), and every hit row — in every
 * state the archive lists — truncates its title inside a shrinkable box and wraps the rest.
 * Those classes hold at any window width, 390 px and 1440 px alike.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SubscriptionItem } from '@/api/types'
import type { ConfirmOptions } from '@/composables/useConfirm'
import { resetEventStream } from '@/composables/useEventStream'
import common from '@/locales/en/common.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: (...args: unknown[]) => put(...args),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  errorMessage: vi.fn(() => 'The service did not answer'),
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: vi.fn() }) })
}))
vi.stubGlobal('EventSource', class {
  addEventListener(): void {}
  close(): void {}
})

afterEach(() => resetEventStream())

const { default: SubscriptionsView } = await import('@/views/SubscriptionsView.vue')
const { default: SubscriptionArchive } = await import('./SubscriptionArchive.vue')

/** Hands `ui.content` to the content box, as Nuxt UI does; the shared stub drops it. */
const UCollapsible = {
  props: { open: { type: Boolean, default: undefined }, ui: { type: Object, default: undefined } },
  emits: ['update:open'],
  template:
    '<div v-bind="$attrs"><div @click="$emit(\'update:open\', !open)"><slot :open="open" /></div>'
    + '<div v-if="open" data-collapsible-content :class="ui?.content"><slot name="content" /></div></div>'
}

const LONG = 'Some.Very.Long.Release.Name.2026.German.DL.1080p.WEB.h264-GROUP_NSW-SUXXORS.part01.rar'
const STATES = ['pending', 'queued', 'dismissed', 'skipped'] as const

function hit(state: SubscriptionItem['state']): SubscriptionItem {
  return {
    id: `item-${state}`,
    subscription_id: 's1',
    item_key: `key-${state}`,
    title: `${LONG}.${state}`,
    url: `https://indexer.test/get/${state}.nzb`,
    published_at: null,
    duration_seconds: null,
    state,
    reason: state === 'skipped' ? 'title_excluded' : null,
    media_type: 'application/x-nzb',
    discovered_at: '2026-10-07T10:00:00Z'
  } as SubscriptionItem
}

const subscription = {
  id: 's1',
  name: 'scnlog - NSW',
  url: 'https://indexer.test/api',
  kind: 'indexer',
  enabled: true,
  mode: 'review',
  interval_seconds: 3600
}

describe('SubscriptionArchive, long release names', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation((path: string) => {
      if (path === '/api/v1/subscriptions') return Promise.resolve({ data: [subscription] })
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        const items = STATES.map(hit)
        return Promise.resolve({
          data: { items, total: items.length, counts: { pending: 1, queued: 1, dismissed: 1, skipped: 1 }, run_total: 0 }
        })
      }
      return Promise.resolve({ data: [] })
    })
  })

  async function openArchive(): Promise<HTMLElement> {
    mountComponent(SubscriptionsView, { messages: { subscriptions, common }, stubs: { UCollapsible } })
    await fireEvent.click(await screen.findByRole('button', { name: subscriptions.actions.details }))
    await screen.findByText(`${LONG}.dismissed`)
    return screen.getByText(subscriptions.history.action).closest('[data-collapsible-content]') as HTMLElement
  }

  it('lets the archive shrink to the card instead of growing to its longest name', async () => {
    const archive = await openArchive()
    expect(archive.className).toContain('min-w-0')
    expect(archive.className).toContain('basis-full')
  })

  it.each(STATES)('truncates a long name of a %s hit inside a box that may shrink, and wraps the rest', async (state) => {
    const archive = await openArchive()
    const title = within(archive).getByText(`${LONG}.${state}`)
    expect(title.className).toContain('truncate')
    expect(title.getAttribute('title')).toBe(`${LONG}.${state}`)
    expect((title.parentElement as HTMLElement).className).toContain('min-w-0')
    const row = title.closest('li') as HTMLElement
    expect((row.firstElementChild as HTMLElement).className).toContain('flex-wrap')
  })

  it('keeps the history header wrapping, so its action stays inside the card', async () => {
    const archive = await openArchive()
    const header = screen.getByText(subscriptions.history.action).closest('div') as HTMLElement
    expect(archive.contains(header)).toBe(true)
    expect(header.className).toContain('flex-wrap')
  })
})

describe('SubscriptionArchive, queueing again (RD-1150-04)', () => {
  const rows = [
    hit('pending'),
    hit('queued'),
    hit('dismissed'),
    hit('skipped'),
    { ...hit('dismissed'), id: 'item-nothing', item_key: 'nothing', title: 'Nothing.To.Fetch', url: 'urn:release:nothing' }
  ]

  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    confirmed.mockClear()
    get.mockImplementation((path: string) => {
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        return Promise.resolve({
          data: { items: rows, total: rows.length, counts: { pending: 1, queued: 1, dismissed: 2, skipped: 1 }, run_total: 0 }
        })
      }
      return Promise.resolve({ data: [] })
    })
  })

  function mount() {
    return mountComponent(SubscriptionArchive, { props: { subscription }, messages: { subscriptions, common } })
  }

  function rowOf(title: string): HTMLElement {
    return screen.getByText(title).closest('li') as HTMLElement
  }

  function requeueBody(call: number): unknown {
    const [path, options] = post.mock.calls[call] as [string, { params: unknown, body: unknown }]
    expect(path).toBe('/api/v1/subscriptions/{id}/items/requeue')
    expect(options.params).toEqual({ path: { id: 's1' } })
    return options.body
  }

  it('offers queueing again for every decided hit and keeps queue and dismiss for a waiting one', async () => {
    mount()
    await screen.findByText(`${LONG}.dismissed`)
    for (const state of ['queued', 'dismissed', 'skipped']) {
      const row = rowOf(`${LONG}.${state}`)
      expect(within(row).getByRole('button', { name: subscriptions.actions.requeue })).toBeTruthy()
      expect(within(row).getByRole('checkbox')).toBeTruthy()
    }
    const waiting = rowOf(`${LONG}.pending`)
    expect(within(waiting).getByRole('button', { name: subscriptions.actions.queue })).toBeTruthy()
    expect(within(waiting).getByRole('button', { name: subscriptions.actions.dismiss })).toBeTruthy()
    expect(within(waiting).queryByRole('button', { name: subscriptions.actions.requeue })).toBeNull()
    expect(within(waiting).queryByRole('checkbox')).toBeNull()
  })

  it('switches the action off for a hit with nothing to fetch, and says why', async () => {
    mount()
    const row = (await screen.findByText('Nothing.To.Fetch')).closest('li') as HTMLElement
    const button = within(row).getByRole('button', { name: subscriptions.actions.requeue }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(button.getAttribute('title')).toBe(subscriptions.requeue.no_source)
    expect(within(row).queryByRole('checkbox')).toBeNull()
  })

  it('queues one dismissed hit again from its row and says so', async () => {
    post.mockResolvedValue({ data: { requeued: ['item-dismissed'], refused: [] } })
    const { emitted } = mount()
    await screen.findByText(`${LONG}.dismissed`)
    await fireEvent.click(within(rowOf(`${LONG}.dismissed`)).getByRole('button', { name: subscriptions.actions.requeue }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(requeueBody(0)).toEqual({ item_ids: ['item-dismissed'], allow_duplicate: false })
    expect(confirmed).not.toHaveBeenCalled()
    await waitFor(() => expect(emitted().notice).toEqual([[{ text: '1 entry queued again.', tone: 'info' }]]))
  })

  it('queues the ticked hits again together, and the page checkbox ticks every one that can go', async () => {
    post.mockResolvedValue({ data: { requeued: ['item-queued', 'item-skipped'], refused: [] } })
    mount()
    await screen.findByText(`${LONG}.dismissed`)
    await fireEvent.click(within(rowOf(`${LONG}.queued`)).getByRole('checkbox'))
    await fireEvent.click(within(rowOf(`${LONG}.skipped`)).getByRole('checkbox'))
    await fireEvent.click(screen.getByRole('button', { name: 'Queue selected again (2)' }))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(requeueBody(0)).toEqual({ item_ids: ['item-queued', 'item-skipped'], allow_duplicate: false })
    // What went is no longer ticked.
    await screen.findByRole('button', { name: 'Queue selected again (0)' })

    await fireEvent.click(screen.getByRole('checkbox', { name: subscriptions.actions.select_page }))
    expect(screen.getByRole('button', { name: 'Queue selected again (3)' })).toBeTruthy()
  })

  it('asks before doubling an address that is still there, and queues it anyway only on a yes', async () => {
    post
      .mockResolvedValueOnce({
        data: {
          requeued: [],
          refused: [{ item_id: 'item-queued', code: 'subscription.item_duplicate', message: 'still there' }]
        }
      })
      .mockResolvedValueOnce({ data: { requeued: ['item-queued'], refused: [] } })
    const { emitted } = mount()
    await screen.findByText(`${LONG}.queued`)
    await fireEvent.click(within(rowOf(`${LONG}.queued`)).getByRole('button', { name: subscriptions.actions.requeue }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(2))
    expect(confirmed).toHaveBeenCalledTimes(1)
    expect(confirmed.mock.calls[0]?.[0].title).toBe(subscriptions.requeue.duplicate_title)
    expect(requeueBody(1)).toEqual({ item_ids: ['item-queued'], allow_duplicate: true })
    await waitFor(() => expect(emitted().notice).toEqual([[{ text: '1 entry queued again.', tone: 'info' }]]))
  })

  it('leaves a duplicate alone on a no, and says what was not queued', async () => {
    confirmed.mockResolvedValueOnce(false)
    post.mockResolvedValueOnce({
      data: {
        requeued: [],
        refused: [{ item_id: 'item-queued', code: 'subscription.item_duplicate', message: 'still there' }]
      }
    })
    const { emitted } = mount()
    await screen.findByText(`${LONG}.queued`)
    await fireEvent.click(within(rowOf(`${LONG}.queued`)).getByRole('button', { name: subscriptions.actions.requeue }))

    await waitFor(() => expect(emitted().notice).toBeTruthy())
    expect(post).toHaveBeenCalledTimes(1)
    const [[notice]] = emitted().notice as [[{ text: string, tone: string }]]
    expect(notice.tone).toBe('error')
    expect(notice.text).toContain('0 queued again, 1 not')
  })
})

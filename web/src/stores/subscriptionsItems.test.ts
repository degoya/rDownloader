/**
 * The subscriptions' hit lists (RD-140-27) on their own: a page read and its failures, a page
 * that no longer exists, and the re-read a `subscription.changed` event triggers through
 * `noteChange` (RD-110-30).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useSubscriptionItems } from './subscriptionsItems'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn() },
  responseError: vi.fn(() => 'The subscription does not exist'),
  errorMessage: vi.fn(() => 'The service did not answer')
}))

function hits(ids: string[], total: number) {
  return { data: { items: ids.map(id => ({ id })), total } } as never
}

type PageQuery = { params: { path: { id: string }, query: { state: string, limit: number, offset: number } } }

function queries(): PageQuery['params']['query'][] {
  return vi.mocked(api.GET).mock.calls
    .filter(call => call[0] === '/api/v1/subscriptions/{id}/items/page')
    .map(call => (call[1] as PageQuery).params.query)
}

describe('subscription hit lists', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
  })

  it('reads one page of a list and records what was asked', async () => {
    vi.mocked(api.GET).mockResolvedValue(hits(['h1', 'h2'], 2))
    const lists = useSubscriptionItems()

    expect(await lists.loadItems('s1', 'all')).toBeNull()

    expect(queries()).toEqual([{ state: 'all', limit: 50, offset: 0 }])
    expect(lists.items.value.s1?.map(item => item.id)).toEqual(['h1', 'h2'])
    expect(lists.itemQueries.value.s1).toEqual({ state: 'all', page: 1, pages: 1 })
    expect(lists.itemErrors.value.s1).toBeNull()
  })

  it('reads the last page that exists instead of an empty one', async () => {
    vi.mocked(api.GET)
      .mockResolvedValueOnce(hits([], 3))
      .mockResolvedValueOnce(hits(['h1', 'h2', 'h3'], 3))
    const lists = useSubscriptionItems()

    await lists.loadItems('s1', 'pending', 2)

    expect(queries().map(query => query.offset)).toEqual([50, 0])
    expect(lists.itemQueries.value.s1?.page).toBe(1)
    expect(lists.items.value.s1).toHaveLength(3)
  })

  it('names a refused read and a lost connection on the list, not as an empty list', async () => {
    const lists = useSubscriptionItems()
    vi.mocked(api.GET).mockResolvedValueOnce({ error: { code: 'subscriptions.not_found' } } as never)
    expect(await lists.loadItems('gone')).toBe('The subscription does not exist')
    expect(lists.itemErrors.value.gone).toBe('The subscription does not exist')
    expect(lists.items.value.gone).toBeUndefined()

    vi.mocked(api.GET).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    expect(await lists.loadItems('s1')).toBe('The service did not answer')
    expect(lists.itemErrors.value.s1).toBe('The service did not answer')
  })

  it('re-reads a wanted list when a finished poll wrote rows for it, and only that one', async () => {
    vi.mocked(api.GET).mockResolvedValue(hits(['h1'], 1))
    const lists = useSubscriptionItems()
    await lists.loadItems('s1')
    await lists.loadItems('s2')
    vi.mocked(api.GET).mockClear()
    vi.mocked(api.GET).mockResolvedValue(hits(['h1', 'h2'], 2))

    lists.noteChange({ lost: false, wroteRowsFor: 's1' })
    await vi.waitFor(() => expect(lists.items.value.s1).toHaveLength(2))

    const asked = vi.mocked(api.GET).mock.calls.map(call => (call[1] as PageQuery).params.path.id)
    expect(asked).toEqual(['s1'])
    expect(lists.items.value.s2).toHaveLength(1)
  })

  it('re-reads every wanted list when the stream lost events', async () => {
    vi.mocked(api.GET).mockResolvedValue(hits(['h1'], 1))
    const lists = useSubscriptionItems()
    await lists.loadItems('s1')
    await lists.loadItems('s2')
    vi.mocked(api.GET).mockClear()

    lists.noteChange({ lost: true, wroteRowsFor: null })
    await vi.waitFor(() => expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(2))

    const asked = vi.mocked(api.GET).mock.calls.map(call => (call[1] as PageQuery).params.path.id)
    expect(asked.sort()).toEqual(['s1', 's2'])
  })
})

/**
 * The download history (RD-1100-04) as the page reads it: the filters it sends, the pages it
 * keeps and the refusal it shows.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useHistoryStore } from './history'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

function entry(id: number) {
  return { id, name: `Package ${id}`, outcome: 'completed' } as never
}

/** One page as `openapi-fetch` hands it over, with the total the server names beside it. */
function page(ids: number[], total: number) {
  return {
    data: ids.map(entry),
    response: new Response(null, { headers: { 'x-total-count': String(total) } })
  } as never
}

describe('history store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('asks with only the filters that are set and keeps both counts', async () => {
    vi.mocked(api.GET).mockImplementation(((_path: string, init: { params: { query: { limit: number } } }) =>
      Promise.resolve(init.params.query.limit === 1 ? page([1], 120) : page([1, 2], 2))) as never)
    const store = useHistoryStore()
    store.filters.search = '  ubuntu  '
    store.filters.outcome = 'failed'

    await store.refresh()

    const [, first] = vi.mocked(api.GET).mock.calls[0] as unknown as [string, { params: { query: Record<string, unknown> } }]
    expect(first.params.query).toEqual({ limit: 50, offset: 0, q: 'ubuntu', outcome: 'failed' })
    expect(store.entries).toHaveLength(2)
    expect(store.total).toBe(2)
    expect(store.stored).toBe(120)
    expect(store.error).toBeNull()
    expect(store.loading).toBe(false)
  })

  it('appends the next page behind the last entry shown and stops at the total', async () => {
    vi.mocked(api.GET)
      .mockResolvedValueOnce(page([1, 2], 3))
      .mockResolvedValueOnce(page([1, 2], 3))
      .mockResolvedValueOnce(page([3], 3))
    const store = useHistoryStore()
    await store.refresh()

    await store.loadMore()
    await store.loadMore()

    const offsets = vi.mocked(api.GET).mock.calls
      .map(call => (call[1] as { params: { query: { offset?: number } } }).params.query.offset)
    expect(offsets).toEqual([0, undefined, 2])
    expect(store.entries.map(item => (item as { id: number }).id)).toEqual([1, 2, 3])
  })

  it('keeps what it showed and names the refusal when a read fails', async () => {
    vi.mocked(api.GET).mockResolvedValue(page([1], 1))
    const store = useHistoryStore()
    await store.refresh()
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'history.invalid_query' } } as never)

    await store.refresh()

    expect(store.error).toBe('The service did not answer')
    expect(store.entries).toHaveLength(1)
  })

  it('answers whether a re-add went and leaves a refusal in the error', async () => {
    const store = useHistoryStore()
    vi.mocked(api.POST).mockResolvedValueOnce({ error: { code: 'history.not_found' } } as never)
    expect(await store.readd(entry(7))).toBe(false)
    expect(store.error).toBe('The service did not answer')

    vi.mocked(api.POST).mockResolvedValueOnce({ data: { added: 1 } } as never)
    expect(await store.readd(entry(7))).toBe(true)
    expect(vi.mocked(api.POST)).toHaveBeenLastCalledWith('/api/v1/history/{id}/readd', { params: { path: { id: 7 } } })
    expect(store.error).toBeNull()
  })

  it('builds the export link from the filters that are set (RD-1240-14)', () => {
    const store = useHistoryStore()
    expect(store.exportHref('csv')).toBe('/api/v1/history/export?format=csv')
    store.filters.search = ' ubuntu '
    store.filters.outcome = 'failed'
    store.filters.kind = 'usenet'
    expect(store.exportHref('ndjson')).toBe('/api/v1/history/export?format=ndjson&q=ubuntu&outcome=failed&kind=usenet')
  })
})

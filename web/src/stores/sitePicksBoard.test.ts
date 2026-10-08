/**
 * The pick board (RD-1170-03) as the LinkGrabber mirrors it: asked again while a page
 * resolves, a page's new state put in place, and a refusal shown.
 */
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useSitePicksStore } from './sitePicks'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The list has no entry with that index')
}))

function pick(id: string, running: boolean) {
  return { id, running, total: running ? 1 : 0, finished: 0, entries: [] } as never
}

describe('sitePicks store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.DELETE).mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('asks again while a page resolves and stops once none does', async () => {
    vi.mocked(api.GET)
      .mockResolvedValueOnce({ data: { pages: [pick('a', true)] } } as never)
      .mockResolvedValue({ data: { pages: [pick('a', false)] } } as never)
    const store = useSitePicksStore()

    await store.listed()
    expect(store.asked).toBe(1)
    expect(store.running).toBe(true)

    await vi.advanceTimersByTimeAsync(1_500)
    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(2)
    expect(store.running).toBe(false)

    await vi.advanceTimersByTimeAsync(10_000)
    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(2)
  })

  it('puts the started page in place and keeps the page when a resolve is refused', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: { pages: [pick('a', false), pick('b', false)] } } as never)
    const store = useSitePicksStore()
    await store.refresh()

    expect(await store.resolve('a', [])).toBe(false)
    expect(vi.mocked(api.POST)).not.toHaveBeenCalled()

    vi.mocked(api.POST).mockResolvedValueOnce({ error: { code: 'site_rules.no_entry' } } as never)
    expect(await store.resolve('a', [99])).toBe(false)
    expect(store.error).toBe('The list has no entry with that index')
    expect(store.pages[0]?.running).toBe(false)
    expect(store.busy.size).toBe(0)

    vi.mocked(api.POST).mockResolvedValueOnce({ data: pick('a', true) } as never)
    expect(await store.resolve('a', [0])).toBe(true)
    expect(vi.mocked(api.POST)).toHaveBeenLastCalledWith(
      '/api/v1/collector/picks/{id}/resolve',
      { params: { path: { id: 'a' } }, body: { entries: [0] } }
    )
    expect(store.error).toBeNull()
    expect(store.pages.map(page => [page.id, page.running])).toEqual([['a', true], ['b', false]])
    store.stop()
  })

  it('names the refusal when the board cannot be read', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'auth.scope_insufficient' } } as never)
    const store = useSitePicksStore()

    await store.refresh()

    expect(store.error).toBe('The list has no entry with that index')
    expect(store.pages).toEqual([])
  })

  it('discards a page from the board once the service did', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: { pages: [pick('a', false), pick('b', false)] } } as never)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { code: 'site_rules.pick_discarded', message: 'The list was discarded' } } as never)
    const store = useSitePicksStore()
    await store.refresh()

    await store.discard('a')

    expect(store.pages.map(page => page.id)).toEqual(['b'])
    expect(store.error).toBeNull()
  })
})

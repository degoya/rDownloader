/**
 * The Newznab indexers (RD-180-19) as the settings card, the search and the subscription form
 * read them: sorted, the enabled ones apart, and each one's caps asked once.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useIndexersStore } from './indexers'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

function indexer(id: string, name: string, enabled: boolean) {
  return { id, name, enabled } as never
}

describe('indexers store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('keeps the list sorted by name and the enabled ones apart', async () => {
    vi.mocked(api.GET).mockResolvedValue({
      data: [indexer('b', 'Zeta', true), indexer('a', 'alpha', false), indexer('c', 'Beta', true)]
    } as never)
    const store = useIndexersStore()

    expect(await store.refresh()).toBeNull()

    expect(store.loaded).toBe(true)
    expect(store.indexers.map(item => item.name)).toEqual(['alpha', 'Beta', 'Zeta'])
    expect(store.enabled.map(item => item.id)).toEqual(['c', 'b'])
  })

  it('answers the refusal and stays unloaded when the list cannot be read', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'auth.scope_insufficient' } } as never)
    const store = useIndexersStore()

    expect(await store.refresh()).toBe('The service did not answer')

    expect(store.loaded).toBe(false)
    expect(store.indexers).toEqual([])
  })

  it('asks each indexer for its caps once, keeps a failed test as null and forgets them on a refresh', async () => {
    vi.mocked(api.POST).mockImplementation(((_path: string, init: { params: { path: { id: string } } }) =>
      Promise.resolve(init.params.path.id === 'a'
        ? { data: { search: ['search', 'tvsearch'] } }
        : { error: { code: 'indexers.caps_failed' } })) as never)
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
    const store = useIndexersStore()

    await Promise.all([store.loadCaps(['a', 'b']), store.loadCaps(['a'])])
    await store.loadCaps(['a', 'b'])

    expect(vi.mocked(api.POST)).toHaveBeenCalledTimes(2)
    expect(store.caps.a).toEqual({ search: ['search', 'tvsearch'] })
    expect(store.caps.b).toBeNull()

    await store.refresh()
    expect(store.caps).toEqual({})
  })
})

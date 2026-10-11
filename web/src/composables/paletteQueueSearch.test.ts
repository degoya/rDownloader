/**
 * The search palette's queue rows (RD-1240-14): what it asks the server and when, the groups it
 * draws from the answer, and the jump a choice makes.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'

import { api } from '@/api/client'
import { router } from '@/router'

import { PALETTE_QUEUE_LIMIT, queueSearchGroups, usePaletteQueueSearch } from './paletteQueueSearch'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn() } }))

const t = (key: string): string => key

const HITS = {
  packages: [{ id: 'p1', name: 'Ubuntu Desktop', state: 'queued' }],
  downloads: [{ id: 'd1', package_id: 'p1', package_name: 'Ubuntu Desktop', file_name: 'ubuntu.iso', state: 'queued' }]
}

beforeEach(() => vi.mocked(api.GET).mockReset())
afterEach(() => vi.useRealTimers())

describe('queueSearchGroups', () => {
  it('draws packages and files as groups the palette does not filter again', () => {
    const groups = queueSearchGroups(t, HITS as never)
    expect(groups.map(group => [group.id, group.label, group.ignoreFilter])).toEqual([
      ['queue-packages', 'nav.search.groups.packages', true],
      ['queue-downloads', 'nav.search.groups.downloads', true]
    ])
    expect(groups[1]?.items?.[0]).toMatchObject({ id: 'file:d1', label: 'ubuntu.iso', suffix: 'Ubuntu Desktop' })
    expect(queueSearchGroups(t, null)).toEqual([])
    expect(queueSearchGroups(t, { packages: [], downloads: [] } as never)).toEqual([])
  })

  it('opens the download list on the chosen row', () => {
    const push = vi.spyOn(router, 'push').mockResolvedValue(undefined)
    const [packages, downloads] = queueSearchGroups(t, HITS as never)
    packages?.items?.[0]?.onSelect?.(new Event('select'))
    downloads?.items?.[0]?.onSelect?.(new Event('select'))
    expect(push.mock.calls).toEqual([
      [{ path: '/downloads', query: { reveal: 'package:p1' } }],
      [{ path: '/downloads', query: { reveal: 'file:d1' } }]
    ])
    push.mockRestore()
  })
})

describe('usePaletteQueueSearch', () => {
  it('asks a moment after the last key, from two characters on, bounded', async () => {
    vi.useFakeTimers()
    vi.mocked(api.GET).mockResolvedValue({ data: HITS } as never)
    const term = ref('')
    const scope = effectScope()
    const search = scope.run(() => usePaletteQueueSearch(term))
    if (!search) throw new Error('no search')

    term.value = 'u'
    await nextTick()
    await vi.advanceTimersByTimeAsync(300)
    expect(api.GET).not.toHaveBeenCalled()

    term.value = 'ub'
    await nextTick()
    term.value = 'ubu'
    await nextTick()
    await vi.advanceTimersByTimeAsync(300)
    expect(api.GET).toHaveBeenCalledTimes(1)
    expect(api.GET).toHaveBeenCalledWith('/api/v1/queue/search', { params: { query: { q: 'ubu', limit: PALETTE_QUEUE_LIMIT } } })
    expect(search.hits.value).toEqual(HITS)
    expect(search.loading.value).toBe(false)

    term.value = ''
    await nextTick()
    await vi.advanceTimersByTimeAsync(300)
    expect(search.hits.value).toBeNull()
    scope.stop()
  })
})

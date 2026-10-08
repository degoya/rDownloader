import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useCollectorStore } from './collector'

vi.mock('@/api/client', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/client')>(),
  api: { GET: vi.fn(), POST: vi.fn(), PATCH: vi.fn(), DELETE: vi.fn() }
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useNotifications', () => ({ useNotifications: () => ({ notify: vi.fn() }) }))

const ids = (count: number) => Array.from({ length: count }, (_, index) => `c-${index}`)
const bodies = () => vi.mocked(api.POST).mock.calls.map(call => (call[1] as { body: Record<string, unknown> & { ids: string[] } }).body)

/** The LinkGrabber's bulk routes take at most 500 ids too; a larger selection goes in batches. */
describe('collector store: selections larger than one request', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('enqueues 733 packages in two requests and adds up the result', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ data: { created: [{}, {}], failed: 1, first_error: 'first', free_download_files: 3 } } as never)
      .mockResolvedValueOnce({ data: { created: [{}], failed: 2, first_error: 'second', free_download_files: 4 } } as never)
    const store = useCollectorStore()

    const result = await store.enqueuePackages(ids(733), true)

    expect(bodies().map(body => body.ids.length)).toEqual([500, 233])
    expect(bodies()[1]).toMatchObject({ paused: true })
    expect(result).toEqual({ created: 3, links: 0, failed: 3, firstError: 'first', freeDownloadFiles: 7 })
    expect(store.error).toBeNull()
  })

  it('reports partial success when the second enqueue batch is refused', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ data: { created: [{}], failed: 0, first_error: null, free_download_files: 0 } } as never)
      .mockResolvedValueOnce({ error: { error: 'refused', code: 'unknown.code' } } as never)
    const store = useCollectorStore()

    const result = await store.enqueuePackages(ids(733))

    expect(result.created).toBe(1)
    expect(store.error).toBe('refused – stopped after part 1 of 2; the parts before it were applied.')
  })

  it('moves 733 links into one new package, not one per batch', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ data: { id: 'pkg-new' } } as never)
      .mockResolvedValueOnce({ data: { id: 'pkg-new' } } as never)
    const store = useCollectorStore()

    expect(await store.moveCandidates(ids(733), { newPackageName: 'Fresh' })).toBe(true)

    expect(bodies()[0]).toMatchObject({ new_package_name: 'Fresh' })
    expect(bodies()[1]).toEqual({ ids: ids(733).slice(500), package_id: 'pkg-new' })
  })

  it('changes 733 packages in two bulk requests', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: [] } as never)
    const store = useCollectorStore()

    expect(await store.updatePackages(ids(733), { priority: 'high' })).toBe(true)

    expect(bodies().map(body => body.ids.length)).toEqual([500, 233])
    expect(bodies()[1]).toMatchObject({ priority: 'high' })
  })

  it('counts the links that went with the enqueued packages, not the packages', async () => {
    const store = useCollectorStore()
    store.candidates = ['a', 'b', 'c'].flatMap(pkg => [1, 2].map(n => ({ id: `${pkg}${n}`, package_id: pkg }))) as never
    vi.mocked(api.POST).mockResolvedValue({ data: { created: [{}, {}], failed: 0, first_error: null, free_download_files: 0 } } as never)

    const result = await store.enqueuePackages(['a', 'b'], false, ['a1', 'a2', 'b1'])

    expect(result).toMatchObject({ created: 2, links: 3 })
    expect(store.candidates.map(candidate => candidate.id)).toEqual(['b2', 'c1', 'c2'])
  })
})

/** No route deletes a chosen set of links; 392 of them one after another took 70 s. */
describe('collector store: deleting many links', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.DELETE).mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('deletes 1042 links four at a time, reports progress and reads the list once', async () => {
    const store = useCollectorStore()
    store.candidates = ids(1042).map(id => ({ id, package_id: 'p' })) as never
    let inFlight = 0
    let peak = 0
    vi.mocked(api.DELETE).mockImplementation((async () => {
      inFlight++
      peak = Math.max(peak, inFlight)
      await Promise.resolve()
      inFlight--
      return { data: { message: 'ok' } }
    }) as never)
    const progress: number[] = []

    const removed = await store.deleteCandidates(ids(1042), done => progress.push(done))

    expect(removed).toBe(1042)
    expect(api.DELETE).toHaveBeenCalledTimes(1042)
    expect(peak).toBe(4)
    expect(progress.at(-1)).toBe(1042)
    // One refresh: the packages, the links and the NZB imports, not three reads per link.
    expect(vi.mocked(api.GET).mock.calls.length).toBeLessThan(10)
    expect(store.deletingIds.size).toBe(0)
  })

  it('goes on past a refused link and says why it stayed', async () => {
    const store = useCollectorStore()
    vi.mocked(api.DELETE)
      .mockResolvedValueOnce({ error: { error: 'busy', code: 'unknown.code' } } as never)
      .mockResolvedValue({ data: { message: 'ok' } } as never)

    expect(await store.deleteCandidates(ids(3))).toBe(2)
    expect(store.error).toBe('busy')
  })
})

/** RD-1170-03: a paste that only listed a series page is no failure; the pick board holds it. */
describe('collector store: a page that waits for a choice', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: { pages: [] } } as never)
  })

  it('answers how many releases were listed and asks the board instead of showing an error', async () => {
    vi.mocked(api.POST).mockResolvedValueOnce({
      error: { code: 'site_rules.pick_waiting', error: 'listed', params: { list: 'p1', entries: '32', rule: 'serienjunkies.org' } }
    } as never)
    const store = useCollectorStore()

    const outcome = await store.collect({ text: 'https://serienjunkies.org/serie/show/' })

    expect(outcome).toMatchObject({ ok: true, listed: 32 })
    expect(store.error).toBeNull()
    expect(vi.mocked(api.GET).mock.calls.map(call => call[0])).toContain('/api/v1/collector/picks')
  })
})

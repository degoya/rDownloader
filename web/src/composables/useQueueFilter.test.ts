/**
 * The download list's filter and name search (RD-190-21): which files each filter keeps, what
 * the search matches, and that both live in the address without typing over the field.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, type EffectScope } from 'vue'

import type { Download, DownloadPackage, DownloadState } from '@/api/types'

import { filterQueue, SEARCH_DEBOUNCE_MS, useQueueFilter } from './useQueueFilter'

/** The route the composable reads, reactive like the real one, and what it asked the router. */
const router = vi.hoisted(() => ({
  route: null as unknown as { path: string, hash: string, query: Record<string, string | undefined> },
  replace: null as unknown as ReturnType<typeof vi.fn>
}))
vi.mock('vue-router', async () => {
  const { reactive } = await import('vue')
  router.route = reactive({ path: '/downloads', hash: '', query: {} })
  router.replace = vi.fn(async ({ query }: { query: Record<string, string | undefined> }) => {
    router.route.query = { ...query }
  })
  return { useRoute: () => router.route, useRouter: () => ({ replace: router.replace }) }
})

function file(id: string, packageId: string, fileName: string, state: DownloadState = 'queued'): Download {
  return { id, package_id: packageId, file_name: fileName, state } as unknown as Download
}

function pkg(id: string, name: string): DownloadPackage {
  return { id, name } as unknown as DownloadPackage
}

const packages = [pkg('p1', 'Ubuntu ISO'), pkg('p2', 'Holiday Photos')]
const downloads = [
  file('a', 'p1', 'ubuntu-24.04.iso', 'downloading'),
  file('b', 'p1', 'SHA256SUMS', 'retry_wait'),
  file('c', 'p2', 'beach.jpg', 'failed'),
  file('d', 'p2', 'sunset.jpg', 'blocked'),
  file('e', 'p2', 'dinner.jpg', 'cancelled'),
  file('f', 'p2', 'pier.jpg', 'paused'),
  file('g', 'p2', 'torrent.mkv', 'seeding'),
  file('h', 'p2', 'done.jpg', 'completed'),
  file('i', 'p2', 'mirror.jpg', 'skipped')
]

const ids = (list: Download[]) => list.map(download => download.id)

describe('filterQueue', () => {
  it('hands the store array back untouched without a filter or a search', () => {
    expect(filterQueue(downloads, packages, 'all', '  ')).toBe(downloads)
  })

  it('keeps the states each filter names', () => {
    expect(ids(filterQueue(downloads, packages, 'active', ''))).toEqual(['a'])
    // Waiting for the next attempt is waiting too.
    expect(ids(filterQueue(downloads, packages, 'queued', ''))).toEqual(['b'])
    expect(ids(filterQueue(downloads, packages, 'paused', ''))).toEqual(['f'])
    // Everything that stopped short and wants a look.
    expect(ids(filterQueue(downloads, packages, 'failed', ''))).toEqual(['c', 'd', 'e'])
    expect(ids(filterQueue(downloads, packages, 'seeding', ''))).toEqual(['g'])
    expect(ids(filterQueue(downloads, packages, 'completed', ''))).toEqual(['h'])
  })

  it('matches the file name, ignoring case', () => {
    expect(ids(filterQueue(downloads, packages, 'all', 'JPG'))).toEqual(['c', 'd', 'e', 'f', 'h', 'i'])
    expect(ids(filterQueue(downloads, packages, 'all', 'sunset'))).toEqual(['d'])
  })

  it('keeps every file of a package whose name matches', () => {
    expect(ids(filterQueue(downloads, packages, 'all', 'ubuntu iso'))).toEqual(['a', 'b'])
  })

  it('applies the filter and the search together', () => {
    expect(ids(filterQueue(downloads, packages, 'failed', 'holiday'))).toEqual(['c', 'd', 'e'])
    expect(ids(filterQueue(downloads, packages, 'failed', 'beach'))).toEqual(['c'])
    expect(filterQueue(downloads, packages, 'completed', 'ubuntu')).toEqual([])
  })
})

describe('useQueueFilter', () => {
  let scope: EffectScope | null = null

  /** One composable per case, its watchers stopped afterwards so no case hears another's route. */
  function setup(): ReturnType<typeof useQueueFilter> {
    scope = effectScope()
    const result = scope.run(() => useQueueFilter())
    if (!result) throw new Error('no composable')
    return result
  }

  beforeEach(() => {
    vi.useFakeTimers()
    router.route.query = {}
    router.replace.mockClear()
  })
  afterEach(() => {
    scope?.stop()
    scope = null
    vi.useRealTimers()
  })

  it('opens on the filter and the search the address names', () => {
    router.route.query = { filter: 'failed', q: ' beach ' }
    const { filter, search, needle, active } = setup()
    expect(filter.value).toBe('failed')
    expect(search.value).toBe('beach')
    expect(needle.value).toBe('beach')
    expect(active.value).toBe(true)
  })

  it('reads an unknown filter as all', () => {
    router.route.query = { filter: 'everything' }
    const { filter, active } = setup()
    expect(filter.value).toBe('all')
    expect(active.value).toBe(false)
  })

  it('writes the filter into the address, and leaves it out for all', async () => {
    const { filter } = setup()
    filter.value = 'seeding'
    await nextTick()
    expect(router.replace).toHaveBeenLastCalledWith(expect.objectContaining({ path: '/downloads', query: { filter: 'seeding' } }))

    filter.value = 'all'
    await nextTick()
    expect(router.replace).toHaveBeenLastCalledWith(expect.objectContaining({ query: {} }))
  })

  it('narrows only after the typing pauses, and writes the search once', async () => {
    const { search, needle } = setup()
    search.value = 'u'
    search.value = 'ub'
    search.value = 'ubu'
    await nextTick()
    expect(needle.value).toBe('')

    vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS)
    await nextTick()
    expect(needle.value).toBe('ubu')
    expect(router.replace).toHaveBeenCalledTimes(1)
    expect(router.route.query).toEqual({ q: 'ubu' })
  })

  it('does not type over the field when an older search lands in the address', async () => {
    const { search, needle } = setup()
    search.value = 'ubu'
    await nextTick()
    vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS)
    await nextTick()
    // The replace for "ubu" has landed while the field already says more.
    search.value = 'ubuntu'
    await nextTick()
    expect(search.value).toBe('ubuntu')
    expect(needle.value).toBe('ubu')
  })

  it('follows a link opened while the list is showing', async () => {
    const { filter, search, needle } = setup()
    router.route.query = { filter: 'failed', q: 'beach' }
    await nextTick()
    expect(filter.value).toBe('failed')
    expect(search.value).toBe('beach')
    expect(needle.value).toBe('beach')
  })

  it('resets to the whole list', async () => {
    router.route.query = { filter: 'paused', q: 'pier' }
    const { filter, search, active, reset } = setup()
    reset()
    await nextTick()
    expect(filter.value).toBe('all')
    expect(search.value).toBe('')
    expect(active.value).toBe(false)
    expect(router.route.query).toEqual({})
  })
})

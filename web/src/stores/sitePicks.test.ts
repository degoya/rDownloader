/**
 * The pick board's mirror in the interface (RD-1190-17): a list that vanished under the drawer is
 * listed again by *Fetch*, let go quietly by *Stop* and *Discard*, and a page another intake
 * listed opens the drawer where a panel is shown and is kept for the next one where none is.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { CollectorPick, CollectorPickEntry } from '@/api/types'

const get = vi.fn()
const post = vi.fn()
const remove = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    DELETE: (...args: unknown[]) => remove(...args)
  },
  responseError: vi.fn(() => 'That pick list no longer exists; add the page again.')
}))

const { useSitePicksStore } = await import('./sitePicks')

function entry(index: number, label: string, state = 'pending'): CollectorPickEntry {
  return { index, label, attributes: {}, state, code: null, links: 0 }
}

function page(id: string, entries: CollectorPickEntry[], extra: Partial<CollectorPick> = {}): CollectorPick {
  return {
    id,
    rule: 'serienjunkies.org',
    rule_id: 'serienjunkies',
    address: 'https://serienjunkies.org/serie/the-show/',
    package_name: 'The Show',
    created_at: '2026-10-08T20:00:00Z',
    running: false,
    total: 0,
    finished: 0,
    waiting_for_captcha: false,
    entries,
    ...extra
  }
}

const gone = { error: { code: 'site_rules.pick_not_found', error: 'gone', params: { reason: 'unknown' } } }

describe('the pick board in the interface', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('lists a vanished page again on Fetch and fetches the same release from the fresh list', async () => {
    const picks = useSitePicksStore()
    picks.pages = [page('old', [entry(0, 'The.Show.S01E01'), entry(1, 'The.Show.S01E02')])]
    // The page lists its releases in another order now.
    const fresh = page('new', [entry(0, 'The.Show.S01E03'), entry(1, 'The.Show.S01E02'), entry(2, 'The.Show.S01E01')])
    post.mockImplementation(async (path: string, init: { body?: { entries?: number[] }, params?: { path?: { id?: string } } }) => {
      if (path === '/api/v1/collector/picks/{id}/resolve' && init.params?.path?.id === 'old') return gone
      if (path === '/api/v1/collector/picks') return { data: fresh }
      if (path === '/api/v1/collector/picks/{id}/resolve') {
        return { data: page('new', fresh.entries.map(item => init.body?.entries?.includes(item.index) ? { ...item, state: 'queued' } : item), { running: true, total: 1 }) }
      }
      throw new Error(`unexpected ${path}`)
    })

    expect(await picks.resolve('old', [1])).toBe(true)

    expect(post).toHaveBeenCalledWith('/api/v1/collector/picks', { body: { address: 'https://serienjunkies.org/serie/the-show/' } })
    expect(post).toHaveBeenLastCalledWith('/api/v1/collector/picks/{id}/resolve', { params: { path: { id: 'new' } }, body: { entries: [1] } })
    expect(picks.pages.map(item => item.id)).toEqual(['new'])
    expect(picks.pages[0]?.entries[1]?.state).toBe('queued')
    expect(picks.error).toBeNull()
    picks.stop()
  })

  it('lets a vanished list go without a word on Stop and on Discard', async () => {
    const picks = useSitePicksStore()
    picks.pages = [page('a', [entry(0, 'A')]), page('b', [entry(0, 'B')])]
    post.mockResolvedValue(gone)
    remove.mockResolvedValue(gone)

    await picks.cancel('a')
    await picks.discard('b')

    expect(picks.pages).toEqual([])
    expect(picks.error).toBeNull()
  })

  it('opens the drawer where a panel is shown, and keeps the page for the next panel where none is', async () => {
    const picks = useSitePicksStore()
    get.mockResolvedValue({ data: { pages: [page('p1', [entry(0, 'A')])] } })
    const listing = { list: 'p1', entries: 1, rule: 'serienjunkies.org' }

    await picks.announced(listing)
    expect(picks.asked).toBe(0)
    expect(picks.notice).toMatchObject(listing)
    expect(picks.attach()).toBe(true)
    expect(picks.attach()).toBe(false)

    await picks.announced(listing)
    expect(picks.asked).toBe(1)
    picks.detach()
    picks.detach()
  })
})

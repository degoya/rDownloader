/**
 * Torrent details as the review and the detail panel read them: one cache entry per scope, the
 * failure on the torrent it belongs to, the pushed aggregates, and a move followed to its end.
 */
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTorrentsStore } from './torrents'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The torrent does not exist'),
  resultMessage: vi.fn(() => 'Done')
}))
vi.mock('@/i18n', () => ({ i18n: { global: { t: (key: string) => key } } }))

function detail(name: string, extra: Record<string, unknown> = {}) {
  return { data: { name, capabilities: { move: true }, ...extra } } as never
}

describe('torrents store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    for (const method of [api.GET, api.POST, api.PUT, api.DELETE]) vi.mocked(method).mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('keeps a candidate and a download with the same id apart', async () => {
    vi.mocked(api.GET).mockImplementation(((path: string) =>
      Promise.resolve(detail(path.startsWith('/api/v1/collector') ? 'candidate' : 'download'))) as never)
    const store = useTorrentsStore()

    await store.load('candidate', 'x')
    await store.load('download', 'x')

    expect(store.detail('candidate', 'x')?.name).toBe('candidate')
    expect(store.detail('download', 'x')?.name).toBe('download')
    expect(store.capabilities).toEqual({ move: true })
    expect(store.isBusy('candidate', 'x')).toBe(false)
  })

  it('puts a refused or failed load on that torrent alone and frees it again', async () => {
    const store = useTorrentsStore()
    vi.mocked(api.GET).mockResolvedValueOnce({ error: { code: 'torrent.not_found' } } as never)
    await store.load('download', 'a')
    expect(store.errorOf('download', 'a')).toBe('The torrent does not exist')
    expect(store.errorOf('download', 'b')).toBe('')

    vi.mocked(api.GET).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    await store.load('download', 'b')
    expect(store.errorOf('download', 'b')).toBe('The torrent does not exist')
    expect(store.isBusy('download', 'b')).toBe(false)
  })

  it('names a magnet whose metadata could not be fetched', async () => {
    const store = useTorrentsStore()
    vi.mocked(api.POST).mockResolvedValueOnce(detail('magnet', { metadata_state: 'failed', metadata_error: null }))

    await store.resolveMetadata('m')

    expect(store.detail('candidate', 'm')?.name).toBe('magnet')
    expect(store.errorOf('candidate', 'm')).toBe('torrent.errors.metadata_failed')
  })

  it('keeps the aggregates the event stream pushes, per torrent', () => {
    const store = useTorrentsStore()

    store.applyStats('a', { peers: 3 } as never)
    store.applyStats('a', { peers: 5 } as never)

    expect(store.statsOf('a')).toEqual({ peers: 5 })
    expect(store.statsOf('b')).toBeNull()
  })

  it('answers why an action was refused, and follows a move until it has ended', async () => {
    vi.useFakeTimers()
    const store = useTorrentsStore()
    vi.mocked(api.POST).mockResolvedValueOnce({ error: { code: 'torrent.not_running' } } as never)
    expect(await store.recheck('d')).toEqual({ message: null, error: 'The torrent does not exist' })

    vi.mocked(api.POST).mockResolvedValueOnce({ data: { code: 'torrent.move_started', message: '' } } as never)
    vi.mocked(api.GET)
      .mockResolvedValueOnce(detail('d', { relocation: { target: '/new' } }))
      .mockResolvedValue(detail('d', { relocation: null }))
    expect(await store.move('d', { target: '/new' } as never)).toEqual({ message: 'Done', error: null })

    await vi.advanceTimersByTimeAsync(2_000)
    expect(store.detail('download', 'd')?.relocation).toEqual({ target: '/new' })
    await vi.advanceTimersByTimeAsync(2_000)
    expect(store.detail('download', 'd')?.relocation).toBeNull()
    await vi.advanceTimersByTimeAsync(20_000)
    expect(vi.mocked(api.GET)).toHaveBeenCalledTimes(2)
  })
})

import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, eventStreamMock } from '@/test/mocks'

import type { Download } from '@/api/types'
import { setTitleStatus } from '@/utils/titleStatus'

const get = vi.fn()
vi.mock('@/api/client', () => apiClientMock({ GET: (...args: unknown[]) => get(...args), responseError: () => 'failed' }))
/** The handlers the store registered, so a test can play the server's events. */
const handlers: Record<string, (event: MessageEvent) => void> = {}
vi.mock('@/composables/useEventStream', () => eventStreamMock({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    Object.assign(handlers, registered)
    return () => {}
  }
}))
const notifications = { value: false }
vi.mock('@/composables/useNotifications', () => ({
  useNotifications: () => ({ enabled: notifications, notify: vi.fn() })
}))

const { useTransfersStore } = await import('./transfers')
const { HIDDEN_SPACING_MS, patchProgress } = await import('./transfersProgress')

type Entry = { id: string, state: string, committed_bytes: string, total_bytes: string | null }

function file(id: string, state = 'downloading', committed = '0', extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, package_id: 'pkg-1', file_name: `${id}.bin`, state, committed_bytes: committed, total_bytes: '1000', ...extra }
}

/** The service: its rows, and the rates it derives from them as the real one does. */
let rows: Record<string, unknown>[] = []
const AT_WORK = ['resolving', 'downloading', 'verifying', 'repairing', 'extracting']

function answer(path: string): { data: unknown } {
  if (path === '/api/v1/downloads') return { data: rows.map(row => ({ ...row })) }
  if (path === '/api/v1/packages') return { data: [{ id: 'pkg-1', name: 'Release', state: 'active', postprocess: null }] }
  const progress = rows.filter(row => AT_WORK.includes(row.state as string)).map((row): Entry => ({
    id: row.id as string,
    state: row.state as string,
    committed_bytes: row.committed_bytes as string,
    total_bytes: row.total_bytes as string | null
  }))
  return {
    data: { bytes_per_second: progress.length * 100, downloads: progress.map(entry => ({ id: entry.id, bytes_per_second: 100 })), progress, waiting_for_host: [], usenet_servers: [] }
  }
}

function emit(name: string, payload?: Record<string, unknown>): void {
  handlers[name]?.(new MessageEvent(name, { data: JSON.stringify(payload ? { payload } : { dropped: 3 }) }))
}

const progress = (id: string): void => emit('download.progress', { download_id: id })
const state = (id: string): void => emit('download.state', { download_id: id })
const reads = (path: string): number => get.mock.calls.filter(call => call[0] === path).length
const queueReads = (): number => reads('/api/v1/downloads')
const rateReads = (): number => reads('/api/v1/downloads/rates')

let visibility: DocumentVisibilityState = 'visible'
function show(next: DocumentVisibilityState): void {
  visibility = next
  document.dispatchEvent(new Event('visibilitychange'))
}

/** A store that has read the queue once and follows it, with the read counts back at zero. */
async function following(): Promise<ReturnType<typeof useTransfersStore>> {
  const store = useTransfersStore()
  await store.refresh()
  store.connectEvents()
  get.mockClear()
  return store
}

describe('transfers store: following the queue', () => {
  let store: ReturnType<typeof useTransfersStore>

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => visibility })
    visibility = 'visible'
    notifications.value = false
    setTitleStatus(true)
    rows = [file('a'), file('b', 'queued')]
    get.mockReset()
    get.mockImplementation(async (path: string) => answer(path))
  })

  afterEach(() => {
    store.disconnectEvents()
    vi.useRealTimers()
  })

  it('reads the figures alone on progress and patches the rows at work', async () => {
    store = await following()
    rows = [file('a', 'downloading', '600'), file('b', 'queued')]
    const resting = store.downloads[1]

    for (let burst = 0; burst < 5; burst += 1) progress('a')
    await vi.advanceTimersByTimeAsync(400)

    expect(queueReads()).toBe(0)
    expect(reads('/api/v1/packages')).toBe(0)
    expect(rateReads()).toBe(1)
    expect(store.downloads[0]?.committed_bytes).toBe('600')
    expect(store.downloads[1]).toBe(resting)
    expect(store.totalCommitted).toBe(600n)
    expect(store.downloadRates).toEqual({ a: 100 })
  })

  it('reads the figures alone for a Usenet transfer', async () => {
    rows = [file('a', 'downloading', '0', { nzb_file_id: 'nzb-1' })]
    store = await following()

    emit('usenet.changed', { nzb_segment_id: 's-1', state: 'downloading' })
    emit('usenet.changed', { resource: 'nzb_checkpoint', nzb_file_id: 'nzb-1' })
    await vi.advanceTimersByTimeAsync(400)

    expect(rateReads()).toBe(1)
    expect(queueReads()).toBe(0)
  })

  it('reads nothing for a torrent\'s own state, which no row shows', async () => {
    rows = [file('a', 'seeding', '1000', { kind: 'torrent' })]
    store = await following()
    emit('download.state', { download_id: 'a', torrent: true })
    await vi.advanceTimersByTimeAsync(5_000)
    expect(get).not.toHaveBeenCalled()

    emit('download.state', { download_id: 'a', torrent: true, state: 'completed', previous: 'seeding' })
    await vi.advanceTimersByTimeAsync(400)
    expect(queueReads()).toBe(1)
  })

  it.each([
    ['a state', () => state('a')],
    ['a package', () => emit('package.state', { package_id: 'pkg-1' })],
    ['any other Usenet change', () => emit('usenet.changed', { nzb_import_id: 'i-1' })],
    ['a deleted category', () => emit('category.changed', { resource: 'category' })],
    ['the lag marker, which names no row', () => emit('download.progress')],
    ['progress of a file the list does not have', () => progress('new')],
    ['progress of a file that rests', () => progress('b')],
    ['a recording, which changes more than bytes', () => emit('download.progress', { download_id: 'a', recording: true })]
  ])('reads the queue on %s', async (_name, event) => {
    store = await following()
    progress('a')
    event()
    await vi.advanceTimersByTimeAsync(400)
    expect(queueReads()).toBe(1)
  })

  it('reads the queue when the figures do not fit the list, without an event saying so', async () => {
    store = await following()
    // Finished, and the state event was lost: `a` is no longer among the rows at work.
    rows = [file('a', 'completed', '1000'), file('b', 'queued')]
    progress('a')
    await vi.advanceTimersByTimeAsync(400)
    expect(queueReads()).toBe(0)
    expect(store.downloads[0]?.state, 'left as it was until the list is read').toBe('downloading')

    await vi.advanceTimersByTimeAsync(400)
    expect(queueReads()).toBe(1)
    expect(store.downloads[0]?.state).toBe('completed')

    await vi.advanceTimersByTimeAsync(5_000)
    expect(queueReads(), 'once').toBe(1)
  })

  it('reads again for a state that changed while the queue was being read', async () => {
    store = await following()
    let release: (() => void) | undefined
    const gate = new Promise<void>((resolve) => { release = resolve })
    const before = answer('/api/v1/downloads')
    get.mockImplementationOnce(async () => { await gate; return before })
    state('a')
    await vi.advanceTimersByTimeAsync(400)

    rows = [file('a', 'completed', '1000'), file('b', 'downloading')]
    state('a')
    progress('a')
    await vi.advanceTimersByTimeAsync(400)
    expect(queueReads(), 'waits for the read on its way').toBe(1)
    release?.()
    await vi.advanceTimersByTimeAsync(400)

    expect(queueReads()).toBe(2)
    expect(store.downloads.map(row => row.state)).toEqual(['completed', 'downloading'])
  })

  it('drops figures that a read of the queue overtook', async () => {
    store = await following()
    let release: (() => void) | undefined
    const gate = new Promise<void>((resolve) => { release = resolve })
    const old = answer('/api/v1/downloads/rates')
    get.mockImplementationOnce(async () => { await gate; return old })
    progress('a')
    await vi.advanceTimersByTimeAsync(400)

    rows = [file('a', 'downloading', '900')]
    await store.refresh()
    release?.()
    await vi.advanceTimersByTimeAsync(0)

    expect(store.downloads.map(row => row.committed_bytes)).toEqual(['900'])
  })

  it('reads the queue on any event after a read that failed', async () => {
    store = await following()
    get.mockImplementation(async () => ({ error: {} }))
    state('a')
    await vi.advanceTimersByTimeAsync(400)
    expect(store.error).toBe('failed')

    rows = [file('a', 'paused', '700'), file('c', 'downloading')]
    get.mockImplementation(async (path: string) => answer(path))
    get.mockClear()
    progress('a')
    await vi.advanceTimersByTimeAsync(400)

    expect(queueReads()).toBe(1)
    expect(store.error).toBeNull()
    expect(store.downloads.map(row => row.id)).toEqual(['a', 'c'])
  })

  describe('in a hidden tab', () => {
    it('reads nothing when neither title nor notifications need it, and catches up', async () => {
      setTitleStatus(false)
      store = await following()
      const samples = store.speedHistory.length
      show('hidden')
      rows = [file('a', 'completed', '1000'), file('b', 'downloading', '5')]
      state('a')
      progress('b')
      await vi.advanceTimersByTimeAsync(60_000)
      expect(get).not.toHaveBeenCalled()
      expect(store.speedHistory.length, 'no samples of a rate nobody reads').toBe(samples)

      show('visible')
      await vi.advanceTimersByTimeAsync(0)
      expect(queueReads()).toBe(1)
      expect(store.downloads.map(row => row.state)).toEqual(['completed', 'downloading'])
    })

    it('catches up with the figures alone when only progress was missed', async () => {
      setTitleStatus(false)
      store = await following()
      show('hidden')
      rows = [file('a', 'downloading', '800'), file('b', 'queued')]
      progress('a')
      await vi.advanceTimersByTimeAsync(60_000)
      show('visible')
      await vi.advanceTimersByTimeAsync(0)

      expect(queueReads()).toBe(0)
      expect(store.downloads[0]?.committed_bytes).toBe('800')
    })

    it('asks for nothing when nothing was missed', async () => {
      store = await following()
      show('hidden')
      await vi.advanceTimersByTimeAsync(60_000)
      show('visible')
      await vi.advanceTimersByTimeAsync(1_000)
      expect(get).not.toHaveBeenCalled()
    })

    it('holds a read that was due when the tab went away', async () => {
      setTitleStatus(false)
      store = await following()
      state('a')
      show('hidden')
      await vi.advanceTimersByTimeAsync(5_000)
      expect(get).not.toHaveBeenCalled()
      show('visible')
      await vi.advanceTimersByTimeAsync(0)
      expect(queueReads()).toBe(1)
    })

    it('keeps the title current with the figures alone, spaced', async () => {
      store = await following()
      show('hidden')
      rows = [file('a', 'downloading'), file('b', 'downloading')]
      for (let at = 0; at < 20; at += 1) {
        state('b')
        progress('a')
        await vi.advanceTimersByTimeAsync(500)
      }
      expect(queueReads()).toBe(0)
      expect(rateReads()).toBeGreaterThan(0)
      expect(rateReads()).toBeLessThanOrEqual(10_000 / HIDDEN_SPACING_MS + 1)
      expect(store.activeCount).toBe(2)

      get.mockClear()
      await vi.advanceTimersByTimeAsync(60_000)
      expect(rateReads(), 'no loop over a list that stays owed').toBeLessThanOrEqual(1)
      show('visible')
      await vi.advanceTimersByTimeAsync(0)
      expect(queueReads()).toBe(1)
      expect(store.active.length).toBe(2)
    })

    it('keeps reading the queue for desktop notifications, spaced', async () => {
      notifications.value = true
      store = await following()
      show('hidden')
      for (let at = 0; at < 20; at += 1) {
        state('a')
        await vi.advanceTimersByTimeAsync(500)
      }
      expect(queueReads()).toBeGreaterThan(0)
      expect(queueReads()).toBeLessThanOrEqual(10_000 / HIDDEN_SPACING_MS + 1)
    })
  })
})

describe('patchProgress', () => {
  const row = (id: string, state: string, committed: string): Download => file(id, state, committed) as unknown as Download

  it('answers no rows when nothing moved', () => {
    const answer = patchProgress([row('a', 'downloading', '5')], [{ id: 'a', state: 'downloading', committed_bytes: '5', total_bytes: '1000' }])
    expect(answer).toEqual({ rows: null, stale: false })
  })

  it('takes a size that became known', () => {
    const unsized = { ...row('a', 'resolving', '0'), total_bytes: null }
    const { rows: next } = patchProgress([unsized], [{ id: 'a', state: 'resolving', committed_bytes: '0', total_bytes: '77' }])
    expect(next?.[0]?.total_bytes).toBe('77')
  })

  it('leaves a row in another state to the read of the list', () => {
    const rows = [row('a', 'downloading', '5')]
    expect(patchProgress(rows, [{ id: 'a', state: 'verifying', committed_bytes: '9', total_bytes: '1000' }]))
      .toEqual({ rows: null, stale: true })
  })

  it('is stale when a row at work is missing on either side', () => {
    expect(patchProgress([], [{ id: 'a', state: 'downloading', committed_bytes: '5', total_bytes: null }]).stale).toBe(true)
    expect(patchProgress([row('a', 'downloading', '5')], []).stale).toBe(true)
    expect(patchProgress([row('a', 'seeding', '5')], []).stale).toBe(false)
  })
})

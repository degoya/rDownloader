import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useStatsStore } from './stats'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The statistics are unavailable')
}))

let stateEvent: (() => void) | null = null
let usenetEvent: (() => void) | null = null
const released = vi.fn()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, () => void>) => {
    stateEvent = handlers['download.state'] ?? null
    usenetEvent = handlers['usenet.changed'] ?? null
    return () => { stateEvent = null; usenetEvent = null; released() }
  }
}))

const TRAFFIC = {
  servers: [{
    server_id: 's1', name: 'Block', enabled: true, today: 10, week: 20, month: 30, year: 40, total: 50,
    quota: { limit_bytes: 100, action: 'pause', used_bytes: 50, reset_on: null, reached_at: null }
  }]
}

/** Answers the traffic per server beside whatever `transfers` answers for the range. */
function routed(transfers: (range: string) => unknown) {
  return async (path: string, options?: unknown) => {
    if (path === '/api/v1/stats/usenet-servers') return { data: TRAFFIC } as never
    const range = (options as { params: { query: { range: string } } }).params.query.range
    return transfers(range) as never
  }
}

/** The reads of one path, whatever else the store asked for beside it. */
function calls(path: string): number {
  return (vi.mocked(api.GET).mock.calls as unknown as [string][]).filter(([called]) => called === path).length
}

function transferCalls(): number {
  return calls('/api/v1/stats/transfers')
}

const FIGURES = { completed: 0, failed: 0, retries: 0, bytes: 0, seconds: 0 }

function response(range: string) {
  return {
    range,
    resolution: range === 'day' ? 'hour' : 'day',
    since: '2026-09-19T12:00:00Z',
    buckets: [{ start: '2026-09-20T10:00:00Z', ...FIGURES, bytes: 42, completed: 1 }],
    by_kind: [],
    by_provider: [],
    totals: { ...FIGURES, bytes: 42, completed: 1 },
    all_time: { ...FIGURES, bytes: 42, completed: 1 }
  }
}

describe('stats store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    released.mockReset()
    stateEvent = null
    usenetEvent = null
    vi.useRealTimers()
  })

  it('asks for the chosen range and keeps the answer', async () => {
    vi.mocked(api.GET).mockImplementation(routed(range => ({ data: response(range) })))
    const store = useStatsStore()
    expect(store.loading).toBe(true)
    await store.refresh()
    expect(store.loading).toBe(false)
    expect(store.stats?.range).toBe('day')
    expect(api.GET).toHaveBeenLastCalledWith('/api/v1/stats/transfers', { params: { query: { range: 'day' } } })

    await store.setRange('week')
    expect(api.GET).toHaveBeenLastCalledWith('/api/v1/stats/transfers', { params: { query: { range: 'week' } } })
    expect(store.stats?.range).toBe('week')
  })

  it('reports a failure instead of showing an empty page as the truth', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'internal' } } as never)
    const store = useStatsStore()
    await store.refresh()
    expect(store.error).toBe('The statistics are unavailable')
    expect(store.settled).toBe(true)
    expect(store.stats).toBeNull()
  })

  it('drops a late answer for a range the reader has already left', async () => {
    const pending: { resolve?: (value: unknown) => void } = {}
    vi.mocked(api.GET).mockImplementation(routed(range => {
      if (range === 'day') return new Promise(resolve => { pending.resolve = resolve })
      return { data: response(range) }
    }))
    const store = useStatsStore()
    const day = store.refresh()
    await store.setRange('month')
    expect(store.stats?.range).toBe('month')
    pending.resolve?.({ data: response('day') })
    await day
    expect(store.stats?.range).toBe('month')
  })

  it('refreshes once for a burst of state events, and releases the stream on stop', async () => {
    vi.useFakeTimers()
    vi.mocked(api.GET).mockImplementation(routed(() => ({ data: response('day') })))
    const store = useStatsStore()
    store.start()
    expect(transferCalls()).toBe(1)
    stateEvent?.()
    stateEvent?.()
    stateEvent?.()
    await vi.advanceTimersByTimeAsync(2_500)
    expect(transferCalls()).toBe(2)
    store.stop()
    expect(released).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(120_000)
    expect(transferCalls()).toBe(2)
  })

  /** RD-1100-05: the per-server figures come with every read, and a quota event reads them again. */
  it('reads the traffic per Usenet server beside the range, and again on usenet.changed', async () => {
    vi.useFakeTimers()
    vi.mocked(api.GET).mockImplementation(routed(() => ({ data: response('day') })))
    const store = useStatsStore()
    store.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(store.servers.map(server => server.name)).toEqual(['Block'])
    expect(store.servers[0]?.total).toBe(50)
    usenetEvent?.()
    await vi.advanceTimersByTimeAsync(2_500)
    expect(calls('/api/v1/stats/usenet-servers')).toBe(2)
    store.stop()
  })
})

import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useStatsStore } from './stats'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The statistics are unavailable')
}))

let stateEvent: (() => void) | null = null
const released = vi.fn()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, () => void>) => {
    stateEvent = handlers['download.state'] ?? null
    return () => { stateEvent = null; released() }
  }
}))

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
    vi.useRealTimers()
  })

  it('asks for the chosen range and keeps the answer', async () => {
    vi.mocked(api.GET).mockImplementation(async (_path: string, options?: unknown) => {
      const range = (options as { params: { query: { range: string } } }).params.query.range
      return { data: response(range) } as never
    })
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
    vi.mocked(api.GET).mockImplementation(async (_path: string, options?: unknown) => {
      const range = (options as { params: { query: { range: string } } }).params.query.range
      if (range === 'day') return new Promise(resolve => { pending.resolve = resolve }) as never
      return { data: response(range) } as never
    })
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
    vi.mocked(api.GET).mockResolvedValue({ data: response('day') } as never)
    const store = useStatsStore()
    store.start()
    expect(api.GET).toHaveBeenCalledTimes(1)
    stateEvent?.()
    stateEvent?.()
    stateEvent?.()
    await vi.advanceTimersByTimeAsync(2_500)
    expect(api.GET).toHaveBeenCalledTimes(2)
    store.stop()
    expect(released).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(120_000)
    expect(api.GET).toHaveBeenCalledTimes(2)
  })
})

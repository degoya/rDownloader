/**
 * The timed queue pause as the interface mirrors it (RD-190-20): the end it asks for, the
 * countdown it shows, and ending it early.
 */
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { nextOccurrence, useQueuePauseStore } from './queuePause'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The pause needs an end ahead, at most 30 days away')
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: vi.fn(() => () => {}) }))

const NOW = new Date('2026-10-02T12:00:00Z')

describe('the next occurrence of a clock time', () => {
  it('is today while the time is still ahead', () => {
    const now = new Date(2026, 9, 2, 12, 0)
    expect(nextOccurrence('18:30', now)).toEqual(new Date(2026, 9, 2, 18, 30))
  })

  it('is tomorrow once the time has passed, or is now', () => {
    const now = new Date(2026, 9, 2, 12, 0)
    expect(nextOccurrence('08:15', now)).toEqual(new Date(2026, 9, 3, 8, 15))
    expect(nextOccurrence('12:00', now)).toEqual(new Date(2026, 9, 3, 12, 0))
  })

  it('refuses what is not a clock time', () => {
    expect(nextOccurrence('')).toBeNull()
    expect(nextOccurrence('25:00')).toBeNull()
    expect(nextOccurrence('12:60')).toBeNull()
    expect(nextOccurrence('noon')).toBeNull()
  })
})

describe('the queue pause store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    vi.setSystemTime(NOW)
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.DELETE).mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('pauses for a number of minutes and counts down to the end', async () => {
    const until = new Date(NOW.getTime() + 30 * 60_000).toISOString()
    vi.mocked(api.PUT).mockResolvedValue({ data: { paused: true, until, files: 4 } } as never)
    const store = useQueuePauseStore()

    expect(await store.pauseFor(30)).toBe(true)

    expect(vi.mocked(api.PUT)).toHaveBeenCalledWith('/api/v1/queue/pause', { body: { minutes: 30 } })
    expect(store.active).toBe(true)
    expect(store.until).toBe(until)
    expect(store.files).toBe(4)
    expect(store.remainingSeconds).toBe(30 * 60)
  })

  it('asks for a chosen time as an instant', async () => {
    const at = new Date(NOW.getTime() + 3 * 3_600_000)
    vi.mocked(api.PUT).mockResolvedValue({ data: { paused: true, until: at.toISOString(), files: 0 } } as never)
    const store = useQueuePauseStore()

    await store.pauseUntil(at)

    expect(vi.mocked(api.PUT)).toHaveBeenCalledWith('/api/v1/queue/pause', { body: { until: at.toISOString() } })
    expect(store.active).toBe(true)
  })

  it('keeps the refusal and stays unpaused', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: undefined, error: { code: 'queue.pause_end_invalid' } } as never)
    const store = useQueuePauseStore()

    expect(await store.pauseFor(30)).toBe(false)

    expect(store.active).toBe(false)
    expect(store.error).toBe('The pause needs an end ahead, at most 30 days away')
  })

  it('ends the pause early and reports how many files it queued again', async () => {
    const until = new Date(NOW.getTime() + 60 * 60_000).toISOString()
    vi.mocked(api.PUT).mockResolvedValue({ data: { paused: true, until, files: 2 } } as never)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { resumed: 2 } } as never)
    const store = useQueuePauseStore()
    await store.pauseFor(60)

    expect(await store.resume()).toBe(2)

    expect(vi.mocked(api.DELETE)).toHaveBeenCalledWith('/api/v1/queue/pause')
    expect(store.active).toBe(false)
    expect(store.until).toBeNull()
  })

  // RD-1190-14: the accounts whose traffic is used up come with the pause, and a start by hand
  // lets go of them too.
  it('names the accounts held for their traffic and forgets them on a start by hand', async () => {
    const hold = {
      account_id: 'account-1', account_label: 'DDownload premium', provider: 'ddownload', action: 'pause_account',
      until: new Date(NOW.getTime() + 3_600_000).toISOString(), next_check_at: new Date(NOW.getTime() + 900_000).toISOString()
    }
    vi.mocked(api.GET).mockResolvedValue({ data: { paused: false, until: null, files: 0, account_traffic: [hold] } } as never)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { resumed: 0 } } as never)
    const store = useQueuePauseStore()

    await store.load()
    expect(store.accountTraffic).toEqual([hold])
    expect(store.active).toBe(false)

    expect(await store.resume()).toBe(0)
    expect(store.accountTraffic).toEqual([])
  })

  it('reads the pause back once its end has passed', async () => {
    const until = new Date(NOW.getTime() + 2_000).toISOString()
    vi.mocked(api.GET)
      .mockResolvedValueOnce({ data: { paused: true, until, files: 1 } } as never)
      .mockResolvedValue({ data: { paused: false, until: null, files: 0 } } as never)
    const store = useQueuePauseStore()
    store.connect()
    await vi.advanceTimersByTimeAsync(0)
    expect(store.active).toBe(true)

    await vi.advanceTimersByTimeAsync(3_000)

    expect(store.active).toBe(false)
    expect(vi.mocked(api.GET).mock.calls.length).toBe(2)
    expect(store.until).toBeNull()
    store.disconnect()
  })
})

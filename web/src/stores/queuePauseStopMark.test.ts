/**
 * The stop mark as the queue pause store mirrors it (RD-1210-02): read with the pause, set and
 * removed through its own route, and the pause it leaves behind, which has no end.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useQueuePauseStore } from './queuePause'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'That download or package is already finished')
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: vi.fn(() => () => {}) }))

const MARK = { download_id: 'd1', package_id: null, name: 'release.rar', set_at: '2026-10-08T10:00:00Z' }

describe('the stop mark in the queue pause store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.DELETE).mockReset()
  })

  it('is read with the pause and names its row', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: { paused: false, files: 0, account_traffic: [], stop_mark: MARK } } as never)
    const store = useQueuePauseStore()

    await store.load()

    expect(store.stopMark).toEqual(MARK)
    expect(store.marks('download', 'd1')).toBe(true)
    expect(store.marks('download', 'd2')).toBe(false)
    expect(store.marks('package', 'd1')).toBe(false)
    expect(store.active).toBe(false)
  })

  it('holds the pause it leaves behind until it is resumed', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: { paused: true, until: null, files: 2, account_traffic: [] } } as never)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { resumed: 2 } } as never)
    const store = useQueuePauseStore()

    await store.load()

    expect(store.active).toBe(true)
    expect(store.openEnded).toBe(true)
    expect(store.remainingSeconds).toBe(0)
    expect(store.stopMark).toBeNull()
    expect(await store.resume()).toBe(2)
    expect(store.active).toBe(false)
    expect(store.openEnded).toBe(false)
  })

  it('sets the mark on a package and removes it again', async () => {
    const onPackage = { ...MARK, download_id: null, package_id: 'p1', name: 'Release' }
    vi.mocked(api.PUT).mockResolvedValue({ data: onPackage } as never)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { cleared: true } } as never)
    const store = useQueuePauseStore()

    expect(await store.setStopMark({ package_id: 'p1' })).toBe(true)
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/queue/stop-mark', { body: { package_id: 'p1' } })
    expect(store.marks('package', 'p1')).toBe(true)

    expect(await store.clearStopMark()).toBe(true)
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/queue/stop-mark')
    expect(store.stopMark).toBeNull()
  })

  it('keeps the old mark and the reason when a mark is refused', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ error: { code: 'queue.stop_mark_target_finished' } } as never)
    const store = useQueuePauseStore()
    store.stopMark = MARK

    expect(await store.setStopMark({ download_id: 'd2' })).toBe(false)
    expect(store.error).toBe('That download or package is already finished')
    expect(store.stopMark).toEqual(MARK)
  })
})

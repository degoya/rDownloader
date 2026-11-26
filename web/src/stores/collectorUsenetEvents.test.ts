/**
 * RD-1480-18: a running Usenet transfer announces itself several times a second, and nothing the
 * LinkGrabber lists changes with it.
 */
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, eventStreamMock } from '@/test/mocks'

const get = vi.fn()
vi.mock('@/api/client', () => apiClientMock({ GET: (...args: unknown[]) => get(...args) }))
const handlers: Record<string, (event: MessageEvent) => void> = {}
vi.mock('@/composables/useEventStream', () => eventStreamMock({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    Object.assign(handlers, registered)
    return () => {}
  }
}))
vi.mock('@/composables/useNotifications', () => ({ useNotifications: () => ({ notify: vi.fn() }) }))

const { useCollectorStore } = await import('./collector')

async function reads(data: unknown, name = 'usenet.changed'): Promise<number> {
  get.mockClear()
  handlers['usenet.changed']?.(new MessageEvent(name, { data: JSON.stringify(data) }))
  await vi.advanceTimersByTimeAsync(1_000)
  return get.mock.calls.filter(call => call[0] === '/api/v1/collector/packages').length
}

describe('collector store: Usenet events', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    get.mockResolvedValue({ data: [] })
    useCollectorStore().connectEvents()
  })
  afterEach(() => {
    useCollectorStore().disconnectEvents()
    vi.useRealTimers()
  })

  it('reads nothing for a running transfer', async () => {
    expect(await reads({ payload: { resource: 'nzb_checkpoint', nzb_file_id: 'f-1' } })).toBe(0)
    expect(await reads({ payload: { nzb_segment_id: 's-1', state: 'downloading' } })).toBe(0)
  })

  it.each([
    ['an import', { payload: { nzb_import_id: 'i-1', state: 'enqueued' } }, undefined],
    ['a removed import', { payload: { nzb_import_id: 'i-1', removed: true } }, undefined],
    ['a hole in the stream', { dropped: 3 }, 'stream.lagged']
  ])('reads again for %s', async (_name, data, name) => {
    expect(await reads(data, name)).toBe(1)
  })
})

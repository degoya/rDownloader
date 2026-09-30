/**
 * A fetch that rejects must not freeze a store.
 *
 * The client turns a network failure into a refusal (`api/client.ts`), but a store's refresh
 * guards its own in-flight flag as well: a rejection that escaped before the flag came down left
 * `scheduleRefresh` re-arming behind it every 300-400 ms without ever asking again, and the queue
 * or the LinkGrabber stopped following events until a reload.
 */
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
/** The handlers each store registered, so a test can play the server's events. */
const handlers: Record<string, (event: MessageEvent) => void> = {}
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (registered: Record<string, (event: MessageEvent) => void>) => {
    Object.assign(handlers, registered)
    return () => {}
  }
}))
vi.mock('@/composables/useNotifications', () => ({ useNotifications: () => ({ notify: vi.fn() }) }))

const { useAutomationsStore } = await import('./automations')
const { useCollectorStore } = await import('./collector')
const { useSubscriptionsStore } = await import('./subscriptions')
const { useTorrentsStore } = await import('./torrents')
const { useTransfersStore } = await import('./transfers')

function event(name: string): void {
  handlers[name]?.(new MessageEvent(name, { data: '{}' }))
}

describe('a store after a rejected fetch', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    get.mockReset()
    get.mockResolvedValue({ data: [] })
    for (const name of Object.keys(handlers)) delete handlers[name]
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('lets the queue follow the next event', async () => {
    const store = useTransfersStore()
    store.connectEvents()
    get.mockRejectedValueOnce(new TypeError('Failed to fetch'))

    await store.refresh()

    expect(store.pending).toBe(false)
    expect(store.error).toBe('The service did not answer')
    get.mockClear()
    event('download.state')
    await vi.advanceTimersByTimeAsync(400)
    expect(get).toHaveBeenCalledWith('/api/v1/downloads')
    store.disconnectEvents()
  })

  it('lets the LinkGrabber follow the next event', async () => {
    const store = useCollectorStore()
    store.connectEvents()
    get.mockRejectedValueOnce(new TypeError('Failed to fetch'))

    await store.refresh()

    expect(store.error).toBe('The service did not answer')
    get.mockClear()
    event('collector.changed')
    await vi.advanceTimersByTimeAsync(300)
    expect(get).toHaveBeenCalledWith('/api/v1/collector/packages')
  })

  it('lets subscriptions follow the next event', async () => {
    const store = useSubscriptionsStore()
    store.connectEvents()
    get.mockRejectedValueOnce(new TypeError('Failed to fetch'))

    await store.refresh()

    expect(store.loading).toBe(false)
    get.mockClear()
    event('subscription.changed')
    await vi.advanceTimersByTimeAsync(300)
    expect(get).toHaveBeenCalledWith('/api/v1/subscriptions')
  })

  it('lets automations follow the next event', async () => {
    const store = useAutomationsStore()
    store.connectEvents()
    get.mockRejectedValueOnce(new TypeError('Failed to fetch'))

    await store.refresh()

    expect(store.loading).toBe(false)
    get.mockClear()
    event('automation.changed')
    await vi.advanceTimersByTimeAsync(300)
    expect(get).toHaveBeenCalledWith('/api/v1/automations')
  })

  it('lets a torrent detail load again', async () => {
    const store = useTorrentsStore()
    get.mockRejectedValueOnce(new TypeError('Failed to fetch'))

    await store.load('download', 'd1')

    expect(store.isBusy('download', 'd1')).toBe(false)
    get.mockClear()
    await store.load('download', 'd1')
    expect(get).toHaveBeenCalledOnce()
  })
})

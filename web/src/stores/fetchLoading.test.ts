/**
 * The stores tell a view whether their list is still on its way (RD-104-07).
 *
 * `transfers.pending`, `collector.pending` and `subscriptions.busy` all existed before this,
 * but none of them answered the question a view actually asks on mount: "is this list empty,
 * or have I simply not been told yet?" `pending` is `false` before the first `refresh()` is
 * called, which is exactly when the empty state would render. Each store now exposes one
 * derived flag that stays true from the first frame until the first fetch has settled.
 */
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

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
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useNotifications', () => ({ useNotifications: () => ({ notify: vi.fn() }) }))

const { useAutomationsStore } = await import('./automations')
const { useCollectorStore } = await import('./collector')
const { useSubscriptionsStore } = await import('./subscriptions')
const { useTransfersStore } = await import('./transfers')

describe('fetch state on the stores', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    get.mockReset()
    get.mockResolvedValue({ data: [] })
  })

  it('reports the transfer queue as loading before the first refresh has run', async () => {
    const store = useTransfersStore()

    expect(store.loading).toBe(true)

    await store.refresh()

    expect(store.loading).toBe(false)
  })

  it('reports the collector as loading before the first refresh has run', async () => {
    const store = useCollectorStore()

    expect(store.loading).toBe(true)

    await store.refresh()

    expect(store.loading).toBe(false)
  })

  it('reports subscriptions as loading before the first refresh has run', async () => {
    const store = useSubscriptionsStore()

    expect(store.loading).toBe(true)

    await store.refresh()

    expect(store.loading).toBe(false)
  })

  it('leaves a failed subscription fetch as an error, not as an empty list', async () => {
    const store = useSubscriptionsStore()
    get.mockResolvedValue({ data: undefined, error: { code: 'internal' } })

    await store.refresh()

    expect(store.loading).toBe(false)
    expect(store.subscriptions).toEqual([])
    expect(store.error).toBe('The service did not answer')
  })
})

/**
 * The loading surface belongs to the first fetch, not to every refresh (RD-106-19).
 *
 * `loading` used to be `pending || !settled` — the per-refresh flag ORed back in. Refreshes
 * arrive from state events, poll timers, the speed sampler and after every write, so an empty
 * list swapped its empty state for the loading skeleton and back on each of them: a flicker,
 * visible precisely because `DataState` renders nothing at all once there is content.
 */
describe('a refresh on a settled store does not re-enter the loading state', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    get.mockReset()
    get.mockResolvedValue({ data: [] })
  })

  /** A response that stays in flight until the test lets it land. */
  function pendingResponse(): { resolve: () => void } {
    let release = (): void => {}
    const promise = new Promise((resolveResponse) => {
      release = () => resolveResponse({ data: [] })
    })
    get.mockReturnValue(promise)
    return { resolve: () => release() }
  }

  it('keeps the transfer queue out of the loading state on a second refresh', async () => {
    const store = useTransfersStore()
    await store.refresh()

    const response = pendingResponse()
    const second = store.refresh()

    expect(store.pending).toBe(true)
    expect(store.loading).toBe(false)

    response.resolve()
    await second

    expect(store.loading).toBe(false)
  })

  it('keeps the collector out of the loading state on a second refresh', async () => {
    const store = useCollectorStore()
    await store.refresh()

    const response = pendingResponse()
    const second = store.refresh()

    expect(store.loading).toBe(false)

    response.resolve()
    await second

    expect(store.loading).toBe(false)
  })

  it('keeps subscriptions out of the loading state on a second refresh', async () => {
    const store = useSubscriptionsStore()
    await store.refresh()

    const response = pendingResponse()
    const second = store.refresh()

    expect(store.loading).toBe(false)

    response.resolve()
    await second

    expect(store.loading).toBe(false)
  })

  it('keeps automations out of the loading state on a second refresh', async () => {
    const store = useAutomationsStore()
    await store.refresh()

    const response = pendingResponse()
    const second = store.refresh()

    expect(store.loading).toBe(false)

    response.resolve()
    await second

    expect(store.loading).toBe(false)
  })

  it('still reports the first fetch as loading while it is in flight', async () => {
    const store = useAutomationsStore()
    const response = pendingResponse()
    const first = store.refresh()

    expect(store.loading).toBe(true)

    response.resolve()
    await first

    expect(store.loading).toBe(false)
  })

  /**
   * A first fetch that failed is settled, so the view leaves the loading surface and shows the
   * error — `DataState` prefers it over the empty state — and a retry keeps that error on
   * screen rather than flashing the skeleton a second time.
   */
  it('leaves the loading state after a failed first fetch and stays out of it on the retry', async () => {
    const store = useAutomationsStore()
    get.mockResolvedValue({ data: undefined, error: { code: 'internal' } })

    await store.refresh()

    expect(store.loading).toBe(false)
    expect(store.error).toBe('The service did not answer')

    const response = pendingResponse()
    const retry = store.refresh()

    expect(store.loading).toBe(false)

    response.resolve()
    await retry

    expect(store.loading).toBe(false)
  })
})

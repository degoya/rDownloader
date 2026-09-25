import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useAutomationsStore } from './automations'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The automation catalogue is unavailable')
}))

/** The shared event stream, reduced to the one handler this store registers. */
let automationEvent: ((event: MessageEvent<string>) => void) | null = null
const released = vi.fn()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    automationEvent = handlers['automation.changed'] ?? null
    return () => { automationEvent = null; released() }
  }
}))

const VOCABULARY = {
  triggers: ['package_completed'],
  action_kinds: ['move'],
  fields: ['size_bytes'],
  operators: ['gt'],
  max_actions: 5,
  max_condition_depth: 3
}

describe('automations store: vocabulary', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
  })

  it('reports a failure instead of leaving the dropdowns silently empty', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'internal' } } as never)
    const store = useAutomationsStore()

    await store.loadVocabulary()

    expect(store.vocabulary).toBeNull()
    // The view only renders its alert from `error`; without this the user saw four empty
    // dropdowns and no explanation at all.
    expect(store.error).toBe('The automation catalogue is unavailable')
  })

  it('retries after a failure rather than staying empty for the session', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce({ error: { code: 'internal' } } as never)
    const store = useAutomationsStore()
    await store.loadVocabulary()

    vi.mocked(api.GET).mockResolvedValueOnce({ data: VOCABULARY } as never)
    await store.loadVocabulary()

    expect(store.vocabulary).toEqual(VOCABULARY)
    expect(store.error).toBeNull()
  })

  it('does not refetch once loaded', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: VOCABULARY } as never)
    const store = useAutomationsStore()

    await store.loadVocabulary()
    await store.loadVocabulary()

    expect(api.GET).toHaveBeenCalledTimes(1)
  })
})

/**
 * The store subscribed to nothing at all until the server published an automation event, so
 * an automation created, enabled, disabled or deleted anywhere else stayed invisible here
 * until somebody reloaded the page.
 */
describe('automations store: server events', () => {
  const AUTOMATION = { id: 'a-1', name: 'Move finished series', enabled: true, trigger: 'download_completed' }

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    automationEvent = null
    released.mockReset()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  function serve(automations: unknown[]): void {
    vi.mocked(api.GET).mockImplementation((async (path: string) =>
      path === '/api/v1/automations' ? { data: automations } : { data: [] }) as never)
  }

  it('re-reads the list when an automation changes elsewhere', async () => {
    serve([])
    const store = useAutomationsStore()
    store.connectEvents()
    await store.refresh()
    expect(store.automations).toEqual([])

    serve([AUTOMATION])
    automationEvent?.({ data: JSON.stringify({ payload: { automation_id: 'a-1' } }) } as MessageEvent<string>)
    await vi.advanceTimersByTimeAsync(400)

    expect(store.automations).toEqual([AUTOMATION])
  })

  it('coalesces a burst of events into one refresh', async () => {
    serve([AUTOMATION])
    const store = useAutomationsStore()
    store.connectEvents()
    await store.refresh()
    const before = vi.mocked(api.GET).mock.calls.filter(call => call[0] === '/api/v1/automations').length

    const event = { data: JSON.stringify({ payload: {} }) } as MessageEvent<string>
    for (let index = 0; index < 5; index += 1) automationEvent?.(event)
    await vi.advanceTimersByTimeAsync(400)

    // An import writes one event per automation; five of them must not cost five round trips.
    expect(vi.mocked(api.GET).mock.calls.filter(call => call[0] === '/api/v1/automations').length).toBe(before + 1)
  })

  it('releases the subscription and drops a pending refresh when the view goes away', async () => {
    serve([])
    const store = useAutomationsStore()
    store.connectEvents()
    await store.refresh()
    const before = vi.mocked(api.GET).mock.calls.length

    automationEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
    store.disconnectEvents()
    await vi.advanceTimersByTimeAsync(400)

    expect(released).toHaveBeenCalledTimes(1)
    // The timer armed a moment ago must not fire into a store nobody is showing.
    expect(vi.mocked(api.GET).mock.calls.length).toBe(before)
  })
})

/**
 * The lists several views read, held once (WEB-3): one request for the components that open
 * together, and a copy the event stream keeps current while a component shows it.
 */
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'

import { api } from '@/api/client'
import { subscribeEvents } from '@/composables/useEventStream'

import { useAccounts } from './accounts'
import { useCategories } from './categories'
import { useProxyProfiles } from './proxyProfiles'
import { useSettingsStore } from './settings'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn() } }))

/** The handlers of every live subscription, by event name. */
const handlers = new Map<string, Set<(event: MessageEvent) => void>>()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: vi.fn((named: Record<string, (event: MessageEvent) => void>) => {
    for (const [name, handler] of Object.entries(named)) {
      if (!handlers.has(name)) handlers.set(name, new Set())
      handlers.get(name)?.add(handler)
    }
    return () => {
      for (const [name, handler] of Object.entries(named)) handlers.get(name)?.delete(handler)
    }
  })
}))

function emit(name: string): void {
  for (const handler of handlers.get(name) ?? []) handler(new MessageEvent(name, { data: '{}' }))
}

const MOVIES = { id: 'movies', name: 'Movies' }
const SERIES = { id: 'series', name: 'Series' }

/** `api.GET` answering each path from `answers`, the latest value at the time of the call. */
function answer(answers: Record<string, unknown>): void {
  vi.mocked(api.GET).mockImplementation((async (path: string) => ({ data: answers[path] })) as never)
}

describe('the shared lists', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    handlers.clear()
    vi.mocked(api.GET).mockReset()
    vi.mocked(subscribeEvents).mockClear()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('shares one request between the components that open together, and answers its response', async () => {
    answer({ '/api/v1/categories': [MOVIES] })
    const scope = effectScope()
    const [first, second] = scope.run(() => [useCategories(), useCategories()]) ?? []

    const [one, two] = await Promise.all([first?.fetchCategories(), second?.fetchCategories()])

    expect(api.GET).toHaveBeenCalledTimes(1)
    expect(one?.data).toEqual([MOVIES])
    expect(two).toBe(one)
    expect(first?.categories.value).toEqual([MOVIES])
    expect(second?.categories.value).toBe(first?.categories.value)
    scope.stop()
  })

  it('asks the server again on the next load, so a view that opens sees the stored state', async () => {
    const answers: Record<string, unknown> = { '/api/v1/categories': [MOVIES] }
    answer(answers)
    const scope = effectScope()
    const categories = scope.run(() => useCategories())
    await categories?.fetchCategories()

    answers['/api/v1/categories'] = [MOVIES, SERIES]
    await categories?.fetchCategories()

    expect(api.GET).toHaveBeenCalledTimes(2)
    expect(categories?.categories.value).toEqual([MOVIES, SERIES])
    scope.stop()
  })

  it('keeps the list it has when a read fails', async () => {
    answer({ '/api/v1/categories': [MOVIES] })
    const scope = effectScope()
    const categories = scope.run(() => useCategories())
    await categories?.fetchCategories()

    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'network.unreachable' } } as never)
    const failed = await categories?.fetchCategories()

    expect(failed?.data).toBeUndefined()
    expect(categories?.categories.value).toEqual([MOVIES])
    scope.stop()
  })

  it('re-reads a list on its event while a component shows it, once per burst', async () => {
    const answers: Record<string, unknown> = {
      '/api/v1/categories': [MOVIES],
      '/api/v1/accounts': [],
      '/api/v1/proxy-profiles': []
    }
    answer(answers)
    const scope = effectScope()
    const lists = scope.run(() => ({ ...useCategories(), ...useAccounts(), ...useProxyProfiles() }))
    await lists?.fetchCategories()
    expect([...handlers.keys()]).toEqual(['category.changed', 'account.changed', 'proxy.changed'])

    // A category created in the settings while the LinkGrabber is open.
    answers['/api/v1/categories'] = [MOVIES, SERIES]
    emit('category.changed')
    emit('category.changed')
    await vi.runAllTimersAsync()

    expect(vi.mocked(api.GET).mock.calls.filter(call => (call as unknown[])[0] === '/api/v1/categories')).toHaveLength(2)
    expect(lists?.categories.value).toEqual([MOVIES, SERIES])
    scope.stop()
  })

  it('drops the subscription with the last component that follows the list', () => {
    answer({ '/api/v1/categories': [] })
    const first = effectScope()
    const second = effectScope()
    first.run(() => useCategories())
    second.run(() => useCategories())
    expect(handlers.get('category.changed')?.size).toBe(1)

    first.stop()
    expect(handlers.get('category.changed')?.size).toBe(1)
    second.stop()
    expect(handlers.get('category.changed')?.size).toBe(0)
  })

  it('reads the settings document without a subscription, since no event says it changed', async () => {
    answer({ '/api/v1/settings': { byte_unit: 'si' } })
    const store = useSettingsStore()

    await Promise.all([store.fetchSettings(), store.fetchSettings()])

    expect(api.GET).toHaveBeenCalledTimes(1)
    expect(store.settings).toEqual({ byte_unit: 'si' })
    expect(subscribeEvents).not.toHaveBeenCalled()
  })
})

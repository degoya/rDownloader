/**
 * The provider registry is filled solely from installed plugin manifests, and this composable
 * read it once for the lifetime of the page. So a resolver plugin removed elsewhere left every
 * candidate row still flagged "no account for this hoster" for a hoster that no longer exists,
 * and one freshly installed left its links unflagged — until the page was reloaded.
 */
import { waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

type Provider = { slug: string, credentials: string, kind: string }

const get = vi.fn()
vi.mock('@/api/client', () => ({ api: { GET: (...args: unknown[]) => get(...args) } }))

/** The shared event stream, reduced to the one handler this composable registers. */
let pluginEvent: ((event: MessageEvent<string>) => void) | null = null
/**
 * Every channel the screen subscribes to. The name matters as much as the reaction:
 * `Granted::may_observe` hands a subscriber an event only when it holds that event's
 * exact scope, so a screen listening on a channel named for another scope is silently
 * never served.
 */
let subscribedNames: string[] = []
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    subscribedNames = Object.keys(handlers)
    pluginEvent = handlers['plugin_catalog.changed'] ?? null
    return () => { pluginEvent = null }
  }
}))

/** The registry as the service currently answers it; the test moves this set around. */
let providers: Provider[] = []

/**
 * A fresh copy of the module.
 *
 * Its state is module-level and loaded exactly once, so every case needs its own instance —
 * otherwise the second one would inspect the first one's already-loaded sets.
 */
async function loadComposable() {
  vi.resetModules()
  const { useAccountProviders } = await import('./useAccountProviders')
  return useAccountProviders()
}

describe('useAccountProviders reacting to plugin_catalog.changed', () => {
  beforeEach(() => {
    get.mockReset()
    pluginEvent = null
    subscribedNames = []
    providers = [{ slug: 'rapidgator', credentials: 'api_key', kind: 'hoster' }]
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/providers' ? { data: providers } : { data: [] }
    )
  })

  /**
   * The channel, not just the reaction. `/api/v1/providers` costs `Config`, and a subscriber is
   * handed an event only when it holds that event's exact scope — scopes widen towards `Read`
   * only, so `plugin.changed` would be a subscription the service can never serve. Naming the
   * set exactly also keeps the screen from listening to all three and hiding the next such
   * mistake.
   */
  it('subscribes at the scope its own data is read at', async () => {
    await loadComposable()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/providers'))
    expect(subscribedNames).toEqual(['plugin_catalog.changed', 'account.changed'])
  })

  it('stops flagging a hoster whose plugin was removed elsewhere', async () => {
    const { lacksAccount } = await loadComposable()

    await waitFor(() => expect(lacksAccount('rapidgator')).toBe(true))

    // The plugin was removed in the plugin manager; this composable is only told that
    // something about the installed plugins changed.
    providers = []
    pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(lacksAccount('rapidgator')).toBe(false), { timeout: 2000 })
  })

  it('flags a hoster whose plugin was installed elsewhere', async () => {
    providers = []
    const { lacksAccount } = await loadComposable()

    await waitFor(() => expect(get).toHaveBeenCalled())
    expect(lacksAccount('rapidgator')).toBe(false)

    providers = [{ slug: 'rapidgator', credentials: 'api_key', kind: 'hoster' }]
    pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(() => expect(lacksAccount('rapidgator')).toBe(true), { timeout: 2000 })
  })

  it('coalesces a burst of events into one pair of reads', async () => {
    await loadComposable()

    await waitFor(() => expect(get.mock.calls.length).toBe(2))
    const before = get.mock.calls.length
    const event = { data: JSON.stringify({ payload: {} }) } as MessageEvent<string>
    for (let index = 0; index < 5; index += 1) pluginEvent?.(event)

    // Installing one package emits more than one event; that must not cost ten requests.
    await waitFor(() => expect(get.mock.calls.length).toBe(before + 2), { timeout: 2000 })
    expect(get.mock.calls.length).toBe(before + 2)
  })
})

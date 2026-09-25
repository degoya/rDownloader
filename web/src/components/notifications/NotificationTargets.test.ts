/**
 * The destinations come solely from installed notifier plugins, and they decide whether the
 * `plugin` kind is offered at all. Read once on mount, the form kept the kind hidden after a
 * notifier was installed elsewhere — the feature simply appeared not to exist — and kept
 * offering it after the last one was removed, which produces a target that fails on its first
 * delivery.
 */
import { screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import notifications from '@/locales/en/notifications.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

/** The shared event stream, reduced to the one handler this list registers. */
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

const { default: NotificationTargets } = await import('./NotificationTargets.vue')

const DESTINATION = { plugin_id: 'rd-plugin-ntfy', name: 'ntfy', version: '0.4.0' }

function mount() {
  return mountComponent(NotificationTargets, {
    messages: { notifications },
    props: { modelValue: [], loading: false, loadError: null }
  })
}

function fire(): void {
  pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)
}

describe('NotificationTargets reacting to plugin_catalog.changed', () => {
  beforeEach(() => {
    get.mockReset()
    pluginEvent = null
    subscribedNames = []
  })

  /**
   * The channel, not just the reaction. `/api/v1/notifications/destinations` costs `Config`, and a subscriber is
   * handed an event only when it holds that event's exact scope — scopes widen towards `Read`
   * only, so `plugin.changed` would be a subscription the service can never serve. Naming the
   * set exactly also keeps the screen from listening to all three and hiding the next such
   * mistake.
   */
  it('subscribes at the scope its own data is read at', async () => {
    get.mockResolvedValue({ data: [] })

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/notifications/destinations'))
    expect(subscribedNames).toEqual(['plugin_catalog.changed'])
  })

  it('offers the plugin kind once a notifier is installed elsewhere', async () => {
    get.mockResolvedValue({ data: [] })

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/notifications/destinations'))
    expect(screen.queryByText(notifications.kind.plugin)).toBeNull()

    get.mockResolvedValue({ data: [DESTINATION] })
    fire()

    await waitFor(() => expect(screen.getByText(notifications.kind.plugin)).toBeTruthy(), { timeout: 2000 })
  })

  it('withdraws the plugin kind when the last notifier is removed elsewhere', async () => {
    get.mockResolvedValue({ data: [DESTINATION] })

    mount()

    await waitFor(() => expect(screen.getByText(notifications.kind.plugin)).toBeTruthy())

    get.mockResolvedValue({ data: [] })
    fire()

    await waitFor(() => expect(screen.queryByText(notifications.kind.plugin)).toBeNull(), { timeout: 2000 })
  })

  it('coalesces a burst of events into one read', async () => {
    get.mockResolvedValue({ data: [DESTINATION] })

    mount()

    await waitFor(() => expect(screen.getByText(notifications.kind.plugin)).toBeTruthy())
    get.mockClear()
    for (let index = 0; index < 5; index += 1) fire()

    await waitFor(() => expect(get.mock.calls.length).toBe(1), { timeout: 2000 })
    expect(get.mock.calls.length).toBe(1)
  })
})

/**
 * The destinations come solely from installed notifier plugins, and they decide whether the
 * `plugin` kind is offered at all. Read once on mount, the form kept the kind hidden after a
 * notifier was installed elsewhere — the feature simply appeared not to exist — and kept
 * offering it after the last one was removed, which produces a target that fails on its first
 * delivery.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import notifications from '@/locales/en/notifications.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: (...args: unknown[]) => post(...args), PUT: vi.fn(), DELETE: vi.fn() },
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

/** RD-150-11 and RD-150-12: the kind leads the form, and a copy never carries the secret. */
describe('NotificationTargets form and duplicate', () => {
  const SMTP = {
    id: 't1', name: 'Mail', kind: 'smtp', enabled: true, endpoint: 'smtp.example:587',
    config: { from: 'rd@example', to: ['me@example'], tls: 'starttls' }, has_secret: true
  }

  beforeEach(() => {
    get.mockReset()
    get.mockResolvedValue({ data: [] })
    post.mockReset()
  })

  it('asks for the kind before the name, because the kind decides every field after it', () => {
    mountComponent(NotificationTargets, { messages: { notifications }, props: { modelValue: [], loading: false, loadError: null } })
    const labels = Array.from(document.querySelectorAll('form label')).map(label => label.textContent?.trim() ?? '')
    expect(labels[0]).toContain(notifications.target.kind_label)
    expect(labels[1]).toContain(notifications.target.name_label)
  })

  it('copies a target without its secret, opens the copy and says the secret is to be entered again', async () => {
    const copy = { ...SMTP, id: 't2', name: `Mail (${common.copy_suffix})`, has_secret: false }
    post.mockResolvedValueOnce({ data: copy })
    mountComponent(NotificationTargets, {
      messages: { notifications },
      props: { modelValue: [SMTP], loading: false, loadError: null },
      // The shared field stub drops the description, which is where the form says it.
      stubs: { UFormField: { props: ['label', 'description'], template: '<div><label v-if="label">{{ label }}<slot /></label><slot v-else /><p>{{ description }}</p></div>' } }
    })

    const row = screen.getByText('Mail').closest('div.flex') as HTMLElement
    await fireEvent.click(within(row).getByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/notifications/targets', expect.anything()))
    expect(post.mock.calls[0]?.[1]?.body).toEqual({
      name: `Mail (${common.copy_suffix})`,
      kind: 'smtp',
      enabled: true,
      endpoint: 'smtp.example:587',
      config: SMTP.config,
      secret: null,
      clear_secret: false
    })
    await screen.findByRole('heading', { name: notifications.target.form_edit })
    expect(screen.getByText(notifications.target.secret_copy)).toBeTruthy()
    expect(screen.getByText(common.editing)).toBeTruthy()
  })
})

/**
 * The reported case (RD-104-07).
 *
 * "When account data is being loaded it takes a while, but you see no loading indicator." It
 * was worse than that: opening the tab fires three requests in parallel, `accounts` is empty
 * for their whole duration, and the empty state rendered off that emptiness — so the tab
 * stated "No provider account created yet" to somebody who has accounts, long enough on a
 * slow connection to be believed. A failed request left the same sentence standing for good.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import network from '@/locales/en/network.json'
import { mountComponent } from '@/test/mount'

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
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

/** The shared event stream, reduced to the one handler this tab registers. */
let pluginEvent: ((event: MessageEvent<string>) => void) | null = null
const released = vi.fn()
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
    return () => { pluginEvent = null; released() }
  }
}))

const { default: SettingsAccountsTab } = await import('./SettingsAccountsTab.vue')

const ACCOUNT = {
  id: 'account-1',
  label: 'Rapidgator',
  provider: 'rapidgator',
  username: 'someone',
  enabled: true,
  has_secret: true,
  has_cookies: false,
  credential_mode: 'password',
  proxy_profile_id: null
}

/** A request that never settles, so the first render can be inspected mid-fetch. */
function neverResolves() {
  return new Promise<never>(() => {})
}

/**
 * The remote-jobs card is stubbed out: it fetches and subscribes on its own, so leaving it in
 * would put a second loading surface and a second event subscription on the screen, and the
 * assertions below are about *this* tab's three requests (RD-108-04).
 */
function mount() {
  return mountComponent(SettingsAccountsTab, {
    messages: { network },
    stubs: { SettingsRemoteJobsCard: true }
  })
}

describe('SettingsAccountsTab', () => {
  beforeEach(() => {
    get.mockReset()
  })

  it('does not claim there are no accounts while the three requests are in flight', () => {
    get.mockImplementation(neverResolves)

    mount()

    expect(screen.queryByText(network.account.empty)).toBeNull()
    expect(screen.getByRole('status').textContent).toContain('Loading')
  })

  it('shows the empty state once the fetch came back with nothing', async () => {
    get.mockResolvedValue({ data: [] })

    mount()

    await waitFor(() => expect(screen.getByText(network.account.empty)).toBeTruthy())
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('shows a failed fetch as a failure rather than as "no accounts"', async () => {
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/accounts' ? { data: undefined, error: { code: 'internal' } } : { data: [] }
    )

    mount()

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('The service did not answer'))
    expect(screen.queryByText(network.account.empty)).toBeNull()
  })

  it('shows neither state once accounts arrive', async () => {
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/accounts' ? { data: [ACCOUNT] } : { data: [] }
    )

    mount()

    await waitFor(() => expect(screen.getByText('Rapidgator')).toBeTruthy())
    expect(screen.queryByText(network.account.empty)).toBeNull()
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('offers device sign-in only while an API-key account has no key', async () => {
    const withoutKey = {
      ...ACCOUNT,
      id: 'premiumize-empty',
      label: 'Premiumize without key',
      provider: 'premiumize',
      has_secret: false
    }
    const withKey = {
      ...withoutKey,
      id: 'premiumize-ready',
      label: 'Premiumize with key',
      has_secret: true
    }
    const oauthWithClientSecret = {
      ...ACCOUNT,
      id: 'oauth-client',
      label: 'OAuth with client secret',
      provider: 'cloud',
      has_secret: true
    }
    const providers = [
      {
        slug: 'premiumize',
        display_name: 'Premiumize.me',
        kind: 'multihoster',
        credentials: 'api_key',
        credential_modes: [],
        username_required: false,
        device_flow: true
      },
      {
        slug: 'cloud',
        display_name: 'Cloud',
        kind: 'cloud',
        credentials: 'oauth',
        credential_modes: [],
        username_required: false,
        device_flow: true
      }
    ]
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/accounts') return { data: [withoutKey, withKey, oauthWithClientSecret] }
      if (path === '/api/v1/providers') return { data: providers }
      if (path.includes('/auth')) return { data: null }
      return { data: [] }
    })

    mount()

    const emptyRow = (await screen.findByText('Premiumize without key')).closest('.border') as HTMLElement
    const readyRow = screen.getByText('Premiumize with key').closest('.border') as HTMLElement
    const oauthRow = screen.getByText('OAuth with client secret').closest('.border') as HTMLElement
    expect(within(emptyRow).getByRole('button', { name: network.account.connect })).toBeTruthy()
    expect(within(readyRow).queryByRole('button', { name: network.account.connect })).toBeNull()
    expect(within(oauthRow).getByRole('button', { name: network.account.connect })).toBeTruthy()
  })
})

/**
 * The provider catalogue is filled solely from installed plugin manifests, and this tab read it
 * once on mount. So a plugin installed, removed, enabled or disabled elsewhere left the picker
 * and the sign-in offer describing the state from when the tab was opened.
 */
describe('SettingsAccountsTab reacting to plugin_catalog.changed', () => {
  const account = {
    ...ACCOUNT,
    id: 'premiumize-empty',
    label: 'Premiumize without key',
    provider: 'premiumize',
    has_secret: false
  }

  function provider(deviceFlow: boolean) {
    return {
      slug: 'premiumize',
      display_name: 'Premiumize.me',
      kind: 'multihoster',
      credentials: 'api_key',
      credential_modes: [],
      username_required: false,
      device_flow: deviceFlow
    }
  }

  /** Answers the tab's three fetches; `providers` is what the test moves around. */
  function serve(providers: unknown[]) {
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/accounts') return { data: [account] }
      if (path === '/api/v1/providers') return { data: providers }
      if (path.includes('/auth')) return { data: null }
      return { data: [] }
    })
  }

  beforeEach(() => {
    get.mockReset()
    released.mockReset()
    pluginEvent = null
    subscribedNames = []
  })

  /**
   * The channel, not just the reaction. `/api/v1/providers` costs `Config`, and a subscriber is
   * handed an event only when it holds that event's exact scope — scopes widen towards `Read`
   * only, so `plugin.changed` would be a subscription the service can never serve. Naming the
   * set exactly also keeps the screen from listening to all three and hiding the next such
   * mistake.
   */
  it('subscribes at the scope its own data is read at', async () => {
    serve([provider(true)])

    mount()

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/providers'))
    expect(subscribedNames).toEqual(['plugin_catalog.changed'])
  })

  it('withdraws the sign-in offer when the authentication plugin is removed elsewhere', async () => {
    serve([provider(true)])

    mount()

    const row = (await screen.findByText('Premiumize without key')).closest('.border') as HTMLElement
    expect(within(row).getByRole('button', { name: network.account.connect })).toBeTruthy()

    // The plugin that could sign this provider in was removed in the plugin manager. The
    // button stays only because the catalogue is stale — pressing it cannot work.
    serve([provider(false)])
    pluginEvent?.({ data: JSON.stringify({ payload: {} }) } as MessageEvent<string>)

    await waitFor(
      () => expect(within(row).queryByRole('button', { name: network.account.connect })).toBeNull(),
      { timeout: 2000 }
    )
  })

  it('re-reads only the catalogue, and only once for a burst', async () => {
    serve([provider(true)])

    mount()

    await waitFor(() => expect(screen.getByText('Premiumize without key')).toBeTruthy())
    get.mockClear()
    const event = { data: JSON.stringify({ payload: {} }) } as MessageEvent<string>
    for (let index = 0; index < 5; index += 1) pluginEvent?.(event)

    // One read, and it is the catalogue: accounts and proxy profiles are database rows that a
    // plugin event says nothing about, and each has an event of its own.
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/providers'), { timeout: 2000 })
    expect(get.mock.calls.filter(([path]) => path === '/api/v1/providers').length).toBe(1)
    expect(get.mock.calls.some(([path]) => path === '/api/v1/accounts')).toBe(false)
  })
})

/**
 * The sign-in method decides what the rest of the form asks for (RD-109-35).
 *
 * Reported from use with a screenshot: the form asked for a display name, a username and a
 * connection route before it asked how the account signs in, and the secret field beneath them
 * read "DDownload password or API key" — one box for two answers — because neither radio was
 * selected. `showUsernameInput`, `showCookiesInput` and the secret's own label all hang on that
 * choice, so it has to be made before the fields it governs are on screen.
 */
describe('SettingsAccountsTab sign-in method', () => {
  /** DDownload: the reported provider, and the one kind that offers a choice at all. */
  const withChoice = {
    slug: 'ddownload',
    display_name: 'DDownload',
    kind: 'hoster',
    credentials: 'login_or_api_key',
    credential_modes: ['login', 'api_key'],
    username_required: false,
    device_flow: false
  }
  /** The same slug with one way to hold an account, which must offer no choice. */
  const withoutChoice = {
    ...withChoice,
    credentials: 'api_key',
    credential_modes: []
  }

  function serve(providers: unknown[]) {
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/providers') return { data: providers }
      return { data: [] }
    })
  }

  /**
   * Names the form's controls in the order the document holds them.
   *
   * Deliberately not a snapshot: this has to keep meaning after somebody restyles the form, so
   * it reads the live DOM and identifies each control by what it is for rather than by the
   * markup around it.
   */
  function fieldOrder(form: HTMLFormElement): string[] {
    const seen = new Set<string>()
    const order: string[] = []
    for (const node of form.querySelectorAll('select, input, textarea')) {
      const name = fieldName(node)
      if (!name || seen.has(name)) continue
      seen.add(name)
      order.push(name)
    }
    return order
  }

  function fieldName(node: Element): string | null {
    const placeholder = node.getAttribute('placeholder')
    if (node.getAttribute('type') === 'radio') return 'sign-in method'
    if (node.getAttribute('type') === 'password') return 'secret'
    // The cookie session is a `UTextarea`, which the shared harness renders as a plain input,
    // so it is recognised by what it asks for rather than by its tag.
    if (node.tagName === 'TEXTAREA' || placeholder === network.account.cookies_placeholder) return 'cookie session'
    if (placeholder === network.account.label_placeholder) return 'display name'
    if (placeholder === network.account.username_placeholder) return 'username'
    if (placeholder === network.account.proxy_placeholder) return 'connection route'
    if (node.tagName === 'SELECT') return 'provider'
    return null
  }

  const radio = (name: string) => screen.getByRole('radio', { name }) as HTMLInputElement

  beforeEach(() => {
    get.mockReset()
  })

  it('puts the sign-in method straight after the provider, ahead of every field it governs', async () => {
    serve([withChoice])

    const { container } = mount()

    await screen.findByRole('radio', { name: network.account.credential_mode_login })
    const form = container.querySelector('form') as HTMLFormElement
    // Signing in leaves nothing to paste, so the cookie session is not among the fields — which
    // is the governance this order exists for.
    expect(fieldOrder(form)).toEqual([
      'provider',
      'sign-in method',
      'display name',
      'username',
      'connection route',
      'secret'
    ])

    await fireEvent.update(radio(network.account.credential_mode_api_key))

    await waitFor(() => expect(fieldOrder(form)).toContain('cookie session'))
    expect(fieldOrder(form)).toEqual([
      'provider',
      'sign-in method',
      'display name',
      'username',
      'connection route',
      'secret',
      'cookie session'
    ])
  })

  it('has the provider’s first sign-in method chosen when the form is first opened', async () => {
    serve([withChoice])

    mount()

    await waitFor(() => expect(radio(network.account.credential_mode_login).checked).toBe(true))
    expect(radio(network.account.credential_mode_api_key).checked).toBe(false)
  })

  it('offers no choice for a provider that has only one way to hold an account', async () => {
    serve([withoutChoice])

    mount()

    await waitFor(() => expect(screen.getByText(network.account.empty)).toBeTruthy())
    expect(screen.queryAllByRole('radio')).toEqual([])
    expect(screen.queryByText(network.account.credential_mode)).toBeNull()
  })

  it('names the secret after the chosen sign-in method, never after both at once', async () => {
    serve([withChoice])

    const { container } = mount()

    await waitFor(() => expect(radio(network.account.credential_mode_login).checked).toBe(true))
    const secret = () => container.querySelector('input[type="password"]') as HTMLInputElement
    expect(secret().getAttribute('aria-label')).toBe(network.account.secret_generic_password)

    await fireEvent.update(radio(network.account.credential_mode_api_key))

    await waitFor(() =>
      expect(secret().getAttribute('aria-label')).toBe(network.account.secret_generic_api_key)
    )
    expect(secret().getAttribute('aria-label')).not.toBe(network.account.secret_generic)
  })
})

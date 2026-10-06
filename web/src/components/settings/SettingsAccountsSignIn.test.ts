/**
 * A running sign-in stays on screen until it ends (RD-150-09).
 *
 * Reported from 1.5.1: the Real-Debrid code showed for five to ten seconds and was gone, while
 * the service went on polling the provider with it — and "Connect" then asked for a new code,
 * which made the one being typed worthless. A poll that did not answer used to write `null`
 * over the flow, which hid it and stopped the watch at once.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import network from '@/locales/en/network.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

/** The handlers the tab registered on the event stream, by channel. */
let handlers: Record<string, (event: MessageEvent<string>) => void> = {}
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (registered: Record<string, (event: MessageEvent<string>) => void>) => {
    handlers = registered
    return () => { handlers = {} }
  }
}))

const { default: SettingsAccountsCard } = await import('./SettingsAccountsCard.vue')

const PROVIDER = {
  slug: 'realdebrid',
  display_name: 'Real-Debrid',
  kind: 'multihoster',
  credentials: 'oauth_or_api_key',
  credential_modes: ['oauth', 'api_key'],
  username_required: false,
  device_flow: true
}
const ACCOUNT = {
  id: 'rd-code',
  label: 'Real-Debrid by code',
  provider: 'realdebrid',
  username: null,
  enabled: true,
  has_secret: false,
  has_cookies: false,
  credential_mode: 'oauth',
  proxy_profile_id: null
}
const RUNNING = {
  account_id: ACCOUNT.id,
  plugin_id: 'realdebrid-auth',
  state: 'polling',
  verification_url: 'https://real-debrid.com/device',
  user_code: 'WXYZ1234',
  expires_at: '2099-01-01T00:15:00Z',
  next_poll_at: '2099-01-01T00:00:05Z',
  message: null,
  started_at: '2099-01-01T00:00:00Z',
  token_expires_at: null
}

/** What `GET /accounts/{id}/auth` answers; a test moves it around. */
let flowAnswer: unknown = { data: null }

function serve() {
  get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/providers') return { data: [PROVIDER] }
    if (path === '/api/v1/accounts') return { data: [ACCOUNT] }
    if (path === '/api/v1/accounts/{id}/auth') return flowAnswer
    return { data: [] }
  })
}

function mount() {
  return mountComponent(SettingsAccountsCard, {
    messages: { network },
    stubs: { SettingsRemoteJobsCard: true }
  })
}

async function row(): Promise<HTMLElement> {
  return (await screen.findByText(ACCOUNT.label)).closest('.border') as HTMLElement
}

function flowEvent(): MessageEvent<string> {
  return {
    data: JSON.stringify({ payload: { entity: 'auth_flow', account_id: ACCOUNT.id, state: 'polling' } })
  } as MessageEvent<string>
}

const beginCalls = () => post.mock.calls.filter(([path]) => path === '/api/v1/accounts/{id}/auth/begin')

describe('SettingsAccountsCard while a sign-in runs', () => {
  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    handlers = {}
    flowAnswer = { data: RUNNING }
    serve()
  })

  it('shows a sign-in that was already running when the page is opened again', async () => {
    mount()

    const account = await row()
    await waitFor(() => expect(within(account).getByText('WXYZ1234')).toBeTruthy())
    const link = within(account).getByRole('link', { name: 'https://real-debrid.com/device' })
    expect(link.getAttribute('href')).toBe('https://real-debrid.com/device')
    expect(link.getAttribute('rel')).toContain('noopener')
    expect(within(account).getByRole('status').textContent).toContain(network.account.connect_waiting)
    expect(within(account).getByText(network.account.connect_polling.replace('{provider}', 'Real-Debrid'))).toBeTruthy()
    // Nothing offers a second code while this one is waiting.
    expect(within(account).queryByRole('button', { name: network.account.connect })).toBeNull()
  })

  it('keeps the code when the bus reports a step and the read after it does not answer', async () => {
    mount()

    const account = await row()
    await waitFor(() => expect(within(account).getByText('WXYZ1234')).toBeTruthy())
    expect(Object.keys(handlers)).toContain('account.changed')

    flowAnswer = { error: { code: 'internal', message: 'busy' }, response: new Response(null, { status: 503 }) }
    get.mockClear()
    handlers['account.changed']?.(flowEvent())
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/accounts/{id}/auth', expect.anything()))
    await new Promise(resolve => setTimeout(resolve, 0))

    expect(within(account).getByText('WXYZ1234')).toBeTruthy()

    // A step the service did record is shown as soon as it is announced.
    flowAnswer = { data: { ...RUNNING, state: 'failed', user_code: null, verification_url: null, message: 'the sign-in window expired' } }
    handlers['account.changed']?.(flowEvent())
    await waitFor(() => expect(within(account).queryByText('WXYZ1234')).toBeNull())
    expect(within(account).getByText('the sign-in window expired')).toBeTruthy()
  })

  it('shows the running sign-in on "Connect" instead of asking for a new code', async () => {
    // The page does not know about the flow yet, e.g. a read failed before the fix; the
    // service does.
    flowAnswer = { data: null }
    mount()

    const account = await row()
    const connect = await within(account).findByRole('button', { name: network.account.connect })
    flowAnswer = { data: RUNNING }
    await fireEvent.click(connect)

    await waitFor(() => expect(within(account).getByText('WXYZ1234')).toBeTruthy())
    expect(beginCalls()).toHaveLength(0)
  })

  it('starts a sign-in when none is running', async () => {
    flowAnswer = { data: null }
    post.mockImplementation(async (path: string) =>
      path === '/api/v1/accounts/{id}/auth/begin' ? { data: { ...RUNNING, state: 'waiting_for_user' } } : { data: null })
    mount()

    const account = await row()
    await fireEvent.click(await within(account).findByRole('button', { name: network.account.connect }))

    await waitFor(() => expect(within(account).getByText('WXYZ1234')).toBeTruthy())
    expect(beginCalls()).toHaveLength(1)
  })
})

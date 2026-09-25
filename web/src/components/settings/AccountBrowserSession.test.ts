/**
 * "Take over from browser" at an account (RD-120-45).
 *
 * The service cannot read a browser's cookies, so the account row opens a request the browser
 * extension answers. What these hold: the offer exists only where a plugin declares the site,
 * the waiting line says where to answer and whether an extension is around, and a session that
 * arrived makes the row re-read the account and check it.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import captcha from '@/locales/en/captcha.json'
import network from '@/locales/en/network.json'
import { mountComponent } from '@/test/mount'

import { BROWSER_SESSION_POLL_MS } from '@/composables/useBrowserSessions'

const get = vi.fn()
const post = vi.fn()
const del = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: (...args: unknown[]) => del(...args)
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))

const { default: SettingsAccountsTab } = await import('./SettingsAccountsTab.vue')

const SCOPED = {
  id: 'account-ddl',
  label: 'DDownload main',
  provider: 'ddownload',
  username: 'someone',
  enabled: true,
  has_secret: true,
  has_cookies: false,
  credential_mode: 'login',
  proxy_profile_id: null
}
const UNSCOPED = { ...SCOPED, id: 'account-rg', label: 'Second hoster account', provider: 'rapidgator', credential_mode: null }
const PROVIDERS = [
  { slug: 'ddownload', display_name: 'DDownload', kind: 'hoster', credentials: 'login_or_api_key', credential_modes: ['api_key', 'login'], username_required: false, device_flow: false, cookie_scope_host: 'ddownload.com' },
  { slug: 'rapidgator', display_name: 'Rapidgator', kind: 'hoster', credentials: 'username_password', credential_modes: [], username_required: true, device_flow: false }
]
const WAITING = { id: 'request-1', state: 'waiting', host: 'ddownload.com', expires_at: '2026-09-23T20:05:00Z' }

let session: Record<string, unknown> | null = null
let extensionConnected = false

function answer(path: string) {
  if (path === '/api/v1/accounts') return { data: [SCOPED, UNSCOPED] }
  if (path === '/api/v1/providers') return { data: PROVIDERS }
  if (path === '/api/v1/captcha-answerers') return { data: { browser_extension_connected: extensionConnected } }
  if (path.endsWith('/browser-session')) return session ? { data: session } : { data: undefined, error: { code: 'browser_session.none' } }
  if (path.includes('/auth')) return { data: null }
  return { data: [] }
}

function mount() {
  return mountComponent(SettingsAccountsTab, {
    messages: { network, captcha },
    stubs: { SettingsRemoteJobsCard: true }
  })
}

describe('Take over from browser', () => {
  beforeEach(() => {
    session = null
    extensionConnected = false
    get.mockReset().mockImplementation(async (path: string) => answer(path))
    post.mockReset().mockImplementation(async (path: string) => {
      if (path.endsWith('/browser-session')) {
        session = WAITING
        return { data: WAITING }
      }
      return { data: { valid: true, premium: false, label: [], traffic_left: null } }
    })
    del.mockReset().mockResolvedValue({ data: { code: 'browser_session.cancelled', message: '' } })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('is offered only where the provider\'s plugin declares the site', async () => {
    mount()

    const scoped = (await screen.findByText('DDownload main')).closest('.border') as HTMLElement
    const unscoped = screen.getByText('Second hoster account').closest('.border') as HTMLElement
    expect(within(scoped).getByRole('button', { name: network.account.browser_session.take_over })).toBeTruthy()
    expect(within(unscoped).queryByRole('button', { name: network.account.browser_session.take_over })).toBeNull()
  })

  it('opens a request for that account and says where to answer it, and that no extension was seen', async () => {
    mount()
    const row = (await screen.findByText('DDownload main')).closest('.border') as HTMLElement

    await fireEvent.click(within(row).getByRole('button', { name: network.account.browser_session.take_over }))

    expect(post).toHaveBeenCalledWith('/api/v1/accounts/{id}/browser-session', { params: { path: { id: 'account-ddl' } } })
    const panel = await within(row).findByTestId('browser-session')
    expect(panel.textContent).toContain(network.account.browser_session.waiting.replace('{host}', 'ddownload.com'))
    await waitFor(() => expect(within(panel).getByTestId('browser-session-extension-missing')).toBeTruthy())

    await fireEvent.click(within(panel).getByRole('button', { name: network.account.browser_session.cancel }))
    expect(del).toHaveBeenCalledWith('/api/v1/accounts/{id}/browser-session', { params: { path: { id: 'account-ddl' } } })
    await waitFor(() => expect(within(row).queryByTestId('browser-session')).toBeNull())
  })

  it('does not warn about a missing extension when one is connected', async () => {
    extensionConnected = true
    mount()
    const row = (await screen.findByText('DDownload main')).closest('.border') as HTMLElement

    await fireEvent.click(within(row).getByRole('button', { name: network.account.browser_session.take_over }))

    const panel = await within(row).findByTestId('browser-session')
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/captcha-answerers'))
    expect(within(panel).queryByTestId('browser-session-extension-missing')).toBeNull()
  })

  it('re-reads the account and checks it once the session arrived', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    mount()
    const row = (await screen.findByText('DDownload main')).closest('.border') as HTMLElement
    await fireEvent.click(within(row).getByRole('button', { name: network.account.browser_session.take_over }))
    await within(row).findByTestId('browser-session')
    const accountReads = get.mock.calls.filter(([path]) => path === '/api/v1/accounts').length

    session = { ...WAITING, state: 'delivered' }
    await vi.advanceTimersByTimeAsync(BROWSER_SESSION_POLL_MS)

    await waitFor(() =>
      expect(within(row).getByTestId('browser-session').textContent)
        .toContain(network.account.browser_session.delivered.replace('{host}', 'ddownload.com'))
    )
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/accounts/{id}/test', { params: { path: { id: 'account-ddl' } } }))
    expect(get.mock.calls.filter(([path]) => path === '/api/v1/accounts').length).toBeGreaterThan(accountReads)
  })
})

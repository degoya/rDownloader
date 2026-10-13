/**
 * The account row's layout (RD-1260-05).
 *
 * Switch, name, status icons and every action shared one wrapping line. With *Take over from
 * browser* among them the line no longer fit: *Edit* and *Delete*, or *Delete* alone, broke onto
 * a line of their own under the switch, so the icons stood somewhere else on every account and an
 * account without the handover looked different again. The first line is now fixed and what only
 * some accounts have goes to a second one.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, confirmMock, eventStreamMock, toastMock } from '@/test/mocks'

import common from '@/locales/en/common.json'
import network from '@/locales/en/network.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => apiClientMock({ GET: (...args: unknown[]) => get(...args) }))
vi.mock('@/composables/useConfirm', () => confirmMock())
vi.mock('@nuxt/ui/composables', () => toastMock())
vi.mock('@/composables/useEventStream', () => eventStreamMock())

const { default: SettingsAccountsCard } = await import('./SettingsAccountsCard.vue')

const BASE = {
  username: 'someone',
  enabled: true,
  has_secret: true,
  has_cookies: false,
  credential_mode: null,
  proxy_profile_id: null
}
/** The owner's three accounts: a key with the cookie handover, a login with it, one without. */
const ACCOUNTS = [
  { ...BASE, id: 'key', label: 'Key with cookies', provider: 'keyhoster', has_cookies: true },
  { ...BASE, id: 'login', label: 'Login with handover', provider: 'loginhoster' },
  { ...BASE, id: 'plain', label: 'Plain login', provider: 'plainhoster' }
]
const provider = (slug: string, credentials: string, cookieScopeHost: string | null) => ({
  slug,
  display_name: slug,
  kind: 'hoster',
  credentials,
  credential_modes: [],
  username_required: false,
  device_flow: false,
  cookie_scope_host: cookieScopeHost
})
const PROVIDERS = [
  provider('keyhoster', 'api_key', 'key.example'),
  provider('loginhoster', 'password', 'login.example'),
  provider('plainhoster', 'password', null)
]

function mount() {
  return mountComponent(SettingsAccountsCard, { messages: { network } })
}

async function row(label: string): Promise<HTMLElement> {
  return (await screen.findByText(label)).closest('.border') as HTMLElement
}

/** The accessible names of the buttons in the row's fixed action column, in order. */
function actionNames(element: HTMLElement): (string | null)[] {
  const actions = element.querySelector('[data-account-actions]') as HTMLElement
  return within(actions).getAllByRole('button').map(button => button.getAttribute('aria-label'))
}

describe('SettingsAccountsCard account row', () => {
  beforeEach(() => {
    get.mockReset()
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/accounts') return { data: ACCOUNTS }
      if (path === '/api/v1/providers') return { data: PROVIDERS }
      if (path.endsWith('/hosters')) return { data: { hosters: ['files.example'] } }
      if (path.includes('/auth') || path.includes('/browser-session')) return { data: null }
      return { data: [] }
    })
  })

  it('has Test, Edit and Delete in the same column and order on every account', async () => {
    mount()

    for (const account of ACCOUNTS) {
      expect(actionNames(await row(account.label))).toEqual([
        common.actions.test,
        network.account.edit,
        network.account.delete
      ])
    }
  })

  it('builds every first line from the same four cells, status slots included', async () => {
    mount()

    for (const account of ACCOUNTS) {
      const first = (await row(account.label)).firstElementChild as HTMLElement
      expect(first.className).toContain('grid-cols-[auto_minmax(0,1fr)_auto_auto]')
      expect(first.className).not.toContain('flex-wrap')
      const cells = Array.from(first.children).slice(0, 4)
      expect(cells[0]!.getAttribute('role')).toBe('switch')
      expect(cells[1]!.textContent).toContain(account.label)
      expect(cells[2]!.hasAttribute('data-account-status')).toBe(true)
      expect(cells[3]!.hasAttribute('data-account-actions')).toBe(true)
    }
  })

  it('puts the browser handover on a second line, and gives a row without it none', async () => {
    mount()
    const takeOver = network.account.browser_session.take_over

    for (const label of ['Key with cookies', 'Login with handover']) {
      const element = await row(label)
      const extras = element.querySelector('[data-account-extras]') as HTMLElement
      expect(within(extras).getByRole('button', { name: takeOver })).toBeTruthy()
      expect(within(element.querySelector('[data-account-actions]') as HTMLElement).queryByRole('button', { name: takeOver })).toBeNull()
    }
    const plain = await row('Plain login')
    expect(plain.querySelector('[data-account-extras]')).toBeNull()
    expect(within(plain).queryByRole('button', { name: takeOver })).toBeNull()
  })

  it('keeps Test as an icon with its name on a narrow row', async () => {
    mount()

    const test = within((await row('Plain login')).querySelector('[data-account-actions]') as HTMLElement)
      .getByRole('button', { name: common.actions.test })
    expect(test.getAttribute('title')).toBe(common.actions.test)
  })

  it('opens the supported hosters from a left-aligned toggle that says whether it is open', async () => {
    mount()

    const toggle = within(await row('Plain login')).getByRole('button', { name: network.hosters.toggle })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(toggle.className).toContain('-ml-2')
    expect(toggle.className).not.toContain('justify-between')

    await fireEvent.click(toggle)

    await waitFor(() => expect(screen.getByText('files.example')).toBeTruthy())
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(get).toHaveBeenCalledWith('/api/v1/accounts/{id}/hosters', { params: { path: { id: 'plain' } } })
  })
})

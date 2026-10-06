import { cleanup, fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'
import { leaveFor } from '@/utils/identityProvider'

import SettingsOidcCard from './SettingsOidcCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PUT: vi.fn(), POST: vi.fn(), DELETE: vi.fn() },
  responseError: () => 'refused'
}))
const toastAdd = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toastAdd }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))
vi.mock('@/composables/useCopy', () => ({ useCopy: () => async () => true }))
vi.mock('@/utils/identityProvider', () => ({ leaveFor: vi.fn() }))
const route = { path: '/settings/security', query: {} as Record<string, string>, hash: '' }
const replace = vi.fn()
vi.mock('vue-router', () => ({ useRoute: () => route, useRouter: () => ({ replace }) }))

/** The notice as a `note`, so a test can read what it says. */
const UAlert = {
  props: ['title', 'description'],
  template: '<div role="note">{{ title }} {{ description }}</div>'
}

const REDIRECT = 'https://dl.example.com/api/v1/auth/oidc/callback'

function settings(overrides: Record<string, unknown> = {}) {
  return {
    data: {
      configured: true,
      issuer: 'https://id.example.com/',
      client_id: 'rdownloader',
      display_name: 'Pocket ID',
      client_secret_set: true,
      group_claim: null,
      group_value: null,
      provider_logout: false,
      redirect_uri: REDIRECT,
      identity: null,
      password_login: true,
      provider_session: false,
      ...overrides
    }
  }
}

function mount() {
  return mountComponent(SettingsOidcCard, { messages: { system }, stubs: { UAlert } })
}

async function typePassword() {
  await fireEvent.update(screen.getByLabelText(system.mfa.step_up.label), 'correct-horse-battery')
}

function button(label: string): HTMLButtonElement {
  const element = screen.getByText(label)
  if (!(element instanceof HTMLButtonElement)) throw new Error(`${label} is not a button`)
  return element
}

describe('SettingsOidcCard', () => {
  beforeEach(() => {
    cleanup()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
    vi.mocked(leaveFor).mockReset()
    toastAdd.mockReset()
    replace.mockReset()
    route.query = {}
  })

  /// The address to copy into the provider, which only the external URL can give.
  it('shows the redirect URI, or says that the external URL is missing', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce(settings() as never)
    mount()
    await waitFor(() => screen.getByDisplayValue(REDIRECT))

    cleanup()
    vi.mocked(api.GET).mockResolvedValueOnce(settings({ redirect_uri: null }) as never)
    mount()
    await waitFor(() => screen.getByText(system.oidc.external_url_missing, { exact: false }))
  })

  /// Linking is a round trip: the password again, then the page goes to the provider.
  it('links an account by sending the browser to the provider', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce(settings() as never)
    vi.mocked(api.POST).mockResolvedValueOnce({
      data: { authorization_url: 'https://id.example.com/authorize?state=s' }
    } as never)
    mount()
    await waitFor(() => screen.getByText(system.oidc.link))
    expect(button(system.oidc.link).disabled).toBe(true)

    await typePassword()
    await fireEvent.click(button(system.oidc.link))

    expect(api.POST).toHaveBeenCalledWith('/api/v1/auth/oidc/link', {
      body: { password: 'correct-horse-battery' }
    })
    await waitFor(() => expect(leaveFor).toHaveBeenCalledWith('https://id.example.com/authorize?state=s'))
  })

  /// D3: only a session the provider opened may switch the password form off.
  it('offers switching the password off only from a provider session', async () => {
    const identity = { issuer: 'https://id.example.com/', label: 'owner', linked_at: '2026-10-02T10:00:00Z' }
    vi.mocked(api.GET).mockResolvedValueOnce(settings({ identity }) as never)
    mount()
    await waitFor(() => screen.getByText(system.oidc.password_off.needs_provider_session))
    await typePassword()
    expect(button(system.oidc.password_off.action).disabled).toBe(true)

    cleanup()
    vi.mocked(api.GET).mockResolvedValueOnce(settings({ identity, provider_session: true }) as never)
    vi.mocked(api.POST).mockResolvedValueOnce({ data: { code: 'auth.password_login_switched_off' } } as never)
    vi.mocked(api.GET).mockResolvedValueOnce(settings({ identity, password_login: false }) as never)
    mount()
    await waitFor(() => screen.getByText(system.oidc.password_off.ready))
    await typePassword()
    await fireEvent.click(button(system.oidc.password_off.action))
    expect(api.POST).toHaveBeenCalledWith('/api/v1/auth/password-login/off', {
      body: { password: 'correct-horse-battery' }
    })
    // Off: the way back is the command on the machine, and nothing here ends the provider sign-in.
    await waitFor(() => screen.getByText(system.oidc.password_is_off, { exact: false }))
    expect(screen.getByText(/rdownloader auth password-login on/)).toBeTruthy()
    expect(button(system.oidc.unlink.action).disabled).toBe(true)
  })

  /// The provider sends the browser back here after a link; the address is tidied afterwards.
  it('says a link finished and takes the marker out of the address', async () => {
    route.query = { oidc: 'linked', tab: 'signin' }
    vi.mocked(api.GET).mockResolvedValueOnce(settings() as never)
    mount()
    await waitFor(() => expect(toastAdd).toHaveBeenCalled())
    expect(replace).toHaveBeenCalledWith({ path: '/settings/security', query: { tab: 'signin' }, hash: '' })
  })
})

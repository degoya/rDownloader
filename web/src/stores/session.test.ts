import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import { leaveFor } from '@/utils/identityProvider'

import { useSessionStore } from './session'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The service could not be reached'),
  errorMessage: vi.fn()
}))

vi.mock('@/utils/identityProvider', async (original) => ({
  ...(await original<typeof import('@/utils/identityProvider')>()),
  leaveFor: vi.fn()
}))

vi.mock('@/webauthn', () => ({
  PasskeyAbort: class extends Error {},
  getAssertion: vi.fn(),
  passkeysSupported: () => false
}))

describe('session store, signing out', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
  })

  it('ends the session and returns to the sign-in screen', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: {} } as never)
    const session = useSessionStore()
    session.authenticated = true

    await session.logout()

    expect(api.POST).toHaveBeenCalledWith('/api/v1/auth/logout')
    expect(session.authenticated).toBe(false)
    expect(session.error).toBeNull()
  })

  it('signs this browser out even when the request failed', async () => {
    // Otherwise pressing "sign out", seeing an error and walking away leaves a session open.
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'x' } } as never)
    const session = useSessionStore()
    session.authenticated = true

    await session.logout()

    expect(session.authenticated).toBe(false)
    expect(session.error).toBe('The service could not be reached')
  })

  it('closes the wizard, so it cannot outlive the session', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: {} } as never)
    const session = useSessionStore()
    session.authenticated = true
    session.wizardActive = true

    await session.logout()

    expect(session.wizardActive).toBe(false)
  })
})

describe('session store, a lapsed session (RD-130-09)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('returns to the sign-in and says why', () => {
    const session = useSessionStore()
    session.authenticated = true
    session.wizardActive = true

    session.expire()

    expect(session.authenticated).toBe(false)
    expect(session.expired).toBe(true)
    // The wizard cannot save anything without a session, so it must not stay in front.
    expect(session.wizardActive).toBe(false)
    expect(session.error).toBeNull()
  })

  it('ignores a refusal that arrives after the sign-in screen is already showing', () => {
    // Every request in flight when the session lapsed comes back refused; only the first one
    // is news, and a refusal on the sign-in screen itself is not an expiry.
    const session = useSessionStore()
    session.authenticated = false

    session.expire()

    expect(session.expired).toBe(false)
  })

  it('clears the notice once signed in again', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: {} } as never)
    vi.mocked(api.GET).mockResolvedValue({ data: { wizard_completed: true } } as never)
    const session = useSessionStore()
    session.authenticated = true
    session.expire()

    await session.submitPassword('correct-horse-battery')

    expect(session.authenticated).toBe(true)
    expect(session.expired).toBe(false)
  })
})

describe('session store, the identity provider (RD-190-15)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
    vi.mocked(leaveFor).mockReset()
  })

  it('offers the provider only when the service says it can be used', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce({
      data: {
        setup_required: false,
        authenticated: false,
        oidc_available: true,
        oidc_display_name: 'Pocket ID',
        password_login: false
      }
    } as never)
    const session = useSessionStore()

    await session.initialize()

    expect(session.providerName).toBe('Pocket ID')
    expect(session.passwordLogin).toBe(false)
  })

  it('keeps the password form for a service that says nothing about it', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce({
      data: { setup_required: false, authenticated: false, oidc_display_name: 'Ignored' }
    } as never)
    const session = useSessionStore()

    await session.initialize()

    expect(session.providerName).toBeNull()
    expect(session.passwordLogin).toBe(true)
  })

  it('starts the sign-in as a page load that comes back where it was', () => {
    const session = useSessionStore()

    session.signInWithProvider('/downloads')

    expect(leaveFor).toHaveBeenCalledWith('/api/v1/auth/oidc/start?return_to=%2Fdownloads')
  })

  it('signs out at the provider too only when the service hands out where', async () => {
    const session = useSessionStore()
    session.authenticated = true
    vi.mocked(api.POST).mockResolvedValueOnce({ data: { code: 'auth.logged_out' } } as never)
    await session.logout()
    expect(leaveFor).not.toHaveBeenCalled()

    session.authenticated = true
    vi.mocked(api.POST).mockResolvedValueOnce({
      data: { code: 'auth.logged_out', provider_logout_url: 'https://id.example.com/logout?x=1' }
    } as never)
    await session.logout()
    expect(leaveFor).toHaveBeenCalledWith('https://id.example.com/logout?x=1')
    expect(session.authenticated).toBe(false)
  })
})

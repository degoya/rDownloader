import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useSessionStore } from './session'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The service could not be reached'),
  errorMessage: vi.fn()
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

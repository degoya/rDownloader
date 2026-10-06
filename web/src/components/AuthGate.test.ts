import { cleanup, fireEvent, render, screen } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import auth from '@/locales/en/auth.json'
import { useSessionStore } from '@/stores/session'
import { createTestI18n, uiStubs } from '@/test/mount'

import AuthGate from './AuthGate.vue'

vi.mock('@/api/client', () => ({
  // The signature under the form asks for the service's version.
  api: { GET: vi.fn(async () => ({ data: {} })), POST: vi.fn() },
  responseError: vi.fn(() => 'refused')
}))
vi.mock('@/i18n/server', () => ({
  translateServerMessage: (message: { code?: string }) => `translated ${message.code}`
}))
vi.mock('@/utils/identityProvider', async (original) => ({
  ...(await original<typeof import('@/utils/identityProvider')>()),
  leaveFor: vi.fn()
}))

/** The notice as a `note`, so a test can read the refusal it carries. */
const UAlert = { props: ['title', 'description'], template: '<div role="note">{{ title }} {{ description }}</div>' }

/** Rendered directly rather than through the shared mount helper, which starts a fresh Pinia: each test sets the session store up before the mount. */
function mount() {
  return render(AuthGate, { global: { plugins: [createTestI18n({ auth })], stubs: { ...uiStubs, UAlert } as never } })
}

describe('AuthGate and the identity provider (RD-190-15)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    window.history.replaceState(null, '', '/downloads')
  })
  afterEach(() => cleanup())

  it('offers the provider by its name and starts there, coming back to this page', async () => {
    const session = useSessionStore()
    session.providerName = 'Pocket ID'
    const start = vi.spyOn(session, 'signInWithProvider')
    mount()

    await fireEvent.click(screen.getByText('Sign in with Pocket ID'))

    expect(start).toHaveBeenCalledWith('/downloads')
    // The password form stays below it while the password sign-in is on.
    expect(screen.getByText(auth.password)).toBeTruthy()
  })

  it('leaves out the password form once the password sign-in is switched off', () => {
    const session = useSessionStore()
    session.providerName = 'Pocket ID'
    session.passwordLogin = false
    mount()

    expect(screen.queryByText(auth.password)).toBeNull()
    expect(screen.getByText(auth.password_off)).toBeTruthy()
  })

  it('shows why the provider sign-in was refused, with the account it named, once', () => {
    window.history.replaceState(
      null,
      '',
      '/?oidc_error=auth.oidc_not_administrator&oidc_name=guest'
    )
    mount()

    expect(screen.getByRole('note').textContent).toContain('translated auth.oidc_not_administrator')
    expect(screen.getByRole('note').textContent).toContain('guest')
    expect(window.location.search).toBe('')
  })
})

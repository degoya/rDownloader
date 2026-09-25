import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'

import { api, responseError } from '@/api/client'
import { currentLocale } from '@/i18n'
import { loadPluginMessages, setPluginMessagesAvailable } from '@/i18n/plugins'
import { PasskeyAbort, getAssertion, passkeysSupported } from '@/webauthn'

export const useSessionStore = defineStore('session', () => {
  const initialized = ref(false)
  const setupRequired = ref(false)
  const authenticated = ref(false)
  /** The password was right and a code from the authenticator app is still needed. */
  const mfaRequired = ref(false)
  const loginDisabled = ref(false)
  /** A passkey is enrolled and this browser can use one, so the sign-in screen offers it. */
  const passkeyOffered = ref(false)
  const pending = ref(false)
  const error = ref<string | null>(null)
  /**
   * The session ran out while the interface was open (RD-130-09), so the sign-in screen says
   * why it is there instead of looking like the service forgot who this was.
   */
  const expired = ref(false)

  // Pessimistic default: an install that has run before must never flash the wizard while
  // `/setup/status` is still in flight.
  const wizardCompleted = ref(true)
  const wizardActive = ref(false)
  const wizardRerun = ref(false)
  // Set by the wizard's final step; the tour can only start once the control room is mounted.
  const tourPending = ref(false)

  const ready = computed(() => initialized.value && authenticated.value)

  async function initialize(): Promise<void> {
    pending.value = true
    const response = await api.GET('/api/v1/auth/status')
    pending.value = false
    initialized.value = true
    if (response.data) {
      setupRequired.value = response.data.setup_required
      authenticated.value = response.data.authenticated
      loginDisabled.value = response.data.login_disabled ?? false
      passkeyOffered.value = (response.data.passkeys_available ?? false) && passkeysSupported()
      error.value = null
      if (setupRequired.value) {
        wizardCompleted.value = false
        wizardActive.value = true
      } else if (authenticated.value) {
        await loadSetupStatus()
      }
    } else {
      error.value = responseError(response)
    }
  }

  /** Reads derived wizard completion; opens the wizard when setup is still outstanding. */
  async function loadSetupStatus(): Promise<void> {
    const response = await api.GET('/api/v1/setup/status')
    if (!response.data) return
    wizardCompleted.value = response.data.wizard_completed
    if (!response.data.wizard_completed) wizardActive.value = true
  }

  async function submitPassword(password: string, code?: string): Promise<boolean> {
    pending.value = true
    error.value = null
    if (setupRequired.value) {
      const response = await api.POST('/api/v1/auth/setup', { body: { password } })
      if (!response.data) {
        pending.value = false
        error.value = responseError(response)
        return false
      }
      setupRequired.value = false
      pending.value = false
      return submitPassword(password)
    }
    const response = await api.POST('/api/v1/auth/login', {
      body: { password, code: code || null }
    })

    if (!response.data) {
      pending.value = false
      // The server asks for the second factor only once the password was accepted, so this is
      // a prompt rather than a failure — showing it as an error would read as "wrong password".
      if ((response.error as { code?: string } | undefined)?.code === 'auth.mfa_required') {
        mfaRequired.value = true
        return false
      }
      error.value = responseError(response)
      return false
    }
    mfaRequired.value = false
    expired.value = false
    authenticated.value = true
    pending.value = false
    // Covers the resume path: password was set in an earlier, abandoned wizard run.
    await loadSetupStatus()
    return true
  }

  /**
   * Signs in with a passkey, which replaces the password rather than adding to it.
   *
   * The authenticator has already verified the person — a PIN or a fingerprint — before it
   * will sign, so this single step carries both factors. Asking for the password as well
   * would add nothing and cost the reason passkeys are worth having.
   */
  async function submitPasskey(): Promise<boolean> {
    pending.value = true
    error.value = null
    const started = await api.POST('/api/v1/auth/passkey/challenge')
    if (!started.data) {
      pending.value = false
      error.value = responseError(started)
      return false
    }
    let assertion: unknown
    try {
      const options = (started.data.options as unknown as { publicKey: Record<string, unknown> })
        .publicKey
      assertion = await getAssertion(options)
    } catch (cause) {
      pending.value = false
      // Cancelling the browser's dialog is a choice, not a failure; saying nothing lets the
      // person simply type their password instead.
      if (!(cause instanceof PasskeyAbort)) error.value = responseError(started)
      return false
    }
    const response = await api.POST('/api/v1/auth/passkey/login', {
      body: {
        ceremony_id: started.data.ceremony_id,
        credential: assertion as Record<string, never>
      }
    })
    pending.value = false
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    mfaRequired.value = false
    expired.value = false
    authenticated.value = true
    await loadSetupStatus()
    return true
  }

  /**
   * Ends the session on the server and returns to the sign-in screen (RD-101-19).
   *
   * The local state is cleared whatever the server answered. A logout that failed to reach
   * the service still has to leave this browser signed out -- the alternative is somebody
   * pressing "sign out", seeing an error, and walking away from an open session.
   */
  async function logout(): Promise<void> {
    pending.value = true
    const response = await api.POST('/api/v1/auth/logout')
    pending.value = false
    authenticated.value = false
    mfaRequired.value = false
    wizardActive.value = false
    wizardRerun.value = false
    tourPending.value = false
    expired.value = false
    error.value = response.error ? responseError(response) : null
  }

  /**
   * Returns to the sign-in screen because the service no longer accepts this session — it
   * idled out, reached its maximum lifetime, or was ended from another browser (RD-130-09).
   *
   * Only once: the requests that were in flight when it lapsed all come back refused, and
   * the first one is the news. The wizard closes like on a sign-out, because nothing in it can
   * be saved without a session.
   */
  function expire(): void {
    if (!authenticated.value) return
    authenticated.value = false
    mfaRequired.value = false
    wizardActive.value = false
    wizardRerun.value = false
    tourPending.value = false
    error.value = null
    expired.value = true
  }

  /** Reopens the wizard from the settings; every step is optional in this mode. */
  function openWizard(): void {
    wizardRerun.value = true
    wizardActive.value = true
  }

  function finishWizard(startTour = false): void {
    wizardActive.value = false
    wizardRerun.value = false
    wizardCompleted.value = true
    tourPending.value = startTour
  }

  function consumeTourRequest(): boolean {
    if (!tourPending.value) return false
    tourPending.value = false
    return true
  }

  // Plugin translations sit behind the sign-in, so they can only be fetched once a session
  // exists. Watching the flag covers every way it flips — a reload that restores a session,
  // the password path, the passkey path, and signing out — rather than three call sites that
  // would each have to remember.
  watch(authenticated, (value) => {
    setPluginMessagesAvailable(value)
    if (value) void loadPluginMessages(currentLocale())
  })

  return {
    authenticated,
    consumeTourRequest,
    error,
    expire,
    expired,
    finishWizard,
    initialize,
    initialized,
    loadSetupStatus,
    loginDisabled,
    logout,
    mfaRequired,
    openWizard,
    passkeyOffered,
    pending,
    ready,
    setupRequired,
    submitPasskey,
    submitPassword,
    tourPending,
    wizardActive,
    wizardCompleted,
    wizardRerun
  }
})

import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsPasswordCard from './SettingsPasswordCard.vue'

vi.mock('@/api/client', () => ({
  api: { POST: vi.fn() },
  responseError: () => 'refused'
}))
// Same treatment as SettingsMfaCard.test.ts: useToast reaches for Nuxt's auto-import alias,
// which Vitest cannot resolve.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const stubs = {
  // The error sits *outside* the label on purpose: inside, it would join the field's
  // accessible name and `field()` below would stop finding the input the moment the
  // mismatch it is checking appears.
  UFormField: {
    props: ['label', 'error'],
    template: '<div><label>{{ label }}<slot /></label><span v-if="error">{{ error }}</span></div>'
  }
}

function mount() {
  return mountComponent(SettingsPasswordCard, { messages: { system }, stubs })
}

/// One password field by its label, narrowed rather than asserted.
function field(label: string): HTMLInputElement {
  const element = screen.getByLabelText(label)
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`${label} is not an input`)
  }
  return element
}

const CURRENT = system.password.current_label
const NEW = system.password.new_label
const CONFIRM = system.password.confirm_label

async function fill(current: string, next: string, confirmation: string) {
  await fireEvent.update(field(CURRENT), current)
  await fireEvent.update(field(NEW), next)
  await fireEvent.update(field(CONFIRM), confirmation)
}

describe('SettingsPasswordCard', () => {
  beforeEach(() => {
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.POST).mockResolvedValue({ data: { code: 'auth.password_changed' } } as never)
  })

  /// What the card says out loud, because neither is guessable from the form.
  it('says that every session ends and that API tokens do not', () => {
    mount()

    expect(screen.getByText(system.password.sessions_hint)).toBeTruthy()
    expect(screen.getByText(system.password.tokens_hint)).toBeTruthy()
  })

  /// A typo in the replacement is a mistake only the person typing can see: the server has no
  /// second copy to compare it against, so this guard exists nowhere else.
  it('refuses to send a replacement the confirmation does not match', async () => {
    mount()
    await fill('correct-horse-battery', 'a-new-passphrase', 'a-new-passphrasf')

    await waitFor(() => {
      expect(screen.getByText(system.password.mismatch)).toBeTruthy()
    })
    await fireEvent.click(screen.getByRole('button'))
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('sends both passwords and then clears every field', async () => {
    mount()
    await fill('correct-horse-battery', 'a-new-passphrase', 'a-new-passphrase')
    await fireEvent.click(screen.getByRole('button'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith('/api/v1/auth/password', {
        body: { current_password: 'correct-horse-battery', new_password: 'a-new-passphrase' }
      })
    })
    // Three filled password fields left on screen after a successful change is the habit this
    // card exists to break.
    await waitFor(() => {
      expect([field(CURRENT).value, field(NEW).value, field(CONFIRM).value]).toEqual(['', '', ''])
    })
  })
})

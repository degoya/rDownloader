/**
 * The setup wizard offers "Back" only where there is a step to go back to (RD-120-48).
 *
 * The screenshot run caught it on the password step, drawn disabled but drawn: a way back out of
 * the first step reads as an offer the wizard cannot keep.
 */
import { fireEvent, render, screen } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'

import wizard from '@/locales/en/wizard.json'
import { useSessionStore } from '@/stores/session'
import { createTestI18n, uiStubs } from '@/test/mount'

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({ data: undefined })),
    POST: vi.fn(async () => ({ data: undefined }))
  },
  responseError: vi.fn(() => '')
}))

// The steps are someone else's subject, and some of them reach Nuxt UI's `#imports`.
const empty = { default: { template: '<div />' } }
vi.mock('@/components/AppSignature.vue', () => empty)
vi.mock('@/components/settings/SettingsMcpAccess.vue', () => empty)
vi.mock('@/components/wizard/WizardPairingStep.vue', () => empty)
vi.mock('@/components/wizard/WizardPasswordStep.vue', () => empty)
vi.mock('@/components/wizard/WizardServicesStep.vue', () => empty)
vi.mock('@/components/wizard/WizardStorageStep.vue', () => empty)

const { default: SetupWizard } = await import('./SetupWizard.vue')

const stubs = {
  ...uiStubs,
  UStepper: true
}

describe('SetupWizard', () => {
  it('shows no Back on the first step and one on the next', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    // A re-run starts on the password step with the password already set, so it can advance.
    useSessionStore().wizardRerun = true
    render(SetupWizard, { global: { plugins: [pinia, createTestI18n({ wizard })] as never[], stubs: stubs as never } })

    expect(screen.queryByRole('button', { name: wizard.actions.back })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: wizard.actions.continue }))
    expect(screen.getByRole('button', { name: wizard.actions.back })).toBeTruthy()
  })
})

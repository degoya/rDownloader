import { cleanup, render, screen, waitFor } from '@testing-library/vue'
import { fireEvent } from '@testing-library/dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import system from '@/locales/en/system.json'

import SettingsMfaCard from './SettingsMfaCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: () => 'failed'
}))
// The card pulls in useToast, which reaches for Nuxt's auto-import alias that Vitest cannot
// resolve. Same treatment as IndexerReviewList.test.ts.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { system } } })
const components = {
  UAlert: { props: ['description'], template: '<div>{{ description }}</div>' },
  UBadge: { template: '<span><slot /></span>' },
  UButton: { props: ['label'], template: '<button>{{ label }}<slot /></button>' },
  UFormField: { props: ['label'], template: '<label>{{ label }}<slot /></label>' },
  UInput: { template: '<input>' }
}

const OTPAUTH = 'otpauth://totp/rDownloader:administrator?secret=JBSWY3DPEHPK3PXP&issuer=rDownloader'

function mount() {
  return render(SettingsMfaCard, { global: { plugins: [i18n], components } })
}

function enrolment(uri: string) {
  return {
    data: {
      credential_id: 'cred-1',
      provisioning_uri: uri,
      secret: 'JBSWY3DPEHPK3PXP',
      recovery_codes: ['aaaa-bbbb', 'cccc-dddd']
    }
  }
}

/// Reads the SVG back out of the image's data URI.
function decoded(image: HTMLImageElement): string {
  const prefix = 'data:image/svg+xml;charset=utf-8,'
  expect(image.src.startsWith(prefix)).toBe(true)
  return decodeURIComponent(image.src.slice(prefix.length))
}

describe('SettingsMfaCard', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockResolvedValue({
      data: { enabled: false, credentials: [], recovery_codes_remaining: 0 }
    } as never)
  })

  it('shows no code before an enrolment has been started', async () => {
    mount()

    await waitFor(() => {
      expect(screen.getByText(system.mfa.title)).toBeTruthy()
    })
    expect(screen.queryByAltText(system.mfa.enrol.qr_alt)).toBeNull()
  })

  it('renders the provisioning address as a scannable image', async () => {
    vi.mocked(api.POST).mockResolvedValue(enrolment(OTPAUTH) as never)

    mount()
    await waitFor(() => screen.getByText(system.mfa.enrol.start))
    await fireEvent.click(screen.getByText(system.mfa.enrol.start))

    const image = await waitFor(() => screen.getByAltText(system.mfa.enrol.qr_alt))
    expect(decoded(image as HTMLImageElement)).toContain('<svg')
  })

  it('derives the code from the address rather than drawing a fixed picture', async () => {
    // Without this the test above would pass just as happily on a constant image.
    vi.mocked(api.POST).mockResolvedValue(enrolment(OTPAUTH) as never)
    mount()
    await waitFor(() => screen.getByText(system.mfa.enrol.start))
    await fireEvent.click(screen.getByText(system.mfa.enrol.start))
    const first = decoded((await waitFor(() =>
      screen.getByAltText(system.mfa.enrol.qr_alt))) as HTMLImageElement)

    // The queries a render returns are bound to document.body, not to its own container, so
    // the second mount has to stand alone or both lookups answer with the first image.
    cleanup()

    vi.mocked(api.POST).mockResolvedValue(enrolment(`${OTPAUTH}&digits=8`) as never)
    mount()
    await waitFor(() => screen.getByText(system.mfa.enrol.start))
    await fireEvent.click(screen.getByText(system.mfa.enrol.start))
    const other = decoded((await waitFor(() =>
      screen.getByAltText(system.mfa.enrol.qr_alt))) as HTMLImageElement)

    expect(other).not.toEqual(first)
  })

  it('keeps the typed-in key as the way through for an app without a camera', async () => {
    vi.mocked(api.POST).mockResolvedValue(enrolment(OTPAUTH) as never)

    mount()
    await waitFor(() => screen.getByText(system.mfa.enrol.start))
    await fireEvent.click(screen.getByText(system.mfa.enrol.start))

    await waitFor(() => screen.getByAltText(system.mfa.enrol.qr_alt))
    expect(screen.getByText('JBSWY3DPEHPK3PXP')).toBeTruthy()
    expect(screen.getByText(system.mfa.enrol.manual)).toBeTruthy()
    expect(screen.getByText(system.mfa.enrol.copy_secret)).toBeTruthy()
    expect(screen.getByText(system.mfa.enrol.copy_uri)).toBeTruthy()
  })
})

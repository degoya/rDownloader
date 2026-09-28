/**
 * The setup wizard pairs the browser extension before an account needs it (RD-150-17).
 *
 * Owner, 2026-09-27: an account that wants the browser's session came up in the wizard with no
 * step before it that had paired the extension. What these hold: the pairing step says which
 * accounts need the extension, how to pair it, and shows live whether one reported in; the
 * services step names the providers that need it while none is connected and pairs it in place.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import captcha from '@/locales/en/captcha.json'
import system from '@/locales/en/system.json'
import wizard from '@/locales/en/wizard.json'
import { mountComponent } from '@/test/mount'

import { EXTENSION_POLL_MS } from '@/composables/useExtensionConnection'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer')
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
// The account and Usenet tabs are their own subject.
const empty = { default: { template: '<div />' } }
vi.mock('@/components/settings/SettingsAccountsTab.vue', () => empty)
vi.mock('@/components/settings/SettingsUsenetTab.vue', () => empty)

const { default: WizardPairingStep } = await import('./WizardPairingStep.vue')
const { default: WizardServicesStep } = await import('./WizardServicesStep.vue')

const DDOWNLOAD = { slug: 'ddownload', display_name: 'DDownload', kind: 'hoster', credentials: 'login_or_api_key', credential_modes: ['api_key', 'login'], username_required: false, device_flow: false, cookie_scope_host: 'ddownload.com' }
const RAPIDGATOR = { slug: 'rapidgator', display_name: 'Rapidgator', kind: 'hoster', credentials: 'username_password', credential_modes: [], username_required: true, device_flow: false }

let extensionConnected = false
let providers: unknown[] = []

function answer(path: string) {
  if (path === '/api/v1/captcha-answerers') return { data: { browser_extension_connected: extensionConnected } }
  if (path === '/api/v1/providers') return { data: providers }
  return { data: [] }
}

const stubs = {
  // Drawn only while open, with its body, as the real dialog is.
  UModal: {
    props: ['open', 'title'],
    emits: ['update:open'],
    template: '<div v-if="open" role="dialog" :aria-label="title"><slot name="body" /><slot name="footer" /></div>'
  },
  // The real alert draws its `actions` as buttons.
  UAlert: {
    props: ['title', 'description', 'actions'],
    template: '<div v-bind="$attrs">{{ title }}{{ description }}<button v-for="action in actions ?? []" :key="action.label" type="button" @click="action.onClick()">{{ action.label }}</button><slot /></div>'
  }
}

function mount(component: unknown) {
  return mountComponent(component as never, { messages: { captcha, system, wizard }, stubs })
}

describe('Wizard: the browser extension before the accounts', () => {
  beforeEach(() => {
    extensionConnected = false
    providers = [DDOWNLOAD, RAPIDGATOR]
    get.mockReset().mockImplementation(async (path: string) => answer(path))
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('the pairing step says which accounts need the extension and how to pair it', async () => {
    mount(WizardPairingStep)

    expect(screen.getByText(system.extension.why)).toBeTruthy()
    expect(screen.getByText(system.extension.step_install)).toBeTruthy()
    expect(screen.getByText(system.extension.step_options.replace('{origin}', window.location.origin))).toBeTruthy()
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/captcha-answerers'))
    expect(screen.getByTestId('extension-status').getAttribute('data-connected')).toBe('false')
  })

  it('the pairing step shows the extension as connected once it reports in', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    mount(WizardPairingStep)
    await waitFor(() => expect(screen.getByTestId('extension-status').textContent).toContain(system.extension.missing_title))

    extensionConnected = true
    await vi.advanceTimersByTimeAsync(EXTENSION_POLL_MS)

    await waitFor(() => expect(screen.getByTestId('extension-status').getAttribute('data-connected')).toBe('true'))
    expect(screen.getByTestId('extension-status').textContent).toContain(system.extension.connected_title)
  })

  it('the services step names the providers that need the extension and pairs it in place', async () => {
    mount(WizardServicesStep)

    const hint = await screen.findByTestId('services-extension-hint')
    expect(hint.textContent).toContain(wizard.services.extension_hint.replace('{providers}', 'DDownload'))
    expect(hint.textContent).not.toContain('Rapidgator')

    await fireEvent.click(within(hint).getByRole('button', { name: captcha.widget.extension_setup }))

    const dialog = await screen.findByRole('dialog', { name: system.extension.modal_title })
    expect(within(dialog).getByTestId('extension-pairing-guide')).toBeTruthy()
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/capture/agents'))
  })

  it('the services step drops the hint once an extension reports in', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    mount(WizardServicesStep)
    await screen.findByTestId('services-extension-hint')

    extensionConnected = true
    await vi.advanceTimersByTimeAsync(EXTENSION_POLL_MS)

    await waitFor(() => expect(screen.queryByTestId('services-extension-hint')).toBeNull())
  })

  it('the services step says nothing when no provider takes the browser session', async () => {
    providers = [RAPIDGATOR]
    mount(WizardServicesStep)

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/providers'))
    expect(screen.queryByTestId('services-extension-hint')).toBeNull()
    expect(get).not.toHaveBeenCalledWith('/api/v1/captcha-answerers')
  })
})

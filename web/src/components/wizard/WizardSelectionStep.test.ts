/**
 * "Your services" before the accounts (RD-160-05).
 *
 * Owner, 2026-09-27: the wizard asks which services the person uses and installs only their
 * plugins, and the accounts step after it offers what was installed. What these hold: the
 * default is what needs no account, an installed service is ticked and cannot be unticked, the
 * search narrows the list, "Continue" installs one service per request and resolves only when
 * all of them landed, a failure keeps the wizard on the step, and an accounts step with nothing
 * to offer says so and leads back instead of drawing an empty picker.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'

import captcha from '@/locales/en/captcha.json'
import plugins from '@/locales/en/plugins.json'
import system from '@/locales/en/system.json'
import wizard from '@/locales/en/wizard.json'
import { mountComponent } from '@/test/mount'

const listBundled = vi.fn()
const installBundled = vi.fn()
vi.mock('@/api/bundledPlugins', () => ({
  listBundled: (...args: unknown[]) => listBundled(...args),
  installBundled: (...args: unknown[]) => installBundled(...args)
}))
const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
const empty = { default: { template: '<div />' } }
vi.mock('@/components/settings/SettingsAccountsTab.vue', () => empty)
vi.mock('@/components/settings/SettingsUsenetTab.vue', () => empty)

const { default: WizardSelectionStep } = await import('./WizardSelectionStep.vue')
const { default: WizardServicesStep } = await import('./WizardServicesStep.vue')

function service(key: string, name: string, category: string, needsAccount: boolean, state = 'available') {
  return {
    key,
    name,
    description: `${name} description`,
    category,
    needs_account: needsAccount,
    provider: needsAccount ? key : null,
    state,
    plugins: [{ id: `id-${key}`, name, plugin_type: 'resolver', version: '1.0.0', installed_version: state === 'installed' ? '1.0.0' : null }]
  }
}

const CHECKSUMS = service('sha256_postprocess', 'SHA-256 checksums', 'postprocess', false)
const RAPIDGATOR = service('rapidgator', 'Rapidgator', 'hoster', true)
const MEDIAFIRE = service('mediafire', 'MediaFire', 'hoster', false, 'installed')

const stubs = {
  UAlert: {
    props: ['title', 'description', 'actions'],
    template: '<div v-bind="$attrs">{{ title }}{{ description }}<button v-for="action in actions ?? []" :key="action.label" type="button" @click="action.onClick()">{{ action.label }}</button><slot /></div>'
  }
}

/** The step inside a host that calls its exposed `install()`, as the wizard's "Continue" does. */
const Host = defineComponent({
  components: { WizardSelectionStep },
  setup() {
    const step = ref<{ install: () => Promise<boolean> } | null>(null)
    const result = ref('')
    async function run(): Promise<void> {
      result.value = String(await step.value?.install())
    }
    return { step, result, run }
  },
  template: '<div><WizardSelectionStep ref="step" /><button type="button" @click="run">continue</button><output data-testid="result">{{ result }}</output></div>'
})

function mount(component: unknown) {
  return mountComponent(component as never, { messages: { captcha, plugins, system, wizard }, stubs })
}

describe('Wizard: your services', () => {
  beforeEach(() => {
    listBundled.mockReset().mockResolvedValue({ ok: true, data: { services: [MEDIAFIRE, RAPIDGATOR, CHECKSUMS] } })
    installBundled.mockReset().mockResolvedValue({
      ok: true,
      data: { code: 'plugin.bundled_installed', message: '', installed: [{}], failed: [] }
    })
  })

  it('starts with what needs no account, and an installed service stays ticked', async () => {
    mount(WizardSelectionStep)

    const checksums = await screen.findByLabelText<HTMLInputElement>('SHA-256 checksums')
    const rapidgator = screen.getByLabelText<HTMLInputElement>('Rapidgator')
    const mediafire = screen.getByLabelText<HTMLInputElement>('MediaFire')
    expect(checksums.checked).toBe(true)
    expect(rapidgator.checked).toBe(false)
    expect(mediafire.checked).toBe(true)
    expect(mediafire.disabled).toBe(true)
    expect(screen.getByText(plugins.bundled.state.installed)).toBeTruthy()
  })

  it('narrows the list by the search', async () => {
    mount(WizardSelectionStep)
    await screen.findByLabelText('Rapidgator')

    await fireEvent.update(screen.getByTestId('bundled-search'), 'rapid')

    expect(screen.getByLabelText('Rapidgator')).toBeTruthy()
    expect(screen.queryByLabelText('SHA-256 checksums')).toBeNull()
    expect(screen.queryByLabelText('MediaFire')).toBeNull()
  })

  it('installs every ticked service, one per request, before the wizard moves on', async () => {
    mount(Host)
    await fireEvent.click(await screen.findByLabelText('Rapidgator'))

    await fireEvent.click(screen.getByRole('button', { name: 'continue' }))

    await waitFor(() => expect(screen.getByTestId('result').textContent).toBe('true'))
    expect(installBundled.mock.calls).toEqual([[['sha256_postprocess']], [['rapidgator']]])
    // Read again afterwards, so the list shows what is installed now.
    expect(listBundled).toHaveBeenCalledTimes(2)
  })

  it('stays on the step and names what failed', async () => {
    installBundled.mockResolvedValue({
      ok: true,
      data: {
        code: 'plugin.bundled_partly_installed',
        message: '',
        installed: [],
        failed: [{ service: 'sha256_postprocess', plugin_id: 'x', name: 'SHA-256 checksums', code: 'plugin.install_failed', message: 'broken' }]
      }
    })
    mount(Host)
    await screen.findByLabelText('SHA-256 checksums')

    await fireEvent.click(screen.getByRole('button', { name: 'continue' }))

    await waitFor(() => expect(screen.getByTestId('result').textContent).toBe('false'))
    expect(screen.getByTestId('selection-error').textContent).toContain('SHA-256 checksums could not be installed: broken')
  })
})

describe('Wizard: the accounts step after it', () => {
  beforeEach(() => {
    get.mockReset()
  })

  it('says there is nothing to add an account for, and leads back to the services', async () => {
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/providers'
        ? { data: [{ slug: 'xfs_generic', display_name: 'XFS', kind: 'hoster', credentials: 'none', credential_modes: [], username_required: false, device_flow: false }] }
        : { data: { browser_extension_connected: false } })
    const { emitted } = mount(WizardServicesStep)

    const hint = await screen.findByTestId('services-no-providers')
    expect(hint.textContent).toContain(wizard.services.no_providers_title)
    await fireEvent.click(screen.getByRole('button', { name: wizard.services.choose_services }))
    expect(emitted()['choose-services']).toHaveLength(1)
  })

  it('offers the accounts once a service that takes one is installed', async () => {
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/providers'
        ? { data: [{ slug: 'rapidgator', display_name: 'Rapidgator', kind: 'hoster', credentials: 'username_password', credential_modes: [], username_required: true, device_flow: false }] }
        : { data: { browser_extension_connected: false } })
    mount(WizardServicesStep)

    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/providers'))
    expect(screen.queryByTestId('services-no-providers')).toBeNull()
  })
})

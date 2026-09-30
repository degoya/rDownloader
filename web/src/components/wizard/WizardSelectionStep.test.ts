/**
 * "Your services" before the accounts (RD-160-05).
 *
 * Owner, 2026-09-27: the wizard asks which services the person uses and installs only their
 * plugins, and the accounts step after it offers what was installed. What these hold: the
 * default is what needs no account, an installed service is ticked, the search narrows the list,
 * "Continue" installs one service per request and resolves only when all of them landed, a
 * failure keeps the wizard on the step, and an accounts step with nothing to offer says so and
 * leads back instead of drawing an empty picker. Since RD-170-12 a first install runs at once;
 * what runs only after a restart, the step reports and the accounts step says.
 *
 * Owner, 2026-09-30 (RD-180-14): a fresh installation already installed every service that needs
 * no account, so the step says so, and unticking an installed service removes it on "Continue"
 * after one confirmation; a service an unfinished download still uses stays, and the step says
 * which.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'

import captcha from '@/locales/en/captcha.json'
import plugins from '@/locales/en/plugins.json'
import system from '@/locales/en/system.json'
import wizard from '@/locales/en/wizard.json'
import { useSessionStore } from '@/stores/session'
import { mountComponent } from '@/test/mount'

const listBundled = vi.fn()
const installBundled = vi.fn()
const removeBundled = vi.fn()
vi.mock('@/api/bundledPlugins', () => ({
  listBundled: (...args: unknown[]) => listBundled(...args),
  installBundled: (...args: unknown[]) => installBundled(...args),
  removeBundled: (...args: unknown[]) => removeBundled(...args)
}))
const confirm = vi.fn()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => (...args: unknown[]) => confirm(...args) }))
const push = vi.fn()
vi.mock('vue-router', () => ({ useRouter: () => ({ push: (...args: unknown[]) => push(...args) }) }))
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
    const step = ref<{ apply: () => Promise<boolean>, restartRequired: boolean } | null>(null)
    const result = ref('')
    const restart = ref('')
    async function run(): Promise<void> {
      result.value = String(await step.value?.apply())
      restart.value = String(step.value?.restartRequired)
    }
    return { step, result, restart, run }
  },
  template: '<div><WizardSelectionStep ref="step" /><button type="button" @click="run">continue</button><output data-testid="result">{{ result }}</output><output data-testid="restart">{{ restart }}</output></div>'
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
    removeBundled.mockReset().mockResolvedValue({
      ok: true,
      data: { code: 'plugin.bundled_removed', message: '', removed: ['mediafire'], failed: [] }
    })
    confirm.mockReset().mockResolvedValue(true)
    push.mockReset()
  })

  it('starts with what needs no account, and an installed service is ticked and can be unticked', async () => {
    mount(WizardSelectionStep)

    const checksums = await screen.findByLabelText<HTMLInputElement>('SHA-256 checksums')
    const rapidgator = screen.getByLabelText<HTMLInputElement>('Rapidgator')
    const mediafire = screen.getByLabelText<HTMLInputElement>('MediaFire')
    expect(checksums.checked).toBe(true)
    expect(rapidgator.checked).toBe(false)
    expect(mediafire.checked).toBe(true)
    expect(mediafire.disabled).toBe(false)
    expect(screen.getByText(plugins.bundled.state.installed)).toBeTruthy()
  })

  it('says what is installed already and where to change it later (RD-180-14)', async () => {
    mount(WizardSelectionStep)

    const note = await screen.findByTestId('selection-installed-note')
    expect(note.textContent).toContain(wizard.selection.installed_note_title)
    expect(note.textContent).toContain(wizard.selection.installed_note)
    // A first run has to finish before the plugin manager can open, so there is no way out.
    expect(screen.queryByRole('button', { name: wizard.selection.open_plugins })).toBeNull()
  })

  it('leads a wizard run from the settings to the plugin manager', async () => {
    mount(WizardSelectionStep)
    const session = useSessionStore()
    session.wizardRerun = true
    session.wizardActive = true

    await fireEvent.click(await screen.findByRole('button', { name: wizard.selection.open_plugins }))

    expect(push).toHaveBeenCalledWith({ name: 'settings', params: { section: 'plugins' } })
    expect(session.wizardActive).toBe(false)
  })

  it('removes an unticked installed service after one confirmation, and installs the rest', async () => {
    listBundled
      .mockResolvedValueOnce({ ok: true, data: { services: [MEDIAFIRE, RAPIDGATOR, CHECKSUMS] } })
      .mockResolvedValue({ ok: true, data: { services: [{ ...MEDIAFIRE, state: 'available' }, RAPIDGATOR, CHECKSUMS] } })
    mount(Host)
    await fireEvent.click(await screen.findByLabelText('MediaFire'))
    expect(screen.getByTestId('selection-remove-hint').textContent).toContain('Continue removes 1 service.')

    await fireEvent.click(screen.getByRole('button', { name: 'continue' }))

    await waitFor(() => expect(screen.getByTestId('result').textContent).toBe('true'))
    expect(confirm).toHaveBeenCalledTimes(1)
    const [options] = confirm.mock.calls[0] as [{ description: string, destructive: boolean }]
    expect(options.description).toContain('MediaFire is removed with all of its plugins and stops running at the next start.')
    expect(options.destructive).toBe(true)
    expect(removeBundled.mock.calls).toEqual([[['mediafire']]])
    expect(installBundled.mock.calls).toEqual([[['sha256_postprocess']]])
    expect(screen.getByLabelText<HTMLInputElement>('MediaFire').checked).toBe(false)
  })

  it('changes nothing when the removal is not confirmed', async () => {
    confirm.mockResolvedValue(false)
    mount(Host)
    await fireEvent.click(await screen.findByLabelText('MediaFire'))

    await fireEvent.click(screen.getByRole('button', { name: 'continue' }))

    await waitFor(() => expect(screen.getByTestId('result').textContent).toBe('false'))
    expect(removeBundled).not.toHaveBeenCalled()
    expect(installBundled).not.toHaveBeenCalled()
  })

  it('names a service an unfinished download keeps, and ticks it again', async () => {
    removeBundled.mockResolvedValue({
      ok: true,
      data: {
        code: 'plugin.bundled_partly_removed',
        message: '',
        removed: [],
        failed: [{ service: 'mediafire', plugin_id: 'id-mediafire', name: 'MediaFire', code: 'plugin.version_in_use', message: 'in use' }]
      }
    })
    mount(Host)
    await fireEvent.click(await screen.findByLabelText('MediaFire'))

    await fireEvent.click(screen.getByRole('button', { name: 'continue' }))

    await waitFor(() => expect(screen.getByTestId('result').textContent).toBe('false'))
    expect(screen.getByTestId('selection-error').textContent).toContain('MediaFire stays installed: an unfinished download still uses it.')
    expect(screen.getByLabelText<HTMLInputElement>('MediaFire').checked).toBe(true)
    // The rest went ahead.
    expect(installBundled.mock.calls).toEqual([[['sha256_postprocess']]])
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
    // Both run at once: nothing waits for a restart.
    expect(screen.getByTestId('restart').textContent).toBe('false')
  })

  it('reports a plugin that runs only after a restart (RD-170-12)', async () => {
    installBundled
      .mockResolvedValueOnce({ ok: true, data: { code: 'plugin.bundled_installed', message: '', installed: [{}], failed: [], restart_required: false } })
      .mockResolvedValueOnce({
        ok: true,
        data: { code: 'plugin.bundled_installed_restart_required', message: '', installed: [{}], failed: [], restart_required: true }
      })
    mount(Host)
    await fireEvent.click(await screen.findByLabelText('Rapidgator'))

    await fireEvent.click(screen.getByRole('button', { name: 'continue' }))

    await waitFor(() => expect(screen.getByTestId('result').textContent).toBe('true'))
    expect(screen.getByTestId('restart').textContent).toBe('true')
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
    expect(screen.queryByTestId('services-restart-required')).toBeNull()
  })

  it('says when a plugin just installed runs only after a restart (RD-170-12)', async () => {
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/providers'
        ? { data: [{ slug: 'ddownload', display_name: 'DDownload', kind: 'hoster', credentials: 'login_or_api_key', credential_modes: [], username_required: false, device_flow: false }] }
        : { data: { browser_extension_connected: false } })
    mountComponent(WizardServicesStep as never, { messages: { captcha, plugins, system, wizard }, stubs, props: { restartRequired: true } })

    const notice = await screen.findByTestId('services-restart-required')
    expect(notice.textContent).toContain(wizard.services.restart_required_title)
    expect(notice.textContent).toContain(wizard.services.restart_required)
  })
})

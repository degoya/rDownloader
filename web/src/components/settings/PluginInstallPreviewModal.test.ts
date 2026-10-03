/**
 * The install preview (RD-140-01): publisher, key standing, permissions and release notes before
 * anything is installed, and a key nobody confirmed yet confirmed by the fingerprint shown here.
 * Its layout (RD-180-22): the publisher's key and the package digest fold away, the restart hint
 * stands in the footer beside the two actions.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { PluginPreview, PreviewSource } from '@/api/pluginRepositories'
import pluginsCatalogue from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

const preview = vi.fn()
const install = vi.fn()
vi.mock('@/api/pluginRepositories', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/pluginRepositories')>(),
  preview: (...args: unknown[]) => preview(...args),
  install: (...args: unknown[]) => install(...args)
}))
vi.mock('@/i18n', () => ({ currentLocale: () => 'en' }))
const loadPluginMessages = vi.fn(async () => {})
vi.mock('@/i18n/plugins', () => ({
  loadPluginMessages: () => loadPluginMessages(),
  resetPluginMessages: vi.fn()
}))
vi.mock('@/i18n/server', () => ({
  serverMessageFrom: (body: unknown) => body,
  translateServerMessage: (message: { code?: string } | null) => message?.code ?? 'request failed'
}))

const { default: PluginInstallPreviewModal } = await import('./PluginInstallPreviewModal.vue')

const FINGERPRINT = 'ab'.repeat(32)
const GROUPED = 'abababab abababab abababab abababab abababab abababab abababab abababab'
const SOURCE: PreviewSource = {
  kind: 'repository',
  repositoryId: 'official',
  pluginId: '019d0000-0000-7000-8000-0000000140aa',
  version: '1.2.4'
}

function shown(overrides: Partial<PluginPreview> = {}): PluginPreview {
  return {
    plugin_id: SOURCE.kind === 'repository' ? SOURCE.pluginId : '',
    name: 'DDownload',
    version: '1.2.4',
    plugin_type: 'resolver',
    api_version: '0.10.0',
    min_app_version: null,
    description: 'Resolves DDownload links.',
    homepage: null,
    license: null,
    package_digest: 'cd'.repeat(32),
    size: 4096,
    publisher: { key_id: 'rdownloader-release-v1', fingerprint: FINGERPRINT, author: 'rDownloader' },
    permissions: { granted: ['net_http', 'captcha'], http_domains: ['ddownload.com'], stream_hosts: [] },
    key_status: 'trusted',
    withdrawn: false,
    incompatible: null,
    installable: true,
    installed_versions: ['1.2.3'],
    added_permissions: { installed_version: '1.2.3', permissions: { granted: [], http_domains: [], stream_hosts: [] } },
    source: { repository_id: 'official', repository_name: 'rDownloader', official: true },
    release_notes: '<b>Fixes</b> the countdown.\nNo new permissions.',
    ...overrides
  }
}

/** `UModal` keeps its content in named slots, which the shared passthrough stub drops. */
const MODAL_STUB = { UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' } }

function mount(source: PreviewSource | null = SOURCE) {
  return mountComponent(PluginInstallPreviewModal, {
    messages: { plugins: pluginsCatalogue },
    props: { source },
    stubs: MODAL_STUB
  })
}

function publisherToggle(): HTMLElement {
  return within(document.querySelector('[data-preview-publisher]') as HTMLElement).getByRole('button')
}

function digestToggle(): HTMLElement {
  return within(document.querySelector('[data-preview-digest]') as HTMLElement).getByRole('button')
}

describe('PluginInstallPreviewModal', () => {
  beforeEach(() => {
    preview.mockReset()
    install.mockReset()
    install.mockResolvedValue({ ok: true, data: { code: 'plugin.installed_restart_required' } })
    loadPluginMessages.mockClear()
  })

  it('shows publisher, permissions and release notes before anything is installed', async () => {
    preview.mockResolvedValue({ ok: true, data: shown() })
    mount()
    await screen.findByText('DDownload')
    expect(preview).toHaveBeenCalledWith(SOURCE)
    // A trusted key keeps its fingerprint folded behind the trust line until somebody asks.
    expect(screen.queryByText(GROUPED)).toBeNull()
    await fireEvent.click(publisherToggle())
    expect(screen.getByText(GROUPED)).toBeTruthy()
    expect(screen.getByText('rdownloader-release-v1')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.capability.net_http)).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.capability.captcha)).toBeTruthy()
    expect(screen.getByText('ddownload.com')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.preview.key.trusted)).toBeTruthy()
    // Release notes are text: markup in them is shown, never rendered.
    expect(screen.getByText(/<b>Fixes<\/b> the countdown\./)).toBeTruthy()
    expect(document.querySelector('b')).toBeNull()
    expect(install).not.toHaveBeenCalled()
  })

  it('puts what an update adds over the installed version first (RD-160-09)', async () => {
    preview.mockResolvedValue({
      ok: true,
      data: shown({
        added_permissions: {
          installed_version: '1.2.3',
          permissions: { granted: ['captcha'], http_domains: ['api.ddownload.com'], stream_hosts: [] }
        }
      })
    })
    mount()
    await screen.findByText('DDownload')
    const added = document.querySelector('[data-preview-added-permissions]')
    expect(added?.textContent).toContain(pluginsCatalogue.preview.added_permissions.replace('{version}', '1.2.3'))
    expect(added?.textContent).toContain(pluginsCatalogue.capability.captcha)
    expect(added?.textContent).toContain('api.ddownload.com')
    // Only the new ones: net_http and ddownload.com were granted to 1.2.3 already.
    expect(added?.textContent).not.toContain(pluginsCatalogue.capability.net_http)
    expect(added?.textContent?.match(/ddownload\.com/g)).toHaveLength(1)
  })

  it('says so when an update asks for nothing new, and shows no comparison for a first install', async () => {
    preview.mockResolvedValue({ ok: true, data: shown() })
    const first = mount()
    await screen.findByText('DDownload')
    expect(document.querySelector('[data-preview-added-permissions]')?.textContent).toContain(
      pluginsCatalogue.preview.no_added_permissions.replace('{version}', '1.2.3')
    )
    first.unmount()

    preview.mockResolvedValue({ ok: true, data: shown({ installed_versions: [], added_permissions: null }) })
    mount()
    await screen.findByText('DDownload')
    expect(document.querySelector('[data-preview-added-permissions]')).toBeNull()
  })

  it('installs a trusted package without confirming a key', async () => {
    preview.mockResolvedValue({ ok: true, data: shown() })
    const { emitted } = mount()
    await fireEvent.click(await screen.findByRole('button', { name: pluginsCatalogue.preview.install }))
    await waitFor(() => expect(install).toHaveBeenCalledTimes(1))
    expect(install).toHaveBeenCalledWith(SOURCE, undefined)
    await waitFor(() => expect(emitted().installed).toEqual([['plugin.installed_restart_required']]))
    expect(loadPluginMessages).toHaveBeenCalled()
  })

  it('confirms an unknown key with exactly the fingerprint it shows', async () => {
    preview.mockResolvedValue({ ok: true, data: shown({ key_status: 'untrusted' }) })
    mount()
    expect(await screen.findByText(pluginsCatalogue.trust.warning)).toBeTruthy()
    // What installing confirms is in view from the start, not behind a click.
    expect(publisherToggle().getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByText(GROUPED)).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.trust.confirm }))
    await waitFor(() => expect(install).toHaveBeenCalledWith(SOURCE, FINGERPRINT))
  })

  it('shows the digest by its two ends and unfolds the whole of it', async () => {
    preview.mockResolvedValue({ ok: true, data: shown() })
    mount()
    await screen.findByText('DDownload')
    const full = 'cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd'
    expect(screen.getByText('cdcdcdcd … cdcdcdcd')).toBeTruthy()
    expect(screen.queryByText(full)).toBeNull()
    expect(digestToggle().getAttribute('aria-expanded')).toBe('false')

    await fireEvent.click(digestToggle())
    // Four groups a line, two lines, as it is compared by eye.
    expect(screen.getByText(full).textContent?.split('\n')).toEqual([
      'cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd',
      'cdcdcdcd cdcdcdcd cdcdcdcd cdcdcdcd'
    ])
    expect(screen.queryByText('cdcdcdcd … cdcdcdcd')).toBeNull()
  })

  it('names the source beside the name for a new plugin and beside the installed versions for an update', async () => {
    preview.mockResolvedValue({ ok: true, data: shown() })
    const update = mount()
    await screen.findByText('DDownload')
    expect(document.querySelector('[data-preview-source]')?.textContent).toBe('From rDownloader·Installed here: 1.2.3')
    update.unmount()

    preview.mockResolvedValue({ ok: true, data: shown({ installed_versions: [], added_permissions: null }) })
    mount()
    await screen.findByText('DDownload')
    expect(document.querySelector('[data-preview-source]')?.textContent).toBe('From rDownloader')
  })

  it('ends with the restart hint beside Cancel and Install, and Cancel closes', async () => {
    preview.mockResolvedValue({ ok: true, data: shown() })
    const { emitted } = mount()
    await screen.findByText('DDownload')
    expect(screen.getByText(pluginsCatalogue.preview.restart)).toBeTruthy()
    const buttons = screen.getAllByRole('button').map(button => button.textContent)
    expect(buttons.slice(-2)).toEqual([pluginsCatalogue.trust.cancel, pluginsCatalogue.preview.install])
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.trust.cancel }))
    expect(emitted().close).toHaveLength(1)
  })

  it('offers no install for a withdrawn package', async () => {
    preview.mockResolvedValue({ ok: true, data: shown({ withdrawn: true, installable: false }) })
    mount()
    expect(await screen.findByText(pluginsCatalogue.preview.withdrawn)).toBeTruthy()
    const button = screen.getByRole('button', { name: pluginsCatalogue.preview.install }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
  })

  it('shows the service’s refusal instead of a preview', async () => {
    preview.mockResolvedValue({ ok: false, status: 502, message: { code: 'plugin_repository.digest_mismatch' } })
    mount()
    expect(await screen.findByText('plugin_repository.digest_mismatch')).toBeTruthy()
    expect(screen.queryByText('DDownload')).toBeNull()
  })

  it('stays empty while closed', () => {
    mount(null)
    expect(preview).not.toHaveBeenCalled()
    expect(screen.queryByRole('button')).toBeNull()
  })
})

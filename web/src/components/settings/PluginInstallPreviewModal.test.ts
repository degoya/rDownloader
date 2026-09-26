/**
 * The install preview (RD-140-01): publisher, key standing, permissions and release notes before
 * anything is installed, and a key nobody confirmed yet confirmed by the fingerprint shown here.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
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
    api_version: '0.9.0',
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
    expect(screen.getByText('abababab abababab abababab abababab abababab abababab abababab abababab')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.capability.net_http)).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.capability.captcha)).toBeTruthy()
    expect(screen.getByText('ddownload.com')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.preview.key.trusted)).toBeTruthy()
    // Release notes are text: markup in them is shown, never rendered.
    expect(screen.getByText(/<b>Fixes<\/b> the countdown\./)).toBeTruthy()
    expect(document.querySelector('b')).toBeNull()
    expect(install).not.toHaveBeenCalled()
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
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.trust.confirm }))
    await waitFor(() => expect(install).toHaveBeenCalledWith(SOURCE, FINGERPRINT))
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

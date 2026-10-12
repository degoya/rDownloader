/**
 * The plugin manager's one view of the plugins (RD-1260-02): the bundle's services and what only a
 * repository offers, installed, available or with an update waiting, each with its origin. A
 * bundled service installs on a click; an update and a repository's own plugin start at the
 * preview. Above the list, the switch for all plugins' updates (RD-191-10).
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { PluginUpdate } from '@/api/pluginRepositories'
import plugins from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

const listBundled = vi.fn()
const installBundled = vi.fn()
vi.mock('@/api/bundledPlugins', () => ({
  listBundled: (...args: unknown[]) => listBundled(...args),
  installBundled: (...args: unknown[]) => installBundled(...args),
  removeBundled: vi.fn()
}))
const getUpdateSettings = vi.fn()
const setUpdateSettings = vi.fn()
vi.mock('@/api/pluginRepositories', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/pluginRepositories')>(),
  getUpdateSettings: () => getUpdateSettings(),
  setUpdateSettings: (automatic: boolean) => setUpdateSettings(automatic)
}))
vi.mock('@/i18n/server', () => ({
  serverMessageFrom: (body: unknown) => body,
  translateServerMessage: (message: { code?: string } | null) => message?.code ?? 'request failed'
}))

const { default: PluginCatalogue } = await import('./PluginCatalogue.vue')

const NONE = { granted: [], http_domains: [], stream_hosts: [] }

function update(name: string, from: string, to: string, overrides: Partial<PluginUpdate> = {}): PluginUpdate {
  return {
    offer: {
      repository_id: 'official',
      repository_name: 'rDownloader',
      official: true,
      package: {
        plugin_id: `id-${name}`,
        name,
        version: to,
        plugin_type: 'resolver',
        api_version: '0.11.0',
        min_app_version: null,
        package_digest: 'cd'.repeat(32),
        size: 4096,
        publisher: { key_id: 'rdownloader-release-v1', fingerprint: 'ab'.repeat(32), author: 'rDownloader' },
        permissions: NONE,
        release_notes: null
      },
      compatibility: 'compatible',
      installed_version: from
    },
    installed_version: from,
    policy: 'manual',
    adds_permissions: false,
    added_permissions: NONE,
    ...overrides
  }
}

interface PluginShape {
  origin: string
  installed_origin?: string | null
  installed_version?: string | null
  repository_id?: string | null
  repository_version?: string | null
  update?: PluginUpdate | null
}

function service(key: string, name: string, state: string, plugin: PluginShape) {
  return {
    key,
    name,
    description: `${name} description`,
    category: 'hoster',
    needs_account: false,
    provider: key,
    state,
    update_available: Boolean(plugin.update),
    plugins: [{ id: `id-${name}`, name, plugin_type: 'resolver', version: '1.0.0', installed_version: null, ...plugin }]
  }
}

/** The preview reduced to what the catalogue hands it. */
const previewSources: unknown[] = []
const PREVIEW_STUB = {
  PluginInstallPreviewModal: {
    props: ['source'],
    watch: { source(value: unknown) { if (value) previewSources.push(value) } },
    template: '<div data-preview-stub />'
  }
}

function serve(services: unknown[], repositoryServices: unknown[] = []): void {
  listBundled.mockResolvedValue({ ok: true, data: { services, repository_services: repositoryServices } })
}

function mount() {
  return mountComponent(PluginCatalogue, { messages: { plugins }, stubs: PREVIEW_STUB })
}

const RAPIDGATOR = service('rapidgator', 'Rapidgator', 'available', { origin: 'repository', repository_id: 'official', repository_version: '1.0.1' })
const MEDIAFIRE = service('mediafire', 'MediaFire', 'installed', { origin: 'bundled', installed_origin: 'bundled', installed_version: '1.0.0' })
const DDOWNLOAD = service('ddownload', 'DDownload', 'installed', {
  origin: 'repository',
  installed_origin: 'repository',
  installed_version: '1.2.3',
  update: update('DDownload', '1.2.3', '1.2.4', { policy: 'automatic' })
})
const ONLY = service('id-only', 'Repository only', 'available', { origin: 'repository', repository_id: 'official', repository_version: '2.0.0' })

describe('PluginCatalogue', () => {
  beforeEach(() => {
    listBundled.mockReset()
    installBundled.mockReset().mockResolvedValue({
      ok: true,
      data: { code: 'plugin.bundled_installed', message: '', installed: [{ id: 'id-Rapidgator' }], failed: [], restart_required: false }
    })
    getUpdateSettings.mockReset().mockResolvedValue({ ok: true, data: { automatic_updates: false } })
    setUpdateSettings.mockReset()
    previewSources.length = 0
  })

  it('lists installed and available services in one view, each with its origin', async () => {
    serve([RAPIDGATOR, MEDIAFIRE, DDOWNLOAD], [ONLY])
    mount()
    const installed = await screen.findByTestId('bundled-service-mediafire')
    expect(within(installed).getByText(plugins.bundled.state.installed)).toBeTruthy()
    expect(installed.querySelector('[data-origin="bundled"]')?.textContent?.trim()).toBe(plugins.catalogue.origin.bundled)
    const available = screen.getByTestId('bundled-service-rapidgator')
    expect(available.querySelector('[data-origin="repository"]')?.textContent?.trim()).toBe(plugins.catalogue.origin.repository)
    expect(screen.getByTestId('bundled-service-id-only')).toBeTruthy()
  })

  it('installs a bundled service with one click and never without one', async () => {
    serve([RAPIDGATOR, MEDIAFIRE])
    const { emitted } = mount()
    const row = await screen.findByTestId('bundled-service-rapidgator')
    expect(installBundled).not.toHaveBeenCalled()
    await fireEvent.click(within(row).getByRole('button', { name: plugins.bundled.install }))
    await waitFor(() => expect(emitted().installed).toHaveLength(1))
    expect(installBundled).toHaveBeenCalledWith(['rapidgator'])
    expect(emitted().installed?.[0]).toEqual(['1 plugin installed; it runs now.'])
    // An installed service offers no install.
    expect(within(screen.getByTestId('bundled-service-mediafire')).queryByRole('button')).toBeNull()
  })

  it('opens the preview for an update, names both versions and marks an automatic one', async () => {
    serve([DDOWNLOAD])
    mount()
    const row = await screen.findByTestId('bundled-service-ddownload')
    expect(within(row).getByText('v1.2.3 → v1.2.4')).toBeTruthy()
    expect(within(row).getByText(plugins.catalogue.state.update)).toBeTruthy()
    expect(within(row).getByText(plugins.updates.automatic)).toBeTruthy()
    await fireEvent.click(within(row).getByRole('button', { name: plugins.updates.review }))
    expect(previewSources).toEqual([{ kind: 'repository', repositoryId: 'official', pluginId: 'id-DDownload', version: '1.2.4' }])
    expect(installBundled).not.toHaveBeenCalled()
  })

  it('marks an update that asks for new permissions', async () => {
    const wider = service('wider', 'Wider', 'installed', {
      origin: 'repository',
      installed_version: '1.0.0',
      update: update('Wider', '1.0.0', '1.1.0', {
        adds_permissions: true,
        added_permissions: { granted: [], http_domains: ['api.wider.test'], stream_hosts: [] }
      })
    })
    serve([wider])
    mount()
    const badge = await screen.findByText(plugins.updates.adds_permissions)
    expect(badge.closest('[data-adds-permissions]')?.getAttribute('title')).toContain('api.wider.test')
  })

  it('installs what only a repository offers through the preview', async () => {
    serve([], [ONLY])
    mount()
    const row = await screen.findByTestId('bundled-service-id-only')
    expect(within(row).queryByRole('button', { name: plugins.bundled.install })).toBeNull()
    await fireEvent.click(within(row).getByRole('button', { name: plugins.updates.review_package }))
    expect(previewSources).toEqual([{ kind: 'repository', repositoryId: 'official', pluginId: 'id-Repository only', version: '2.0.0' }])
  })

  it('shows only the services with an update when that filter is chosen', async () => {
    serve([RAPIDGATOR, MEDIAFIRE, DDOWNLOAD])
    mount()
    await screen.findByTestId('bundled-service-rapidgator')
    const filter = screen.getByRole('group', { name: plugins.catalogue.show_label })
    await fireEvent.click(within(filter).getByRole('radio', { name: `${plugins.catalogue.show.updates} 1` }))
    await waitFor(() => expect(screen.queryByTestId('bundled-service-rapidgator')).toBeNull())
    expect(screen.getByTestId('bundled-service-ddownload')).toBeTruthy()
  })

  it('reads the switch for all plugins, switches it on and tells the tab', async () => {
    serve([MEDIAFIRE])
    setUpdateSettings.mockResolvedValue({ ok: true, data: { automatic_updates: true } })
    const view = mount()
    const toggle = await screen.findByRole('switch', { name: plugins.updates.automatic_all })
    await vi.waitFor(() => expect((toggle as HTMLButtonElement).disabled).toBe(false))
    await fireEvent.click(toggle)
    expect(setUpdateSettings).toHaveBeenCalledWith(true)
    expect(await screen.findByText(plugins.updates.automatic_all_on)).toBeTruthy()
    expect(view.emitted('automaticChanged')).toEqual([[true]])
  })

  it('keeps the switch disabled and says why when it cannot be read (RA-WEB-03)', async () => {
    serve([MEDIAFIRE])
    getUpdateSettings.mockResolvedValue({ ok: false, status: 500, message: { code: 'internal.error' } })
    const view = mount()
    expect(await screen.findByText('internal.error')).toBeTruthy()
    expect(view.container.querySelector('[data-automatic-all-error]')).not.toBeNull()
    const toggle = screen.getByRole('switch', { name: plugins.updates.automatic_all }) as HTMLButtonElement
    expect(toggle.disabled).toBe(true)
  })
})

/**
 * The updates list (RD-140-01): an update names both versions and its repository, an automatic
 * one says so, and every install starts at the preview rather than at the service. Above it, the
 * switch for all plugins (RD-191-10).
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { PluginOffer, PluginOffers } from '@/api/pluginRepositories'
import pluginsCatalogue from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

const listOffers = vi.fn()
const getUpdateSettings = vi.fn()
const setUpdateSettings = vi.fn()
vi.mock('@/api/pluginRepositories', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/pluginRepositories')>(),
  listOffers: () => listOffers(),
  getUpdateSettings: () => getUpdateSettings(),
  setUpdateSettings: (automatic: boolean) => setUpdateSettings(automatic)
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/i18n/server', () => ({
  serverMessageFrom: (body: unknown) => body,
  translateServerMessage: (message: { code?: string } | null) => message?.code ?? 'request failed'
}))

const { default: PluginUpdatesList } = await import('./PluginUpdatesList.vue')

const NONE = { granted: [], http_domains: [], stream_hosts: [] }

function offer(name: string, version: string, overrides: Partial<PluginOffer> = {}): PluginOffer {
  return {
    repository_id: 'official',
    repository_name: 'rDownloader',
    official: true,
    package: {
      plugin_id: `id-${name}`,
      name,
      version,
      plugin_type: 'resolver',
      api_version: '0.10.0',
      min_app_version: null,
      package_digest: 'cd'.repeat(32),
      size: 4096,
      publisher: { key_id: 'rdownloader-release-v1', fingerprint: 'ab'.repeat(32), author: 'rDownloader' },
      permissions: { granted: [], http_domains: [], stream_hosts: [] },
      release_notes: null
    },
    compatibility: 'compatible',
    installed_version: null,
    ...overrides
  }
}

/** The preview reduced to what the list hands it. */
const previewSources: unknown[] = []
const PREVIEW_STUB = {
  PluginInstallPreviewModal: {
    props: ['source'],
    watch: { source(value: unknown) { if (value) previewSources.push(value) } },
    template: '<div data-preview-stub />'
  }
}

function mount(offers: PluginOffers) {
  listOffers.mockResolvedValue({ ok: true, data: offers })
  return mountComponent(PluginUpdatesList, {
    messages: { plugins: pluginsCatalogue },
    stubs: PREVIEW_STUB
  })
}

describe('PluginUpdatesList', () => {
  beforeEach(() => {
    listOffers.mockReset()
    getUpdateSettings.mockReset()
    getUpdateSettings.mockResolvedValue({ ok: true, data: { automatic_updates: false } })
    setUpdateSettings.mockReset()
    previewSources.length = 0
  })

  it('reads the switch for all plugins, switches it on and tells the tab', async () => {
    setUpdateSettings.mockResolvedValue({ ok: true, data: { automatic_updates: true } })
    const view = mount({ updates: [], available: [], installed: [] })
    const toggle = await screen.findByRole('switch', { name: pluginsCatalogue.updates.automatic_all })
    await vi.waitFor(() => expect((toggle as HTMLButtonElement).disabled).toBe(false))
    expect(toggle.getAttribute('aria-checked')).toBe('false')

    getUpdateSettings.mockResolvedValue({ ok: true, data: { automatic_updates: true } })
    await fireEvent.click(toggle)
    expect(setUpdateSettings).toHaveBeenCalledWith(true)
    expect(await screen.findByText(pluginsCatalogue.updates.automatic_all_on)).toBeTruthy()
    expect(view.emitted('automaticChanged')).toEqual([[true]])
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'))
  })

  it('shows the switch on as the service reports it, and a refusal to switch it off', async () => {
    getUpdateSettings.mockResolvedValue({ ok: true, data: { automatic_updates: true } })
    setUpdateSettings.mockResolvedValue({ ok: false, status: 403, message: { code: 'auth.forbidden' } })
    const view = mount({ updates: [], available: [], installed: [] })
    const toggle = await screen.findByRole('switch', { name: pluginsCatalogue.updates.automatic_all })
    await vi.waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'))

    await fireEvent.click(toggle)
    expect(setUpdateSettings).toHaveBeenCalledWith(false)
    expect(await screen.findByText('auth.forbidden')).toBeTruthy()
    expect(view.emitted('automaticChanged')).toBeUndefined()
    expect(toggle.getAttribute('aria-checked')).toBe('true')
  })

  it('keeps the switch disabled until the service reported it (RA-WEB-03)', async () => {
    let answer: (value: unknown) => void = () => {}
    getUpdateSettings.mockReturnValue(new Promise(resolve => { answer = resolve }))
    mount({ updates: [], available: [], installed: [] })
    const toggle = await screen.findByRole('switch', { name: pluginsCatalogue.updates.automatic_all }) as HTMLButtonElement
    expect(toggle.disabled).toBe(true)
    await fireEvent.click(toggle)
    expect(setUpdateSettings).not.toHaveBeenCalled()

    answer({ ok: true, data: { automatic_updates: true } })
    await vi.waitFor(() => expect(toggle.disabled).toBe(false))
    expect(toggle.getAttribute('aria-checked')).toBe('true')
  })

  it('shows why the switch cannot be read, and leaves it disabled (RA-WEB-03)', async () => {
    getUpdateSettings.mockResolvedValue({ ok: false, status: 500, message: { code: 'internal.error' } })
    const view = mount({ updates: [], available: [], installed: [] })
    expect(await screen.findByText('internal.error')).toBeTruthy()
    expect(view.container.querySelector('[data-automatic-all-error]')).not.toBeNull()
    const toggle = screen.getByRole('switch', { name: pluginsCatalogue.updates.automatic_all }) as HTMLButtonElement
    expect(toggle.disabled).toBe(true)
  })

  it('names both versions and the repository, and marks an automatic update', async () => {
    mount({
      updates: [
        { offer: offer('DDownload', '1.2.4', { installed_version: '1.2.3' }), installed_version: '1.2.3', policy: 'automatic', adds_permissions: false, added_permissions: NONE },
        { offer: offer('Rapidgator', '2.0.0', { installed_version: '1.9.0' }), installed_version: '1.9.0', policy: 'manual', adds_permissions: false, added_permissions: NONE }
      ],
      available: [], installed: []
    })
    expect(await screen.findByText('v1.2.3 → v1.2.4')).toBeTruthy()
    expect(screen.getByText('v1.9.0 → v2.0.0')).toBeTruthy()
    expect(screen.getAllByText(pluginsCatalogue.updates.automatic)).toHaveLength(1)
    expect(screen.getAllByText('from rDownloader')).toHaveLength(2)
  })

  it('marks an update that asks for new permissions, automatic or not', async () => {
    mount({
      updates: [
        {
          offer: offer('DDownload', '1.2.4'),
          installed_version: '1.2.3',
          policy: 'automatic',
          adds_permissions: true,
          added_permissions: { granted: ['captcha'], http_domains: ['api.ddownload.com'], stream_hosts: [] }
        },
        { offer: offer('Rapidgator', '2.0.0'), installed_version: '1.9.0', policy: 'manual', adds_permissions: false, added_permissions: NONE }
      ],
      available: [], installed: []
    })
    const badge = await screen.findByText(pluginsCatalogue.updates.adds_permissions)
    // The tooltip names what is new, translated grants first, then the addresses (RD-160-09).
    const added = pluginsCatalogue.updates.added_permissions.replace(
      '{permissions}',
      `${pluginsCatalogue.capability.captcha}, api.ddownload.com`
    )
    expect(badge.closest('[data-adds-permissions]')?.getAttribute('title')).toBe(
      `${pluginsCatalogue.updates.adds_permissions_hint}\n${added}`
    )
    expect(screen.getAllByText(pluginsCatalogue.updates.adds_permissions)).toHaveLength(1)
  })

  it('opens the preview for the exact version, and never installs straight away', async () => {
    mount({ updates: [{ offer: offer('DDownload', '1.2.4'), installed_version: '1.2.3', policy: 'manual', adds_permissions: false, added_permissions: NONE }], available: [], installed: [] })
    await fireEvent.click(await screen.findByRole('button', { name: pluginsCatalogue.updates.review }))
    expect(previewSources).toEqual([
      { kind: 'repository', repositoryId: 'official', pluginId: 'id-DDownload', version: '1.2.4' }
    ])
  })

  it('lists what is not installed, and offers no review for what this build cannot run', async () => {
    mount({
      updates: [],
      available: [offer('Fresh', '1.0.0'), offer('Future', '3.0.0', { compatibility: 'contract_unsupported' })],
      installed: []
    })
    expect(await screen.findByText(pluginsCatalogue.updates.empty)).toBeTruthy()
    expect(screen.getByText('Fresh')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.updates.compatibility.contract_unsupported)).toBeTruthy()
    const buttons = screen.getAllByRole('button', { name: pluginsCatalogue.updates.review_package }) as HTMLButtonElement[]
    expect(buttons.map(button => button.disabled)).toEqual([false, true])
  })

  it('shows the refusal when the list cannot be read', async () => {
    listOffers.mockResolvedValue({ ok: false, status: 500, message: { code: 'internal.error' } })
    mountComponent(PluginUpdatesList, { messages: { plugins: pluginsCatalogue }, stubs: PREVIEW_STUB })
    expect(await screen.findByText('internal.error')).toBeTruthy()
  })
})

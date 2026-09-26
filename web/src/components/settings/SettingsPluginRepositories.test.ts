/**
 * The repository list (RD-140-01): the official repository cannot be removed, a third-party one
 * is added only after its key is approved by the fingerprint shown, and switching one off is a
 * `PATCH`, never a deletion.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { PluginRepository } from '@/api/pluginRepositories'
import pluginsCatalogue from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

const listRepositories = vi.fn()
const addRepository = vi.fn()
const updateRepository = vi.fn()
const removeRepository = vi.fn()
const refreshRepositories = vi.fn()
vi.mock('@/api/pluginRepositories', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/pluginRepositories')>(),
  listRepositories: () => listRepositories(),
  addRepository: (...args: unknown[]) => addRepository(...args),
  updateRepository: (...args: unknown[]) => updateRepository(...args),
  removeRepository: (...args: unknown[]) => removeRepository(...args),
  refreshRepositories: () => refreshRepositories(),
  setRefreshHours: vi.fn()
}))
const confirmed = vi.fn<(options: Record<string, unknown>) => Promise<boolean>>()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/i18n', () => ({
  i18n: { global: { d: (value: Date) => value.toISOString(), t: (key: string) => key } }
}))
vi.mock('@/i18n/server', () => ({
  serverMessageFrom: (body: unknown) => body,
  translateServerMessage: (message: { code?: string } | null) => message?.code ?? 'request failed'
}))

const { default: SettingsPluginRepositories } = await import('./SettingsPluginRepositories.vue')

const FINGERPRINT = 'ef'.repeat(32)

function repository(overrides: Partial<PluginRepository> = {}): PluginRepository {
  return {
    id: 'official',
    kind: 'official',
    name: 'rDownloader',
    url: 'https://github.com/degoya/rDownloader/releases/latest/download/rdownloader-plugin-index.json',
    key_id: null,
    fingerprint: null,
    enabled: true,
    sequence: 5,
    issued_at: null,
    expires_at: null,
    last_checked_at: null,
    last_success_at: null,
    last_error: null,
    ...overrides
  }
}

const COMMUNITY = repository({
  id: 'r1',
  kind: 'third_party',
  name: 'Community',
  url: 'https://plugins.example.test/index.json',
  key_id: 'community-v1',
  fingerprint: FINGERPRINT
})

const MODAL_STUB = { UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' } }

function mount(repositories: PluginRepository[] = [repository()]) {
  listRepositories.mockResolvedValue({ ok: true, data: { repositories, refresh_hours: 24 } })
  return mountComponent(SettingsPluginRepositories, {
    messages: { plugins: pluginsCatalogue },
    stubs: MODAL_STUB
  })
}

function rowOf(name: string): HTMLElement {
  return screen.getByText(name).closest('div.border') as HTMLElement
}

describe('SettingsPluginRepositories', () => {
  beforeEach(() => {
    for (const mock of [listRepositories, addRepository, updateRepository, removeRepository, refreshRepositories]) {
      mock.mockReset()
    }
    updateRepository.mockResolvedValue({ ok: true, data: {} })
    removeRepository.mockResolvedValue({ ok: true, data: { code: 'plugin_repository.removed' } })
    confirmed.mockReset()
    confirmed.mockResolvedValue(true)
  })

  it('offers no removal for the official repository, and one for another', async () => {
    mount([repository(), COMMUNITY])
    await screen.findByText('Community')
    expect(within(rowOf('rDownloader')).queryByRole('button', { name: pluginsCatalogue.repositories.remove })).toBeNull()
    expect(within(rowOf('Community')).getByText('efefefef efefefef efefefef efefefef efefefef efefefef efefefef efefefef')).toBeTruthy()
    await fireEvent.click(within(rowOf('Community')).getByRole('button', { name: pluginsCatalogue.repositories.remove }))
    await waitFor(() => expect(removeRepository).toHaveBeenCalledWith('r1'))
    expect(confirmed.mock.calls[0]?.[0]).toMatchObject({ destructive: true })
  })

  it('switches a repository off with a PATCH and deletes nothing', async () => {
    mount()
    await screen.findByText('rDownloader')
    await fireEvent.click(screen.getByRole('switch', { name: 'Use rDownloader' }))
    await waitFor(() => expect(updateRepository).toHaveBeenCalledWith('official', { enabled: false }))
    expect(removeRepository).not.toHaveBeenCalled()
  })

  it('shows why the last check failed', async () => {
    mount([repository({ last_error: 'plugin_repository.official_key_missing' })])
    expect(await screen.findByText('Last check failed: plugin_repository.official_key_missing')).toBeTruthy()
  })

  it('adds a repository only after its key is approved by the fingerprint shown', async () => {
    mount()
    await screen.findByText('rDownloader')
    addRepository.mockResolvedValueOnce({
      ok: false,
      status: 409,
      message: {
        code: 'plugin_repository.key_unconfirmed',
        params: { key_id: 'community-v1', fingerprint: FINGERPRINT, packages: '3', url: COMMUNITY.url }
      }
    })
    const [url, key] = screen.getAllByRole('textbox')
    await fireEvent.update(url as HTMLInputElement, COMMUNITY.url)
    await fireEvent.update(key as HTMLInputElement, 'BASE64KEY=')
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.repositories.add }))
    await waitFor(() => expect(addRepository).toHaveBeenCalledTimes(1))
    expect(addRepository.mock.calls[0]).toEqual([{ url: COMMUNITY.url, public_key: 'BASE64KEY=' }, undefined])

    expect(await screen.findByText('community-v1')).toBeTruthy()
    expect(screen.getByText('efefefef efefefef efefefef efefefef efefefef efefefef efefefef efefefef')).toBeTruthy()
    expect(screen.getByText(pluginsCatalogue.repositories.approve_warning)).toBeTruthy()

    addRepository.mockResolvedValueOnce({ ok: true, data: COMMUNITY })
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.repositories.approve }))
    await waitFor(() => expect(addRepository).toHaveBeenCalledTimes(2))
    expect(addRepository.mock.calls[1]?.[1]).toBe(FINGERPRINT)
    expect(await screen.findByText(pluginsCatalogue.repositories.added)).toBeTruthy()
  })

  it('checks every repository on request', async () => {
    mount()
    await screen.findByText('rDownloader')
    refreshRepositories.mockResolvedValue({
      ok: true,
      data: { repositories: [repository({ last_success_at: '2026-09-26T10:00:00Z' })], refresh_hours: 24 }
    })
    await fireEvent.click(screen.getByRole('button', { name: pluginsCatalogue.repositories.refresh }))
    expect(await screen.findByText(pluginsCatalogue.repositories.refreshed)).toBeTruthy()
  })
})

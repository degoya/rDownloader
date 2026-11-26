/**
 * RD-1480-07: the settings page says when the document was changed elsewhere, keeps what is not
 * saved yet, and names a field both sides changed before a save overwrites the other value.
 * Mounted through `SettingsView` at the page's own address, as a person reaches it.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, eventStreamMock, toastMock } from '@/test/mocks'
import { createMemoryHistory, createRouter } from 'vue-router'

import { api } from '@/api/client'
import { i18n } from '@/i18n'
import settingsCatalogue from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { loadEveryLocale } from '@/test/locales'
import { mountComponent, passTime, settle } from '@/test/mount'
import { loadSettingsPages } from '@/test/settingsPages'

const handlers = vi.hoisted(() => new Map<string, (event: MessageEvent) => void>())
vi.mock('@/api/client', () => apiClientMock({
  POST: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } }))
}))
vi.mock('@nuxt/ui/composables', () => toastMock())
vi.mock('@/composables/useEventStream', () => eventStreamMock({
  subscribeEvents: (map: Record<string, (event: MessageEvent) => void>) => {
    for (const [name, handler] of Object.entries(map)) handlers.set(name, handler)
    return () => {}
  }
}))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))

import SettingsView from './SettingsView.vue'

let stored: ReturnType<typeof defaultSettings>

beforeAll(loadEveryLocale)
beforeAll(loadSettingsPages, 60_000)

/** The retention page with its log field loaded. */
async function mountRetention(): Promise<HTMLInputElement> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/settings/:section', component: SettingsView }]
  })
  await router.push('/settings/system?tab=retention')
  await router.isReady()
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  const { container } = mountComponent(SettingsView, { i18n, plugins: [router], keepPinia: true })
  return await waitFor(() => {
    const input = container.querySelector<HTMLInputElement>('[data-testid="log-retention"] input')
    expect(input?.value).toBe('50000')
    return input as HTMLInputElement
  })
}

async function changedElsewhere(fields: Partial<typeof stored>): Promise<void> {
  stored = { ...stored, ...fields }
  await passTime(300, () => handlers.get('settings.changed')?.(new MessageEvent('settings.changed')))
  await settle()
}

describe('a change of the settings made elsewhere', () => {
  beforeEach(() => {
    handlers.clear()
    stored = { ...defaultSettings(), log_retention_records: 50000 }
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockImplementation((async (path: string) => path === '/api/v1/settings'
      ? { data: structuredClone(stored) }
      : { data: undefined, error: { code: 'internal' } }) as never)
    vi.mocked(api.PATCH).mockReset()
    vi.mocked(api.PATCH).mockImplementation((async (_path: string, init: { body: object }) => {
      stored = { ...stored, ...init.body }
      return { data: structuredClone(stored) }
    }) as never)
  })

  it('is said, and the unsaved input stays and is saved alone', async () => {
    const records = await mountRetention()
    await fireEvent.update(records, '120000')
    await changedElsewhere({ max_retries: 2 })

    expect(screen.getByTestId('settings-foreign').textContent).toContain(settingsCatalogue.foreign.merged)
    expect(records.value).toBe('120000')

    await fireEvent.click(screen.getByRole('button', { name: settingsCatalogue.save }))
    await waitFor(() => expect(screen.getByText(settingsCatalogue.messages.saved)).toBeTruthy())
    const [, init] = vi.mocked(api.PATCH).mock.calls[0] as unknown as [string, { body: object }]
    expect(init.body).toEqual({ log_retention_records: 120000 })
    expect(stored.max_retries).toBe(2)
  })

  it('in the same field is named, and the other value can be taken over', async () => {
    const records = await mountRetention()
    await fireEvent.update(records, '120000')
    await changedElsewhere({ log_retention_records: 70000 })

    expect(records.value).toBe('120000')
    expect(screen.getByTestId('settings-foreign').textContent).toContain('log_retention_records')

    await fireEvent.click(screen.getByRole('button', { name: settingsCatalogue.foreign.take }))
    await waitFor(() => expect(records.value).toBe('70000'))
    expect(screen.queryByTestId('settings-foreign')).toBeNull()
    expect(api.PATCH).not.toHaveBeenCalled()
  })
})

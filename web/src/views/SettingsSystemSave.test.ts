/**
 * The System page edits fields of the settings document — update check, log, audit and
 * statistics retention, trace export — but had no save bar: a change there was saved only by the
 * button of another settings page (found in RD-180-15). Mounted through `SettingsView` at the
 * page's own address, as a person reaches it.
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import { api } from '@/api/client'
import { i18n } from '@/i18n'
import settingsCatalogue from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { loadEveryLocale } from '@/test/locales'
import { uiStubs } from '@/test/mount'

beforeAll(loadEveryLocale)

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(),
    POST: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } })),
    PUT: vi.fn(),
    PATCH: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } })),
    DELETE: vi.fn(async () => ({ data: undefined, error: { code: 'internal' } }))
  },
  responseError: () => 'The service did not answer',
  resultMessage: () => ''
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }), overlays: [] })
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))

import SettingsView from './SettingsView.vue'

const stored = { ...defaultSettings(), log_retention_records: 50000 }

async function mountSystem(tab: string): Promise<HTMLElement> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/settings/:section', component: SettingsView }]
  })
  await router.push(tab ? `/settings/system?tab=${tab}` : '/settings/system')
  await router.isReady()
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  const { container } = render(SettingsView, { global: { plugins: [router, i18n], stubs: uiStubs as never } })
  return container as HTMLElement
}

function saveButton(): HTMLElement | null {
  return screen.queryByRole('button', { name: settingsCatalogue.save })
}

describe('saving the System page', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockImplementation((async (path: string) => path === '/api/v1/settings'
      ? { data: structuredClone(stored) }
      : { data: undefined, error: { code: 'internal' } }) as never)
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.PUT).mockImplementation((async (_path: string, init: { body: unknown }) => ({ data: init.body })) as never)
  })

  it('saves a changed retention field with the page’s own button', async () => {
    const container = await mountSystem('retention')
    const records = await waitFor(() => {
      const input = container.querySelector<HTMLInputElement>('[data-testid="log-retention"] input')
      expect(input?.value).toBe('50000')
      return input as HTMLInputElement
    })

    await fireEvent.update(records, '120000')
    await fireEvent.click(saveButton() as HTMLElement)

    await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(1))
    const [path, init] = vi.mocked(api.PUT).mock.calls[0] as unknown as [string, { body: { log_retention_records: number } }]
    expect(path).toBe('/api/v1/settings')
    expect(init.body.log_retention_records).toBe(120000)
    await waitFor(() => expect(screen.getByText(settingsCatalogue.messages.saved)).toBeTruthy())
  })

  it('offers the save bar on the updates tab too, and not on the status tab, which edits nothing', async () => {
    await mountSystem('updates')
    await waitFor(() => expect(saveButton()).not.toBeNull())
    // The reset sits in the page header already; the bar does not repeat it.
    expect(screen.getAllByRole('button', { name: settingsCatalogue.reset.button })).toHaveLength(1)

    document.body.innerHTML = ''
    await mountSystem('')
    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/settings'))
    expect(saveButton()).toBeNull()
  })
})

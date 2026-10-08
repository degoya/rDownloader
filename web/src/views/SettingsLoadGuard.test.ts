/**
 * WEB-01: the PUT replaces the whole settings document, and until the GET has answered the form
 * holds the placeholders of `defaultSettings()`. A save while the load failed or was still running
 * — the service restarting while the page was open — wrote those placeholders over the real
 * configuration. The pages bound to the document, the save bar and the reset now wait for a
 * successful load.
 */
import { fireEvent, render, screen, waitFor, within } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import { api } from '@/api/client'
import { i18n } from '@/i18n'
import commonCatalogue from '@/locales/en/common.json'
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
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }), overlays: [] })
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))

import SettingsView from './SettingsView.vue'

const stored = { ...defaultSettings(), log_retention_records: 50000 }
const failed = { data: undefined, error: { code: 'network.unreachable' } }

async function mountSection(section: string): Promise<void> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/settings/:section', component: SettingsView }]
  })
  await router.push(`/settings/${section}`)
  await router.isReady()
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  render(SettingsView, { global: { plugins: [router, i18n], stubs: uiStubs as never } })
}

function button(name: string): HTMLElement | null {
  return screen.queryByRole('button', { name })
}

describe('saving before the settings document has loaded', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.POST).mockClear()
    vi.mocked(api.PUT).mockImplementation((async (_path: string, init: { body: unknown }) => ({ data: init.body })) as never)
  })

  it('shows the failure instead of the form and offers no save or reset', async () => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection('general')

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('The service did not answer'))
    expect(button(settingsCatalogue.save)).toBeNull()
    expect(button(settingsCatalogue.reset.button)).toBeNull()
    expect(document.querySelector('[data-tour="settings-tabs"]')).toBeNull()
    expect(api.PUT).not.toHaveBeenCalled()
    expect(api.POST).not.toHaveBeenCalledWith('/api/v1/settings/reset')
  })

  it('offers no save while the load is still running', async () => {
    let answer: (value: unknown) => void = () => {}
    vi.mocked(api.GET).mockImplementation((path: string) => path === '/api/v1/settings'
      ? new Promise(resolve => { answer = resolve }) as never
      : Promise.resolve(failed) as never)
    await mountSection('general')

    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/settings'))
    expect(screen.getByRole('status')).toBeTruthy()
    expect(button(settingsCatalogue.save)).toBeNull()

    answer({ data: structuredClone(stored) })
    await waitFor(() => expect(button(settingsCatalogue.save)).not.toBeNull())
  })

  it('loads again on retry and saves the stored document, not the placeholders', async () => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection('system?tab=retention')
    await waitFor(() => expect(button(commonCatalogue.actions.retry)).not.toBeNull())

    vi.mocked(api.GET).mockImplementation((async (path: string) => path === '/api/v1/settings'
      ? { data: structuredClone(stored) }
      : failed) as never)
    await fireEvent.click(button(commonCatalogue.actions.retry) as HTMLElement)
    await waitFor(() => expect(button(settingsCatalogue.save)).not.toBeNull())

    await fireEvent.click(button(settingsCatalogue.save) as HTMLElement)
    await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(1))
    const [, init] = vi.mocked(api.PUT).mock.calls[0] as unknown as [string, { body: { log_retention_records: number } }]
    expect(init.body.log_retention_records).toBe(50000)
  })

  /**
   * RA-WEB-05: the lock was per page, so a failed load also took the self-saving sub-tabs —
   * categories, rules, sign-in — with it. It is per sub-tab now, and a waiting sub-tab keeps the
   * tab bar, so the others stay in reach.
   */
  it('keeps a self-saving sub-tab usable when the document cannot be loaded', async () => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection('routing?tab=categories')

    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/settings'))
    await waitFor(() => expect(document.querySelector('[data-tour="settings-tabs"]')).not.toBeNull())
    expect(screen.queryByTestId('settings-document-state')).toBeNull()
    expect(button(settingsCatalogue.save)).toBeNull()
  })

  it('locks only the sub-tab bound to the document, and leaves the way back to the others', async () => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection('security?tab=sessions')

    await waitFor(() => expect(button(commonCatalogue.actions.retry)).not.toBeNull())
    const state = screen.getByTestId('settings-document-state')
    expect(document.querySelector('[data-tour="settings-tabs"]')).toBeNull()
    expect(button(settingsCatalogue.save)).toBeNull()

    await fireEvent.click(within(state).getByRole('tab', { name: settingsCatalogue.subtabs.security.signin }))
    await waitFor(() => expect(document.querySelector('[data-tour="settings-tabs"]')).not.toBeNull())
    expect(screen.queryByTestId('settings-document-state')).toBeNull()
  })

  /**
   * RD-1120-21: the tabs that took a card of the document from General save their other cards
   * themselves, so they stay usable; only the moved card waits, with its own retry.
   */
  it.each(['usenet', 'routing', 'security', 'bandwidth'])('keeps %s usable and lets only the moved card wait', async (section) => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection(section)

    await waitFor(() => expect(within(screen.getByTestId('settings-document-card-state')).getByRole('button', { name: commonCatalogue.actions.retry })).toBeTruthy())
    expect(document.querySelector('[data-tour="settings-tabs"]')).not.toBeNull()
    expect(screen.queryByTestId('settings-document-state')).toBeNull()
    expect(button(settingsCatalogue.save)).toBeNull()
  })

  it('shows the moved card and the save bar once the document has loaded', async () => {
    vi.mocked(api.GET).mockImplementation((async (path: string) => path === '/api/v1/settings'
      ? { data: structuredClone(stored) }
      : failed) as never)
    await mountSection('usenet')

    await waitFor(() => expect(button(settingsCatalogue.save)).not.toBeNull())
    expect(screen.queryByTestId('settings-document-card-state')).toBeNull()
    expect(document.querySelector('[data-settings-anchor="usenet.nntp_connections"]')).not.toBeNull()
  })

  // RD-1160-01: three self-saving pages took tabs; none waits for the document or gets a save bar.
  it.each([['notifications', 'history'], ['backup', 'restore'], ['about', 'licenses']])('opens %s on ?tab=%s without waiting for the document', async (section, tab) => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection(`${section}?tab=${tab}`)

    await waitFor(() => expect(document.querySelector(`[data-tab="${tab}"]`)).not.toBeNull())
    expect((document.querySelector(`[data-tab="${tab}"]`) as HTMLElement).hidden).toBe(false)
    expect(document.querySelectorAll('[role="tabpanel"]:not([hidden])')).toHaveLength(1)
    expect(screen.queryByTestId('settings-document-state')).toBeNull()
    expect(button(settingsCatalogue.save)).toBeNull()
  })

  it('still locks a sub-tab that shows the document without editing it', async () => {
    vi.mocked(api.GET).mockImplementation((async () => failed) as never)
    await mountSection('system')

    await waitFor(() => expect(button(commonCatalogue.actions.retry)).not.toBeNull())
    expect(document.querySelector('[data-tour="settings-tabs"]')).toBeNull()
  })
})

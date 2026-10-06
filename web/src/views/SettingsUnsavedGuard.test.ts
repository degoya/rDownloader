/**
 * Unsaved settings are not lost without a question (RD-180-16). `SettingsView` is rendered
 * through `RouterView` at a page's own address, as a person reaches it, because the router's
 * leave guard only reaches a component the router rendered. The confirmation is replaced by a
 * function each test answers (Nuxt UI's overlay needs `#imports`, see `ControlRoomLayout.test.ts`).
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

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
const confirm = vi.fn<() => Promise<boolean>>()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))

import SettingsView from './SettingsView.vue'

const stored = { ...defaultSettings(), log_retention_records: 50000 }
const storedCaptcha = {
  solver: 'two_captcha_compatible',
  endpoint: 'https://api.2captcha.com',
  has_api_key: true,
  manual_enabled: true,
  manual_timeout_seconds: 180
}

async function mountAt(path: string): Promise<{ router: Router, container: HTMLElement }> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/settings/:section', component: SettingsView },
      { path: '/queue', component: { template: '<p>queue</p>' } }
    ]
  })
  await router.push(path)
  await router.isReady()
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  const { container } = render({ template: '<RouterView />' }, {
    global: { plugins: [router, i18n], stubs: uiStubs as never }
  })
  return { router, container: container as HTMLElement }
}

/** The retention page with its log field loaded, then changed. */
async function editRetention(): Promise<{ router: Router, container: HTMLElement }> {
  const mounted = await mountAt('/settings/system?tab=retention')
  const records = await waitFor(() => {
    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="log-retention"] input')
    expect(input?.value).toBe('50000')
    return input as HTMLInputElement
  })
  await fireEvent.update(records, '120000')
  return mounted
}

describe('leaving the settings with unsaved changes', () => {
  beforeEach(() => {
    confirm.mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockImplementation((async (path: string) => {
      if (path === '/api/v1/settings') return { data: structuredClone(stored) }
      if (path === '/api/v1/captcha-config') return { data: { ...storedCaptcha } }
      return { data: undefined, error: { code: 'internal' } }
    }) as never)
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.PUT).mockImplementation((async (_path: string, init: { body: unknown }) => ({ data: init.body })) as never)
  })

  it('goes without a question when nothing was changed', async () => {
    const { router } = await mountAt('/settings/system?tab=retention')
    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/settings'))

    await router.push('/queue')

    expect(confirm).not.toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/queue')
  })

  it('asks, and Cancel keeps the page with the change still on it', async () => {
    const { router, container } = await editRetention()
    confirm.mockResolvedValue(false)

    await router.push('/queue')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.fullPath).toBe('/settings/system?tab=retention')
    expect(container.querySelector<HTMLInputElement>('[data-testid="log-retention"] input')?.value).toBe('120000')
  })

  it('leaves when the change is discarded', async () => {
    const { router } = await editRetention()
    confirm.mockResolvedValue(true)

    await router.push('/queue')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/queue')
  })

  it('keeps the change across sub-tabs and pages without asking, and asks when the settings are left', async () => {
    const { router } = await editRetention()
    confirm.mockResolvedValue(false)

    await router.push('/settings/system?tab=updates')
    await router.push('/settings/general')
    expect(confirm).not.toHaveBeenCalled()

    await router.push('/queue')
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/settings/general')
  })

  it('asks nothing once the change is saved', async () => {
    const { router } = await editRetention()
    await fireEvent.click(screen.getByRole('button', { name: settingsCatalogue.save }))
    await waitFor(() => expect(screen.getByText(settingsCatalogue.messages.saved)).toBeTruthy())

    await router.push('/queue')

    expect(confirm).not.toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/queue')
  })

  it('asks before another settings page when the captcha form would be dropped', async () => {
    const { router } = await mountAt('/settings/captcha')
    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/captcha-config'))
    confirm.mockResolvedValue(false)

    await router.push('/settings/general')
    expect(confirm).not.toHaveBeenCalled()
    await router.push('/settings/captcha')

    await fireEvent.update(await screen.findByPlaceholderText('••••••••'), 'typed-key')
    await router.push('/settings/general')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/settings/captcha')
  })

  it('asks before another settings page when the proxy form would be dropped (191-01 N3)', async () => {
    const { router } = await mountAt('/settings/network')
    confirm.mockResolvedValue(false)
    await screen.findByPlaceholderText(settingsCatalogue.proxy.name_placeholder)

    await router.push('/settings/general')
    expect(confirm).not.toHaveBeenCalled()
    await router.push('/settings/network')

    await fireEvent.update(await screen.findByPlaceholderText(settingsCatalogue.proxy.name_placeholder), 'Office proxy')
    await router.push('/settings/general')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/settings/network')
  })

  // RD-1120-21 / RD-1120-15 N3: the fields that left General are guarded on their new tabs too,
  // and the save bar under those tabs saves them.
  it('guards and saves the admin login on the sign-in tab it moved to', async () => {
    const { router, container } = await mountAt('/settings/security')
    const field = await waitFor(() => {
      const anchor = container.querySelector<HTMLElement>('[data-settings-anchor="security.admin_login"] [role="switch"]')
      expect(anchor).not.toBeNull()
      return anchor as HTMLElement
    })
    await fireEvent.click(field)
    confirm.mockResolvedValue(false)

    await router.push('/queue')
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/settings/security')

    await fireEvent.click(screen.getByRole('button', { name: settingsCatalogue.save }))
    await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(1))
    const [, init] = vi.mocked(api.PUT).mock.calls[0] as unknown as [string, { body: { admin_login_disabled: boolean } }]
    expect(init.body.admin_login_disabled).toBe(true)
  })

  it('guards the NNTP limits on the Usenet page they moved to', async () => {
    const { router, container } = await mountAt('/settings/usenet')
    const field = await waitFor(() => {
      const input = container.querySelector<HTMLInputElement>('[data-testid="nntp-parallel-files"]')
      expect(input).not.toBeNull()
      return input as HTMLInputElement
    })
    await fireEvent.update(field, '4')
    confirm.mockResolvedValue(false)

    await router.push('/queue')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/settings/usenet')
    expect(screen.getByRole('button', { name: settingsCatalogue.save })).toBeTruthy()
  })
})

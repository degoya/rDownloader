/**
 * *FTP, SFTP, WebDAV & S3* in two tabs (RD-1160-01), built like the other pages with tabs: the
 * remote logins with their service-off notice on the first, the object storage profiles on *S3*,
 * the tab in the address as `?tab=` and the search landing on the tab its card is on. The page
 * runs here under a real router and the settings view's own `useSettingsSubTab`, as it does there.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive } from 'vue'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import type { Settings } from '@/api/types'
import { useSettingsSubTab } from '@/composables/useSettingsSubTab'
import remote from '@/locales/en/remote.json'
import server from '@/locales/en/server.json'
import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { SETTINGS_SEARCH_ENTRIES, settingsSearchLocation } from '@/settingsSearch'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsTransfersTab from './SettingsTransfersTab.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: () => 'failed',
  resultMessage: () => 'done'
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }) })
}))

const TABS = settingsMessages.subtabs.transfers

async function mountAt(location: string | { path: string, query?: Record<string, string> }, values: Partial<Settings> = {}) {
  const router: Router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/settings/:section', component: { render: () => null } }]
  })
  await router.push(location)
  await router.isReady()
  const settings = reactive({ ...defaultSettings(), ...values })
  const Host = defineComponent({
    setup() {
      const { active } = useSettingsSubTab(() => 'transfers')
      return () => h(SettingsTransfersTab, {
        'modelValue': settings,
        'subTab': active.value,
        'onUpdate:subTab': (value: string) => { active.value = value }
      })
    }
  })
  const view = mountComponent(Host, { messages: { settings: settingsMessages, remote, server }, plugins: [router] })
  return { ...view, router, settings }
}

function panel(container: Element, tab: string): HTMLElement {
  return container.querySelector(`[data-tab="${tab}"]`) as HTMLElement
}

describe('SettingsTransfersTab tabs', () => {
  it('opens on FTP, SFTP & WebDAV with its notice, and keeps S3 on its own tab', async () => {
    const { container } = await mountAt('/settings/transfers', { remote_service_enabled: false })

    expect(screen.getAllByRole('tab').map(tab => tab.textContent?.trim())).toEqual([TABS.remote, TABS.s3])
    expect(screen.getByRole('tab', { name: TABS.remote }).getAttribute('aria-selected')).toBe('true')
    const first = panel(container, 'remote')
    expect(first.hidden).toBe(false)
    expect(first.querySelector('[data-service-off="remote"]')).not.toBeNull()
    expect(first.querySelector('[data-settings-anchor="transfers.remote"]')).not.toBeNull()
    expect(panel(container, 's3').querySelector('[data-settings-anchor="transfers.object_storage"]')).not.toBeNull()
    expect(panel(container, 's3').querySelector('[data-service-off]')).toBeNull()
    expect(screen.getByRole('heading', { level: 2 }).textContent?.trim()).toBe(settingsMessages.headers.transfers.title)
  })

  it('opens the tab ?tab= names', async () => {
    const { container } = await mountAt('/settings/transfers?tab=s3')

    expect(screen.getByRole('tab', { name: TABS.s3 }).getAttribute('aria-selected')).toBe('true')
    expect(panel(container, 's3').hidden).toBe(false)
    expect(panel(container, 'remote').hidden).toBe(true)
  })

  it('puts the tab into the address, and none for the first', async () => {
    const { router } = await mountAt('/settings/transfers')

    await fireEvent.click(screen.getByRole('tab', { name: TABS.s3 }))
    await waitFor(() => expect(router.currentRoute.value.query.tab).toBe('s3'))
    await fireEvent.click(screen.getByRole('tab', { name: TABS.remote }))
    await waitFor(() => expect(router.currentRoute.value.query.tab).toBeUndefined())
  })

  it('lands every search entry of the page on the tab that shows it', async () => {
    const entries = SETTINGS_SEARCH_ENTRIES.filter(entry => entry.section === 'transfers')
    expect(entries.map(entry => entry.tab)).toEqual(['remote', 'remote', 's3'])
    for (const entry of entries) {
      const { container, unmount } = await mountAt(settingsSearchLocation(entry))
      const shown = container.querySelector('[role="tabpanel"]:not([hidden])') as HTMLElement
      expect(shown.querySelector(`[data-settings-anchor="${entry.id}"]`), entry.id).not.toBeNull()
      unmount()
    }
  })

  it('keeps an unsaved limit through a switch to S3 and back', async () => {
    const { settings } = await mountAt('/settings/transfers')
    const field = screen.getByLabelText(remote.settings.max_parallel) as HTMLInputElement
    await fireEvent.update(field, '6')

    await fireEvent.click(screen.getByRole('tab', { name: TABS.s3 }))
    await fireEvent.click(screen.getByRole('tab', { name: TABS.remote }))
    expect(settings.remote_max_parallel).toBe(6)
    expect((screen.getByLabelText(remote.settings.max_parallel) as HTMLInputElement).value).toBe('6')
  })

  it('renders without an axe violation', async () => {
    const { container } = await mountAt('/settings/transfers', { remote_service_enabled: false })
    await waitFor(() => expect(panel(container, 's3').querySelector('[data-settings-anchor]')).not.toBeNull())
    expect(await axeViolations(container)).toBe('')
  })
})

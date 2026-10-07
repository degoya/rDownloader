/**
 * *Media, galleries & streams* in three tabs (RD-1160-01), built like the other pages with tabs:
 * one helper per tab, each with its own service-off notice, the tab in the address as `?tab=` and
 * the search landing on the tab its card is on. The page runs here under a real router and the
 * settings view's own `useSettingsSubTab`, as it does there.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive } from 'vue'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import type { Settings } from '@/api/types'
import { useSettingsSubTab } from '@/composables/useSettingsSubTab'
import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { SETTINGS_SEARCH_ENTRIES, settingsSearchLocation } from '@/settingsSearch'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsMediaTab from './SettingsMediaTab.vue'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn() } }))

const TABS = settingsMessages.subtabs.media

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
      const { active } = useSettingsSubTab(() => 'media')
      return () => h(SettingsMediaTab, {
        'modelValue': settings,
        'subTab': active.value,
        'onUpdate:subTab': (value: string) => { active.value = value }
      })
    }
  })
  const view = mountComponent(Host, { messages: { settings: settingsMessages }, plugins: [router] })
  return { ...view, router, settings }
}

function panel(container: Element, tab: string): HTMLElement {
  return container.querySelector(`[data-tab="${tab}"]`) as HTMLElement
}

const ALL_OFF = { media_service_enabled: false, gallery_service_enabled: false, recording_service_enabled: false }

describe('SettingsMediaTab tabs', () => {
  it('gives each helper its tab, with its own service-off notice there', async () => {
    const { container } = await mountAt('/settings/media', ALL_OFF)

    expect(screen.getAllByRole('tab').map(tab => tab.textContent?.trim())).toEqual([TABS.media, TABS.galleries, TABS.streams])
    expect(screen.getByRole('tab', { name: TABS.media }).getAttribute('aria-selected')).toBe('true')
    for (const [tab, service, anchor] of [
      ['media', 'media', 'media.media'],
      ['galleries', 'gallery', 'media.gallery'],
      ['streams', 'recording', 'media.streams']
    ] as const) {
      const shown = panel(container, tab)
      expect([...shown.querySelectorAll('[data-service-off]')].map(alert => alert.getAttribute('data-service-off')), tab).toEqual([service])
      expect(shown.querySelector(`[data-settings-anchor="${anchor}"]`), tab).not.toBeNull()
    }
    expect(screen.getByRole('heading', { level: 2 }).textContent?.trim()).toBe(settingsMessages.headers.media.title)
  })

  it('shows no notice for a service that is on', async () => {
    const { container } = await mountAt('/settings/media', { ...ALL_OFF, gallery_service_enabled: true })

    expect(panel(container, 'galleries').querySelector('[data-service-off]')).toBeNull()
    expect(panel(container, 'streams').querySelector('[data-service-off="recording"]')).not.toBeNull()
  })

  it('opens the tab ?tab= names', async () => {
    const { container } = await mountAt('/settings/media?tab=streams')

    expect(screen.getByRole('tab', { name: TABS.streams }).getAttribute('aria-selected')).toBe('true')
    expect(panel(container, 'streams').hidden).toBe(false)
    expect(panel(container, 'media').hidden).toBe(true)
    expect(panel(container, 'galleries').hidden).toBe(true)
  })

  it('puts the tab into the address, and none for the first', async () => {
    const { router } = await mountAt('/settings/media')

    await fireEvent.click(screen.getByRole('tab', { name: TABS.galleries }))
    await waitFor(() => expect(router.currentRoute.value.query.tab).toBe('galleries'))
    await fireEvent.click(screen.getByRole('tab', { name: TABS.media }))
    await waitFor(() => expect(router.currentRoute.value.query.tab).toBeUndefined())
  })

  it('lands every search entry of the page on the tab that shows it', async () => {
    const entries = SETTINGS_SEARCH_ENTRIES.filter(entry => entry.section === 'media')
    expect(entries.map(entry => entry.tab)).toEqual(['media', 'galleries', 'streams'])
    for (const entry of entries) {
      const { container, unmount } = await mountAt(settingsSearchLocation(entry))
      const shown = container.querySelector('[role="tabpanel"]:not([hidden])') as HTMLElement
      expect(shown.querySelector(`[data-settings-anchor="${entry.id}"]`), entry.id).not.toBeNull()
      unmount()
    }
  })

  it('keeps an unsaved value through a switch to another tab and back', async () => {
    const { settings } = await mountAt('/settings/media?tab=streams')
    await fireEvent.update(screen.getByLabelText(settingsMessages.streams.max_parallel.label), '5')

    await fireEvent.click(screen.getByRole('tab', { name: TABS.media }))
    await fireEvent.click(screen.getByRole('tab', { name: TABS.streams }))
    expect(settings.record_max_parallel).toBe(5)
    expect((screen.getByLabelText(settingsMessages.streams.max_parallel.label) as HTMLInputElement).value).toBe('5')
  })

  it('renders without an axe violation', async () => {
    const { container } = await mountAt('/settings/media', ALL_OFF)
    expect(await axeViolations(container)).toBe('')
  })
})

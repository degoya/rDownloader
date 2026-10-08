/**
 * *Notifications* in two tabs (RD-1160-01; owner, 2026-10-07): the delivery history used to sit
 * under the forms that set up targets and rules. It has a tab of its own now, built like every
 * other settings page with tabs: the tab in the address, the search's anchors in the tab it names.
 */
import { fireEvent, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import settings from '@/locales/en/settings.json'
import { SETTINGS_SEARCH_ENTRIES } from '@/settingsSearch'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsNotificationsTab from './SettingsNotificationsTab.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })) },
  responseError: vi.fn(() => 'The service did not answer')
}))
// The shared lists follow the event stream (WEB-3); jsdom has no `EventSource`.
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

const reload = vi.fn()

/** The cards keep their own tests; here only where they stand. */
const stubs = {
  NotificationTargets: {
    emits: ['changed'],
    template: '<section data-settings-anchor="notifications.targets"><button type="button" @click="$emit(\'changed\')">changed</button></section>'
  },
  NotificationRules: { template: '<section data-settings-anchor="notifications.rules" />' },
  NotificationHistory: { template: '<section data-settings-anchor="notifications.history" />', methods: { reload } }
}

function mount(subTab?: string) {
  return mountComponent(SettingsNotificationsTab, { messages: { settings }, props: subTab ? { subTab } : {}, stubs })
}

const panel = (container: Element, tab: string) => container.querySelector(`[data-tab="${tab}"]`) as HTMLElement

describe('SettingsNotificationsTab', () => {
  it('has the tabs Targets & rules and History, in that order, the first one open', () => {
    const { getAllByRole, container } = mount()

    expect(getAllByRole('tab').map(tab => tab.textContent?.trim()))
      .toEqual([settings.subtabs.notifications.targets, settings.subtabs.notifications.history])
    expect(panel(container, 'targets').hidden).toBe(false)
    expect(panel(container, 'history').hidden).toBe(true)
  })

  it('opens the tab it is handed from the address and hands a chosen one back', async () => {
    const { container, getAllByRole, emitted } = mount('history')

    expect(panel(container, 'history').hidden).toBe(false)
    expect(panel(container, 'targets').hidden).toBe(true)
    await fireEvent.click(getAllByRole('tab')[0] as HTMLElement)
    expect(emitted()['update:subTab']).toEqual([['targets']])
  })

  it('puts every card the search finds on the tab its entry names', () => {
    const { container } = mount()

    const entries = SETTINGS_SEARCH_ENTRIES.filter(entry => entry.section === 'notifications')
    expect(entries.map(entry => entry.id)).toEqual(['notifications.targets', 'notifications.rules', 'notifications.history'])
    for (const entry of entries) {
      expect(panel(container, entry.tab ?? '').querySelector(`[data-settings-anchor="${entry.id}"]`), entry.id).not.toBeNull()
    }
  })

  // The history stays mounted behind its tab, so a changed target still reaches it.
  it('reloads the history in its hidden tab when a target changed', async () => {
    const { getByRole } = mount()

    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/notifications/targets'))
    await fireEvent.click(getByRole('button', { name: 'changed' }))
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1))
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()

    expect(await axeViolations(container)).toBe('')
  })
})

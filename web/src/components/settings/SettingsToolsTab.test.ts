/**
 * *Tools* in tabs (RD-1240-26; owner, 2026-10-10), by what somebody comes to do: see what was
 * found, say where to look, let the service install versions. The card that held the vendor folder
 * and the managed switches was split along those two subjects.
 */
import { fireEvent } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settings from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { SETTINGS_SEARCH_ENTRIES } from '@/settingsSearch'
import { mountComponent } from '@/test/mount'

import SettingsToolsTab from './SettingsToolsTab.vue'

/** The status and the managed versions load on their own and keep their own tests. */
const stubs = {
  SettingsToolStatus: { template: '<section data-settings-anchor="tools.status" />' },
  SettingsManagedTools: { template: '<section data-settings-anchor="tools.managed" />' }
}

function mount(subTab?: string) {
  const props = subTab ? { modelValue: defaultSettings(), subTab } : { modelValue: defaultSettings() }
  return mountComponent(SettingsToolsTab, { messages: { settings }, props, stubs })
}

const panel = (container: Element, tab: string) => container.querySelector(`[data-tab="${tab}"]`) as HTMLElement

describe('SettingsToolsTab', () => {
  it('has the tabs Status, Paths and Managed tools, in that order, the first one open', () => {
    const { container, getAllByRole } = mount()

    const labels = settings.subtabs.tools
    expect(getAllByRole('tab').map(tab => tab.textContent?.trim())).toEqual([labels.status, labels.paths, labels.managed])
    expect(panel(container, 'status').hidden).toBe(false)
    expect(panel(container, 'paths').hidden).toBe(true)
    expect(panel(container, 'managed').hidden).toBe(true)
  })

  it('opens the tab it is handed from the address and hands a chosen one back', async () => {
    const { container, getAllByRole, emitted } = mount('managed')

    expect(panel(container, 'managed').hidden).toBe(false)
    await fireEvent.click(getAllByRole('tab')[1] as HTMLElement)
    expect(emitted()['update:subTab']).toEqual([['paths']])
  })

  it('keeps the managed switches beside the managed versions, the vendor folder beside the paths', () => {
    const { container } = mount()

    const managed = panel(container, 'managed')
    expect(managed.textContent).toContain(settings.managed_tools.enabled_label)
    expect(managed.textContent).toContain(settings.managed_tools.manifest_url_label)
    expect(managed.textContent).toContain(settings.managed_tools.overrides_label)
    expect(panel(container, 'paths').textContent).not.toContain(settings.managed_tools.enabled_label)
    expect(panel(container, 'paths').textContent).toContain(settings.vendor.directory.label)
  })

  it('puts every card and field the search finds on the tab its entry names', () => {
    const { container } = mount()

    const entries = SETTINGS_SEARCH_ENTRIES.filter(entry => entry.section === 'tools')
    expect(entries).toHaveLength(6)
    for (const entry of entries) {
      expect(panel(container, entry.tab ?? '').querySelector(`[data-settings-anchor="${entry.id}"]`), entry.id).not.toBeNull()
    }
  })
})

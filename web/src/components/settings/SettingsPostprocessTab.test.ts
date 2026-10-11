/**
 * *Post-processing* in tabs (RD-1240-26; owner, 2026-10-10): the pipeline card held some
 * twenty-five fields. The malware scan and the package names are tabs of their own, the rest is
 * split in the order a package meets it; the tab is in the address like on every other page.
 */
import { fireEvent } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settings from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { SETTINGS_SEARCH_ENTRIES } from '@/settingsSearch'
import { mountComponent } from '@/test/mount'

import SettingsPostprocessTab from './SettingsPostprocessTab.vue'

/** The cards keep their own tests; here only where they stand, with the anchors each carries. */
const anchors = (...ids: string[]) => ({ template: `<section>${ids.map(id => `<div data-settings-anchor="${id}" />`).join('')}</section>` })
const stubs = {
  SettingsPostprocessCard: anchors('postprocess.defaults', 'postprocess.unpack_to_subfolder', 'postprocess.unwrap_package_folder', 'postprocess.direct_unpack', 'postprocess.passwords_file'),
  SettingsPostprocessRepairCard: anchors('postprocess.delete_par2', 'postprocess.cleanup_extensions'),
  SettingsPackageNameRules: anchors('postprocess.package_names'),
  SettingsMalwareScan: anchors('postprocess.malware_scan'),
  SettingsPostprocessDeliveryCard: anchors('postprocess.scripts_directory', 'postprocess.mcp_scripts_allowed', 'postprocess.upload')
}

function mount(subTab?: string) {
  const props = subTab ? { modelValue: defaultSettings(), subTab } : { modelValue: defaultSettings() }
  return mountComponent(SettingsPostprocessTab, { messages: { settings }, props, stubs })
}

const panel = (container: Element, tab: string) => container.querySelector(`[data-tab="${tab}"]`) as HTMLElement

describe('SettingsPostprocessTab', () => {
  it('has five tabs in pipeline order, malware scan and package names each on its own, the first one open', () => {
    const { container, getAllByRole } = mount()

    const labels = settings.subtabs.postprocess
    expect(getAllByRole('tab').map(tab => tab.textContent?.trim()))
      .toEqual([labels.unpack, labels.repair, labels.names, labels.malware, labels.delivery])
    expect(panel(container, 'unpack').hidden).toBe(false)
    for (const tab of ['repair', 'names', 'malware', 'delivery']) expect(panel(container, tab).hidden, tab).toBe(true)
  })

  it('opens the tab it is handed from the address and hands a chosen one back', async () => {
    const { container, getAllByRole, emitted } = mount('malware')

    expect(panel(container, 'malware').hidden).toBe(false)
    expect(panel(container, 'unpack').hidden).toBe(true)
    await fireEvent.click(getAllByRole('tab')[2] as HTMLElement)
    expect(emitted()['update:subTab']).toEqual([['names']])
  })

  it('puts every card and field the search finds on the tab its entry names', () => {
    const { container } = mount()

    const entries = SETTINGS_SEARCH_ENTRIES.filter(entry => entry.section === 'postprocess')
    expect(entries).toHaveLength(12)
    for (const entry of entries) {
      expect(panel(container, entry.tab ?? '').querySelector(`[data-settings-anchor="${entry.id}"]`), entry.id).not.toBeNull()
    }
  })
})

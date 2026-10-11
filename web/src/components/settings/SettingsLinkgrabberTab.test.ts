/**
 * The LinkGrabber's page (RD-1120-23): the excluded domains and DLC from the "Collector" sub-tab
 * of Storage & rules and mirror detection from General; its display switches went to Interface.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settings from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { SETTINGS_SEARCH_ENTRIES } from '@/settingsSearch'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsLinkgrabberTab from './SettingsLinkgrabberTab.vue'

/** The LinkFilter card (RD-1240-09) loads and saves on its own; `SettingsLinkFilters.test.ts` has it. */
const stubs = { SettingsLinkFilters: { template: '<section data-testid="link-filters" data-settings-anchor="linkgrabber.link_filters" />' } }

function mount(subTab?: string) {
  const props = subTab ? { modelValue: defaultSettings(), subTab } : { modelValue: defaultSettings() }
  return mountComponent(SettingsLinkgrabberTab, { messages: { settings }, props, stubs })
}

const panel = (container: Element, tab: string) => container.querySelector(`[data-tab="${tab}"]`) as HTMLElement

describe('SettingsLinkgrabberTab', () => {
  it('holds the blocklist, DLC and mirror detection, and no display switch', () => {
    const { container } = mount()

    for (const anchor of ['linkgrabber.blocklist', 'linkgrabber.excluded_domains', 'linkgrabber.dlc', 'linkgrabber.mirrors']) {
      expect(container.querySelector(`[data-settings-anchor="${anchor}"]`), anchor).not.toBeNull()
    }
    expect(screen.queryByText(settings.collector.indexer_images.enabled.label)).toBeNull()
    expect(screen.queryByText(settings.collector.nzb_hand_over.title)).toBeNull()
    expect(container.querySelector('h2')?.textContent?.trim()).toBe(settings.headers.linkgrabber.title)
    expect(screen.getByTestId('link-filters')).toBeTruthy()
  })

  it('names nothing "Collector" any more', () => {
    const { container } = mount()

    expect(container.textContent).not.toMatch(/Collector/)
  })

  it('switches mirror detection in the settings document', async () => {
    const model = { ...defaultSettings(), mirror_detection: true }
    const { container } = mountComponent(SettingsLinkgrabberTab, { messages: { settings }, props: { modelValue: model }, stubs })

    await fireEvent.click(container.querySelector('[data-settings-anchor="linkgrabber.mirrors"] [role="switch"]') as HTMLElement)
    expect(model.mirror_detection).toBe(false)
  })

  // RD-1240-26: one card per tab, the two switches for every link first.
  it('has the tabs General, Blocklist, Containers and LinkFilter, in that order, the first one open', () => {
    const { container, getAllByRole } = mount()

    expect(getAllByRole('tab').map(tab => tab.textContent?.trim())).toEqual([
      settings.subtabs.linkgrabber.general,
      settings.subtabs.linkgrabber.blocklist,
      settings.subtabs.linkgrabber.containers,
      settings.subtabs.linkgrabber.filters
    ])
    expect(panel(container, 'general').hidden).toBe(false)
    for (const tab of ['blocklist', 'containers', 'filters']) expect(panel(container, tab).hidden, tab).toBe(true)
  })

  it('opens the tab it is handed from the address and hands a chosen one back', async () => {
    const { container, getAllByRole, emitted } = mount('filters')

    expect(panel(container, 'filters').hidden).toBe(false)
    expect(panel(container, 'general').hidden).toBe(true)
    await fireEvent.click(getAllByRole('tab')[1] as HTMLElement)
    expect(emitted()['update:subTab']).toEqual([['blocklist']])
  })

  it('puts every card and field the search finds on the tab its entry names', () => {
    const { container } = mount()

    const entries = SETTINGS_SEARCH_ENTRIES.filter(entry => entry.section === 'linkgrabber')
    expect(entries).toHaveLength(6)
    for (const entry of entries) {
      expect(panel(container, entry.tab ?? '').querySelector(`[data-settings-anchor="${entry.id}"]`), entry.id).not.toBeNull()
    }
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()

    expect(await axeViolations(container)).toBe('')
  })
})

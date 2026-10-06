/**
 * The LinkGrabber's page (RD-1120-23): the excluded domains and DLC from the "Collector" sub-tab
 * of Storage & rules and mirror detection from General; its display switches went to Interface.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settings from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsLinkgrabberTab from './SettingsLinkgrabberTab.vue'

function mount() {
  return mountComponent(SettingsLinkgrabberTab, { messages: { settings }, props: { modelValue: defaultSettings() } })
}

describe('SettingsLinkgrabberTab', () => {
  it('holds the blocklist, DLC and mirror detection, and no display switch', () => {
    const { container } = mount()

    for (const anchor of ['linkgrabber.blocklist', 'linkgrabber.excluded_domains', 'linkgrabber.dlc', 'linkgrabber.mirrors']) {
      expect(container.querySelector(`[data-settings-anchor="${anchor}"]`), anchor).not.toBeNull()
    }
    expect(screen.queryByText(settings.collector.indexer_images.enabled.label)).toBeNull()
    expect(screen.queryByText(settings.collector.nzb_hand_over.title)).toBeNull()
    expect(container.querySelector('h2')?.textContent?.trim()).toBe(settings.headers.linkgrabber.title)
  })

  it('names nothing "Collector" any more', () => {
    const { container } = mount()

    expect(container.textContent).not.toMatch(/Collector/)
  })

  it('switches mirror detection in the settings document', async () => {
    const model = { ...defaultSettings(), mirror_detection: true }
    const { container } = mountComponent(SettingsLinkgrabberTab, { messages: { settings }, props: { modelValue: model } })

    await fireEvent.click(container.querySelector('[data-settings-anchor="linkgrabber.mirrors"] [role="switch"]') as HTMLElement)
    expect(model.mirror_detection).toBe(false)
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()

    expect(await axeViolations(container)).toBe('')
  })
})

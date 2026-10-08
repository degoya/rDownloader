import { describe, expect, it } from 'vitest'

import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsInterfaceTab from './SettingsInterfaceTab.vue'


/**
 * Any class that puts two settings beside each other: `grid-cols-N` or `col-span-N` with N > 1,
 * at any breakpoint prefix. `grid-cols-1` and `col-span-1` are the single column and stay.
 */
const SIDE_BY_SIDE = /(?:^|:)(?:grid-cols|col-span)-(?!1$)\d+$/

function sideBySideClasses(container: Element): string[] {
  return [...container.querySelectorAll('*')]
    .flatMap(element => [...element.classList])
    .filter(name => SIDE_BY_SIDE.test(name))
}

function mount() {
  const model = {
    byte_display: 'binary',
    byte_unit: 'auto',
    title_status_enabled: true
  }
  return mountComponent(SettingsInterfaceTab, { messages: { settings }, props: { modelValue: model as never } })
}

/**
 * RD-120-27: one setting per row. Labels and their hints differ in length, so a pair sat at two
 * different heights and the eye jumped between entries that have nothing to do with each other.
 */
describe('SettingsInterfaceTab layout', () => {
  it('renders no setting beside another', () => {
    const { container } = mount()

    expect(sideBySideClasses(container)).toEqual([])
  })

  it('still renders all four appearance selects, one per row', () => {
    const { container } = mount()

    expect(container.querySelectorAll('select')).toHaveLength(4)
  })
})

/**
 * RD-1120-23: the page said "This browser only" over fields of the settings document. The card
 * of the browser's own choices says so now, and the fields every browser shares — sizes, the tab
 * title and the two display switches that were on Storage & rules — are a card of their own.
 */
describe('SettingsInterfaceTab browser and display', () => {
  function card(anchor: string): HTMLElement {
    return document.querySelector(`[data-settings-anchor="${anchor}"]`) as HTMLElement
  }

  it('keeps language, theme and notifications under "This browser only"', () => {
    mount()

    const appearance = card('interface.appearance')
    expect(appearance.textContent).toContain(settings.appearance.eyebrow)
    // The page header no longer claims the whole page for this browser.
    expect(settings.headers.interface.eyebrow).not.toBe(settings.appearance.eyebrow)
    for (const anchor of ['interface.language', 'interface.theme', 'interface.browser_notifications']) {
      expect(appearance.querySelector(`[data-settings-anchor="${anchor}"]`), anchor).not.toBeNull()
    }
    expect(appearance.querySelector('[data-settings-anchor="interface.byte_display"]')).toBeNull()
  })

  it('puts the shared fields and the display switches on the display card', () => {
    mount()

    const display = card('interface.display')
    expect(display.textContent).toContain(settings.display.title)
    for (const anchor of ['interface.byte_display', 'interface.title_status', 'interface.indexer_images', 'interface.nzb_hand_over', 'interface.package_groups']) {
      expect(display.querySelector(`[data-settings-anchor="${anchor}"]`), anchor).not.toBeNull()
    }
    expect(display.textContent).toContain(settings.collector.nzb_hand_over.linkgrabber.label)
    expect(display.textContent).toContain(settings.collector.nzb_hand_over.downloads.label)
    expect(display.textContent).toContain(settings.package_groups.downloads.label)
    expect(display.textContent).toContain(settings.package_groups.linkgrabber.label)
  })
})

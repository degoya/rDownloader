import { beforeAll, describe, expect, it } from 'vitest'

import { SUPPORTED_LOCALES, i18n } from '@/i18n'
import { SETTINGS_SEARCH_ENTRIES, SETTINGS_SEARCH_PAGES, settingsSearchLocation } from '@/settingsSearch'
import { SETTINGS_SECTIONS, SETTINGS_SUB_TABS, settingsSubTab, settingsSubTabs } from '@/settingsSections'
import { loadEveryLocale } from '@/test/locales'

beforeAll(loadEveryLocale)

/** Every component and view source, read as text: the anchors live in the templates. */
const sources: Record<string, string> = {
  ...import.meta.glob<string>('@/components/**/*.vue', { eager: true, query: '?raw', import: 'default' }),
  ...import.meta.glob<string>('@/views/**/*.vue', { eager: true, query: '?raw', import: 'default' })
}

function anchorsInSources(): string[] {
  return Object.values(sources).flatMap(source =>
    [...source.matchAll(/data-settings-anchor="([^"]+)"/g)].map(match => match[1] ?? ''))
}

/** The component that renders a settings page with sub-tabs: `routing` → `SettingsRoutingTab.vue`. */
function pageSource(section: string): string {
  const file = `/Settings${section.charAt(0).toUpperCase()}${section.slice(1)}Tab.vue`
  return Object.entries(sources).find(([path]) => path.endsWith(file))?.[1] ?? ''
}

/** A page's `UTabs` slots by name, each with the template it renders, in source order. */
function tabSlots(section: string): Record<string, string> {
  const source = pageSource(section)
  const tabs = source.slice(source.indexOf('<UTabs'), source.indexOf('</UTabs>'))
  return Object.fromEntries([...tabs.matchAll(/^( *)<template #([a-z]+)>\n([\s\S]*?)^\1<\/template>/gm)]
    .map(match => [match[2] ?? '', match[3] ?? '']))
}

/** The anchors a template carries itself and through every component it renders, however deep. */
function anchorsReachableFrom(template: string, seen = new Set<string>()): Set<string> {
  const found = new Set([...template.matchAll(/data-settings-anchor="([^"]+)"/g)].map(match => match[1] ?? ''))
  for (const [, name] of template.matchAll(/<([A-Z][A-Za-z]+)[\s/>]/g)) {
    if (!name || seen.has(name)) continue
    seen.add(name)
    const child = Object.entries(sources).find(([path]) => path.endsWith(`/${name}.vue`))?.[1]
    if (child) for (const anchor of anchorsReachableFrom(child, seen)) found.add(anchor)
  }
  return found
}

describe('the settings search registry (RD-170-15)', () => {
  it('has a row for every settings page and no row for a page that does not exist', () => {
    expect(Object.keys(SETTINGS_SEARCH_PAGES).sort()).toEqual(SETTINGS_SECTIONS.map(section => section.value).sort())
  })

  it('reaches into every settings page with at least one card or field', () => {
    const covered = new Set(SETTINGS_SEARCH_ENTRIES.map(entry => entry.section))
    expect(SETTINGS_SECTIONS.map(section => section.value).filter(value => !covered.has(value))).toEqual([])
  })

  it('names a sub-tab for every entry on a page that has them, and only there', () => {
    const wrong = SETTINGS_SEARCH_ENTRIES.filter(entry => settingsSubTabs(entry.section).length
      ? settingsSubTab(entry.section, entry.tab) === null
      : entry.tab !== undefined)
    expect(wrong.map(entry => entry.id)).toEqual([])
  })

  it('reaches every sub-tab of every page', () => {
    const unreached = Object.entries(SETTINGS_SUB_TABS).flatMap(([section, tabs]) => tabs
      .filter(tab => !SETTINGS_SEARCH_ENTRIES.some(entry => entry.section === section && entry.tab === tab.value))
      .map(tab => `${section}?tab=${tab.value}`))
    expect(unreached).toEqual([])
  })

  it('renders each page’s sub-tabs as the table declares them, in order', () => {
    for (const [section, tabs] of Object.entries(SETTINGS_SUB_TABS)) {
      expect(Object.keys(tabSlots(section)), section).toEqual(tabs.map(tab => tab.value))
    }
  })

  // The entry's `tab` is a claim about the template; a card moved to another tab without its
  // entry would open the right page on the wrong tab and wait there for nothing (RD-180-15).
  it('finds every entry’s anchor inside the tab it names', () => {
    const misplaced = SETTINGS_SEARCH_ENTRIES.flatMap((entry) => {
      if (!entry.tab) return []
      const slot = tabSlots(entry.section)[entry.tab] ?? ''
      return anchorsReachableFrom(slot).has(entry.id) ? [] : [`${entry.id} not on ${entry.section}?tab=${entry.tab}`]
    })
    expect(misplaced).toEqual([])
  })

  it('gives every entry a unique id', () => {
    const ids = SETTINGS_SEARCH_ENTRIES.map(entry => entry.id)
    expect(new Set(ids).size).toBe(ids.length)
  })

  it('has exactly one anchor in the templates for every entry, and an entry for every anchor', () => {
    const anchors = anchorsInSources()
    const ids = SETTINGS_SEARCH_ENTRIES.map(entry => entry.id)
    expect(ids.filter(id => anchors.filter(anchor => anchor === id).length !== 1)).toEqual([])
    expect(anchors.filter(anchor => !ids.includes(anchor))).toEqual([])
  })

  it('resolves every title, description and synonym key in all four languages', () => {
    const keys = [
      ...SETTINGS_SEARCH_ENTRIES.flatMap(entry => [entry.titleKey, entry.descriptionKey, entry.keywordsKey]),
      ...Object.values(SETTINGS_SEARCH_PAGES).map(page => page.keywordsKey)
    ].filter((key): key is string => typeof key === 'string')
    const missing = SUPPORTED_LOCALES.flatMap(locale =>
      keys.filter(key => !i18n.global.te(key, locale)).map(key => `${locale}: ${key}`))
    expect(missing).toEqual([])
  })

  it('leads to the page and, on a page with sub-tabs, to the tab', () => {
    expect(settingsSearchLocation({ section: 'backup' })).toEqual({ path: '/settings/backup' })
    expect(settingsSearchLocation({ section: 'routing', tab: 'collector' }))
      .toEqual({ path: '/settings/routing', query: { tab: 'collector' } })
    expect(settingsSubTab('routing', 'collector')).toBe('collector')
    expect(settingsSubTab('routing', 'nonsense')).toBeNull()
    expect(settingsSubTab('routing', ['collector'])).toBeNull()
    // A tab of another page is not a tab of this one.
    expect(settingsSubTab('plugins', 'collector')).toBeNull()
  })

  it('opens the cards that moved into a sub-tab on that tab', () => {
    const location = (id: string) => {
      const entry = SETTINGS_SEARCH_ENTRIES.find(candidate => candidate.id === id)
      return entry ? settingsSearchLocation(entry) : null
    }
    expect(location('plugins.keys')).toEqual({ path: '/settings/plugins', query: { tab: 'trust' } })
    expect(location('plugins.updates')).toEqual({ path: '/settings/plugins', query: { tab: 'updates' } })
    expect(location('plugins.install')).toEqual({ path: '/settings/plugins', query: { tab: 'add' } })
    expect(location('security.allowed_hosts')).toEqual({ path: '/settings/security', query: { tab: 'proxy' } })
    expect(location('network.reconnect')).toEqual({ path: '/settings/network', query: { tab: 'reconnect' } })
    expect(location('system.audit')).toEqual({ path: '/settings/system', query: { tab: 'retention' } })
  })
})

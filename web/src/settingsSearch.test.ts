import { beforeAll, describe, expect, it } from 'vitest'

import { SUPPORTED_LOCALES, i18n } from '@/i18n'
import { SETTINGS_SEARCH_ENTRIES, SETTINGS_SEARCH_PAGES, settingsSearchLocation } from '@/settingsSearch'
import { ROUTING_TABS, SETTINGS_SECTIONS, routingTab } from '@/settingsSections'
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

describe('the settings search registry (RD-170-15)', () => {
  it('has a row for every settings page and no row for a page that does not exist', () => {
    expect(Object.keys(SETTINGS_SEARCH_PAGES).sort()).toEqual(SETTINGS_SECTIONS.map(section => section.value).sort())
  })

  it('reaches into every settings page with at least one card or field', () => {
    const covered = new Set(SETTINGS_SEARCH_ENTRIES.map(entry => entry.section))
    expect(SETTINGS_SECTIONS.map(section => section.value).filter(value => !covered.has(value))).toEqual([])
  })

  it('reaches every routing sub-tab, and the sub-tabs are the ones the page renders', () => {
    const covered = new Set(SETTINGS_SEARCH_ENTRIES.flatMap(entry => entry.tab ? [entry.tab] : []))
    expect([...ROUTING_TABS].filter(tab => !covered.has(tab))).toEqual([])
    const routingSource = Object.entries(sources).find(([path]) => path.endsWith('/SettingsRoutingTab.vue'))?.[1] ?? ''
    const rendered = [...routingSource.matchAll(/\{ value: '([a-z]+)', slot:/g)].map(match => match[1])
    expect(rendered).toEqual([...ROUTING_TABS])
    // Only routing entries carry a sub-tab; nothing else reads it.
    expect(SETTINGS_SEARCH_ENTRIES.filter(entry => entry.tab && entry.section !== 'routing')).toEqual([])
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

  it('leads to the page and, on the routing page, to the sub-tab', () => {
    expect(settingsSearchLocation({ section: 'security' })).toEqual({ path: '/settings/security' })
    expect(settingsSearchLocation({ section: 'routing', tab: 'collector' }))
      .toEqual({ path: '/settings/routing', query: { tab: 'collector' } })
    expect(routingTab('collector')).toBe('collector')
    expect(routingTab('nonsense')).toBeNull()
    expect(routingTab(['collector'])).toBeNull()
  })
})

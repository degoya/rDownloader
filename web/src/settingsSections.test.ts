import { describe, expect, it } from 'vitest'

import {
  SETTINGS_SECTION_GROUPS,
  SETTINGS_SECTIONS,
  SETTINGS_SUB_TABS,
  settingsRedirect,
  settingsSection
} from './settingsSections'
import type { SettingsSection, SettingsSubTab } from './settingsSections'

describe('settings section navigation', () => {
  /**
   * The owner's decision of 2026-09-22 (RD-110-29, "Vorschlag A"), page for page, with the
   * decisions of 2026-10-06 (RD-1120-23): *Services* heads Sources & protocols (A), *Desktop
   * client* and *API & MCP* are one page *Clients & API* (B), and Downloads follows the way a
   * download takes — in, to its folder, after it, at what pace. The sidebar and the overview both
   * read this table, so this is the test that a page sits in the rubric it was put in; a page that
   * wanders fails here rather than in front of a reader.
   */
  it('groups the pages into the six rubrics of the decision, without losing or duplicating a route', () => {
    expect(SETTINGS_SECTION_GROUPS.map(group => [group.value, group.sections.map(section => section.value)])).toEqual([
      ['general', ['general', 'interface']],
      ['downloads', ['hotfolders', 'linkgrabber', 'routing', 'postprocess', 'bandwidth', 'unattended']],
      ['sources', ['services', 'accounts', 'captcha', 'siterules', 'usenet', 'torrent', 'media', 'transfers']],
      ['integrations', ['plugins', 'tools', 'notifications', 'clients']],
      ['network', ['network', 'security']],
      ['administration', ['backup', 'system', 'about']]
    ])

    const groupedSections = SETTINGS_SECTION_GROUPS.flatMap<SettingsSection>(group => group.sections)
    expect(SETTINGS_SECTIONS).toEqual(groupedSections)
    expect(new Set(SETTINGS_SECTIONS.map(section => section.value)).size).toBe(
      SETTINGS_SECTIONS.length
    )
  })

  it('keeps every page address from before the rubrics but the two that became one', () => {
    for (const value of [
      'system', 'backup', 'general', 'security', 'services', 'routing', 'accounts',
      'usenet', 'network', 'bandwidth', 'notifications', 'media', 'postprocess', 'plugins', 'interface'
    ]) {
      expect(settingsSection(value)).toBe(value)
    }
    expect(settingsSection('desktop')).toBeNull()
    expect(settingsSection('mcp')).toBeNull()
  })

  it('gives System an icon of its own, not the network one', () => {
    const icons = SETTINGS_SECTIONS.map(section => section.icon)
    expect(SETTINGS_SECTIONS.find(section => section.value === 'system')?.icon).not.toBe('i-lucide-network')
    expect(new Set(icons).size).toBe(icons.length)
  })

  it('names every page with its own title and description keys', () => {
    for (const section of SETTINGS_SECTIONS) {
      expect(section.labelKey).toBe(`settings.tabs.${section.value}`)
      expect(section.titleKey).toMatch(/\.title$/)
      expect(section.descriptionKey).toMatch(/\.description$/)
      expect(section.icon).toMatch(/^i-lucide-/)
    }
  })

  it('answers an unknown or missing segment with nothing rather than a guess', () => {
    expect(settingsSection('usenet')).toBe('usenet')
    expect(settingsSection('unknown')).toBeNull()
    expect(settingsSection(undefined)).toBeNull()
    expect(settingsSection(['usenet'])).toBeNull()
  })
})

describe('older settings addresses', () => {
  it('turns the `?tab=` form into the page address', () => {
    expect(settingsRedirect(undefined, 'usenet')).toBe('/settings/usenet')
    expect(settingsRedirect(undefined, 'hotfolders')).toBe('/settings/hotfolders')
  })

  it('lets `/settings` itself, and an unknown tab, render the overview', () => {
    expect(settingsRedirect(undefined, undefined)).toBeNull()
    expect(settingsRedirect(undefined, 'nonsense')).toBeNull()
  })

  it('sends an unknown segment to the overview and leaves a known one alone', () => {
    expect(settingsRedirect('nonsense', undefined)).toBe('/settings')
    expect(settingsRedirect('system', undefined)).toBeNull()
    expect(settingsRedirect('system', 'ignored')).toBeNull()
  })

  /** RD-1120-23: a page or a sub-tab that moved leads to its new place, in both address forms. */
  it('leads the pages and sub-tabs that moved to their new place', () => {
    expect(settingsRedirect('desktop', undefined)).toBe('/settings/clients')
    expect(settingsRedirect('desktop', 'anything')).toBe('/settings/clients')
    expect(settingsRedirect('mcp', undefined)).toBe('/settings/clients?tab=api')
    expect(settingsRedirect(undefined, 'desktop')).toBe('/settings/clients')
    expect(settingsRedirect(undefined, 'mcp')).toBe('/settings/clients?tab=api')
    expect(settingsRedirect('routing', 'collector')).toBe('/settings/linkgrabber?tab=blocklist')
    expect(settingsRedirect('network', 'auth')).toBe('/settings/accounts?tab=logins')
    // The tabs that stayed are left alone, and so is the new place itself.
    expect(settingsRedirect('routing', 'rules')).toBeNull()
    expect(settingsRedirect('network', 'proxies')).toBeNull()
    expect(settingsRedirect('accounts', 'logins')).toBeNull()
    expect(settingsRedirect('clients', 'api')).toBeNull()
  })
})

/** RD-1120-21: the five groups that left General went to tabs that save their other cards themselves. */
describe('where the settings document is edited', () => {
  it('marks the tabs that carry one card of the document among their own', () => {
    const tabs = Object.entries(SETTINGS_SUB_TABS).flatMap(([section, list]) => (list as readonly SettingsSubTab[])
      .filter(tab => tab.documentCard)
      .map(tab => `${section}?tab=${tab.value}`))
    expect(tabs).toEqual([
      'routing?tab=roots', 'bandwidth?tab=status', 'security?tab=signin', 'usenet?tab=servers', 'clients?tab=api'
    ])
  })

  it('splits bandwidth into status and limits, profiles and schedule, the limits first', () => {
    expect(SETTINGS_SUB_TABS.bandwidth.map(tab => tab.value)).toEqual(['status', 'profiles', 'schedule'])
  })
})

/** RD-1120-23: the pages that took cards from elsewhere, tab by tab. */
describe('the sub-tabs of the pages that were rearranged', () => {
  it('splits Usenet, Accounts and Clients & API, and takes the moved tabs off Storage & rules and Network', () => {
    expect(SETTINGS_SUB_TABS.usenet.map(tab => tab.value)).toEqual(['servers', 'indexers'])
    expect(SETTINGS_SUB_TABS.accounts.map(tab => tab.value)).toEqual(['accounts', 'logins'])
    expect(SETTINGS_SUB_TABS.clients.map(tab => tab.value)).toEqual(['desktop', 'browser', 'api'])
    expect(SETTINGS_SUB_TABS.routing.map(tab => tab.value)).toEqual(['roots', 'categories', 'rules'])
    expect(SETTINGS_SUB_TABS.network.map(tab => tab.value)).toEqual(['proxies', 'reconnect'])
  })

  // RD-1240-26: the browser's own choices save as they are made; only the tabs with fields of the
  // settings document carry the save bar, and the LinkFilter rules save themselves.
  it('splits LinkGrabber and Interface, the save bar only under document fields', () => {
    const shape = (section: 'linkgrabber' | 'interface') => (SETTINGS_SUB_TABS[section] as readonly SettingsSubTab[])
      .map(tab => [tab.value, Boolean(tab.saveBar)])
    expect(shape('linkgrabber')).toEqual([['general', true], ['blocklist', true], ['containers', true], ['filters', false]])
    expect(shape('interface')).toEqual([['browser', false], ['display', true]])
  })

  // RD-1240-26, owner: the malware scan and the package names each a tab; the tool status saves nothing.
  it('splits Post-processing and Tools, the save bar under every tab with document fields', () => {
    const shape = (section: 'postprocess' | 'tools') => (SETTINGS_SUB_TABS[section] as readonly SettingsSubTab[])
      .map(tab => [tab.value, Boolean(tab.saveBar)])
    expect(shape('postprocess')).toEqual([['unpack', true], ['repair', true], ['names', true], ['malware', true], ['delivery', true]])
    expect(shape('tools')).toEqual([['status', false], ['paths', true], ['managed', true]])
  })
})

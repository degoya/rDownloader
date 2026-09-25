import { describe, expect, it } from 'vitest'

import {
  SETTINGS_SECTION_GROUPS,
  SETTINGS_SECTIONS,
  settingsRedirect,
  settingsSection
} from './settingsSections'
import type { SettingsSection } from './settingsSections'

describe('settings section navigation', () => {
  /**
   * The owner's decision of 2026-09-22 (RD-110-29, "Vorschlag A"), page for page. The sidebar
   * and the overview both read this table, so this is the test that a page sits in the rubric
   * it was put in; a page that wanders fails here rather than in front of a reader.
   */
  it('groups the pages into the six rubrics of the decision, without losing or duplicating a route', () => {
    expect(SETTINGS_SECTION_GROUPS.map(group => [group.value, group.sections.map(section => section.value)])).toEqual([
      ['general', ['general', 'interface', 'desktop']],
      ['downloads', ['routing', 'hotfolders', 'bandwidth', 'unattended', 'postprocess']],
      ['sources', ['accounts', 'captcha', 'siterules', 'usenet', 'torrent', 'media', 'transfers']],
      ['integrations', ['services', 'plugins', 'tools', 'notifications', 'mcp']],
      ['network', ['network', 'security']],
      ['administration', ['backup', 'system', 'about']]
    ])

    const groupedSections = SETTINGS_SECTION_GROUPS.flatMap<SettingsSection>(group => group.sections)
    expect(SETTINGS_SECTIONS).toEqual(groupedSections)
    expect(new Set(SETTINGS_SECTIONS.map(section => section.value)).size).toBe(
      SETTINGS_SECTIONS.length
    )
  })

  it('keeps every page address from before the rubrics', () => {
    for (const value of [
      'system', 'desktop', 'backup', 'general', 'security', 'services', 'routing', 'accounts',
      'usenet', 'network', 'bandwidth', 'notifications', 'media', 'postprocess', 'plugins', 'mcp', 'interface'
    ]) {
      expect(settingsSection(value)).toBe(value)
    }
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
})

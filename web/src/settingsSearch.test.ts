import { beforeAll, describe, expect, it } from 'vitest'

import { SUPPORTED_LOCALES, i18n } from '@/i18n'
import { MOVED_SETTINGS_ANCHORS, SETTINGS_SEARCH_ENTRIES, SETTINGS_SEARCH_PAGES, settingsSearchEntry, settingsSearchLocation } from '@/settingsSearch'
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

/** Pages whose component does not follow the `Settings<Page>Tab.vue` name. */
const PAGE_COMPONENTS: Partial<Record<string, string>> = { backup: 'SettingsBackupRestore' }

/** The component that renders a settings page with sub-tabs: `routing` → `SettingsRoutingTab.vue`. */
function pageSource(section: string): string {
  const file = `/${PAGE_COMPONENTS[section] ?? `Settings${section.charAt(0).toUpperCase()}${section.slice(1)}Tab`}.vue`
  return Object.entries(sources).find(([path]) => path.endsWith(file))?.[1] ?? ''
}

/** A page's `UTabs` slots by name, each with the template it renders, in source order. */
function tabSlots(section: string): Record<string, string> {
  const source = pageSource(section)
  const tabs = source.slice(source.indexOf('<UTabs'), source.indexOf('</UTabs>'))
  return Object.fromEntries([...tabs.matchAll(/^( *)<template #([a-z0-9]+)>\n([\s\S]*?)^\1<\/template>/gm)]
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
    expect(settingsSearchLocation({ section: 'general' })).toEqual({ path: '/settings/general' })
    expect(settingsSearchLocation({ section: 'routing', tab: 'rules' }))
      .toEqual({ path: '/settings/routing', query: { tab: 'rules' } })
    expect(settingsSubTab('routing', 'rules')).toBe('rules')
    expect(settingsSubTab('routing', 'nonsense')).toBeNull()
    expect(settingsSubTab('routing', ['rules'])).toBeNull()
    // A tab of another page is not a tab of this one.
    expect(settingsSubTab('plugins', 'rules')).toBeNull()
  })

  // RD-1120-21: five groups left General; an old anchor still leads to its field.
  it('leads every moved anchor to its field at the new place, and keeps the old id off the templates', () => {
    const anchors = anchorsInSources()
    for (const [old, current] of Object.entries(MOVED_SETTINGS_ANCHORS)) {
      expect(settingsSearchEntry(old)?.id, old).toBe(current)
      expect(anchors, old).not.toContain(old)
      expect(anchors.filter(anchor => anchor === current), current).toHaveLength(1)
      expect(SETTINGS_SEARCH_ENTRIES.some(entry => entry.id === old), old).toBe(false)
    }
    expect(settingsSearchEntry('general.retries')?.id).toBe('general.retries')
    expect(settingsSearchEntry('general.nonsense')).toBeNull()
  })

  it('opens the fields that left General on their new page and tab', () => {
    const location = (id: string) => {
      const entry = settingsSearchEntry(id)
      return entry ? settingsSearchLocation(entry) : null
    }
    expect(location('general.admin_login')).toEqual({ path: '/settings/security', query: { tab: 'signin' } })
    expect(location('general.ui_port')).toEqual({ path: '/settings/security', query: { tab: 'proxy' } })
    expect(location('general.minimum_free')).toEqual({ path: '/settings/routing', query: { tab: 'roots' } })
    expect(location('general.collision')).toEqual({ path: '/settings/routing', query: { tab: 'roots' } })
    expect(location('general.speed_limit')).toEqual({ path: '/settings/bandwidth', query: { tab: 'status' } })
    expect(location('usenet.nntp_connections')).toEqual({ path: '/settings/usenet', query: { tab: 'servers' } })
    expect(location('bandwidth.upload_limit')).toEqual({ path: '/settings/bandwidth', query: { tab: 'status' } })
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

  // RD-1120-23: the cards that changed page, by their old anchor.
  it('opens the cards that moved by topic on their new page and tab', () => {
    const location = (id: string) => {
      const entry = settingsSearchEntry(id)
      return entry ? settingsSearchLocation(entry) : null
    }
    expect(location('routing.collector')).toEqual({ path: '/settings/linkgrabber', query: { tab: 'blocklist' } })
    expect(location('routing.dlc')).toEqual({ path: '/settings/linkgrabber', query: { tab: 'containers' } })
    expect(location('general.mirrors')).toEqual({ path: '/settings/linkgrabber', query: { tab: 'general' } })
    expect(location('routing.indexer_images')).toEqual({ path: '/settings/interface', query: { tab: 'display' } })
    expect(location('routing.nzb_hand_over')).toEqual({ path: '/settings/interface', query: { tab: 'display' } })
    expect(location('network.auth_profiles')).toEqual({ path: '/settings/accounts', query: { tab: 'logins' } })
    expect(location('desktop.pairing')).toEqual({ path: '/settings/clients', query: { tab: 'desktop' } })
    expect(location('mcp.access')).toEqual({ path: '/settings/clients', query: { tab: 'api' } })
    expect(location('clients.browser')).toEqual({ path: '/settings/clients', query: { tab: 'browser' } })
    expect(location('system.import_history')).toEqual({ path: '/settings/system', query: { tab: 'retention' } })
    expect(location('usenet.indexers')).toEqual({ path: '/settings/usenet', query: { tab: 'indexers' } })
  })

  // RD-1160-01: three long pages split by subject; the search and every cross link open the tab.
  it('opens the cards of Notifications, Backup & restore and About on their tab', () => {
    const location = (id: string) => {
      const entry = settingsSearchEntry(id)
      return entry ? settingsSearchLocation(entry) : null
    }
    expect(location('notifications.targets')).toEqual({ path: '/settings/notifications', query: { tab: 'targets' } })
    expect(location('notifications.rules')).toEqual({ path: '/settings/notifications', query: { tab: 'targets' } })
    expect(location('notifications.history')).toEqual({ path: '/settings/notifications', query: { tab: 'history' } })
    expect(location('backup.import')).toEqual({ path: '/settings/backup', query: { tab: 'config' } })
    expect(location('backup.schedule')).toEqual({ path: '/settings/backup', query: { tab: 'full' } })
    expect(location('backup.full_restore')).toEqual({ path: '/settings/backup', query: { tab: 'restore' } })
    expect(location('about.build')).toEqual({ path: '/settings/about', query: { tab: 'about' } })
    expect(location('about.licenses')).toEqual({ path: '/settings/about', query: { tab: 'licenses' } })
  })

  // RD-1240-26: LinkGrabber and Interface split; every card and field opens its tab.
  it('opens the cards and fields of LinkGrabber and Interface on their tab', () => {
    const tabs = (section: string) => Object.fromEntries(SETTINGS_SEARCH_ENTRIES
      .filter(entry => entry.section === section)
      .map(entry => [entry.id, settingsSearchLocation(entry).query?.tab]))
    expect(tabs('linkgrabber')).toEqual({
      'linkgrabber.blocklist': 'blocklist',
      'linkgrabber.excluded_domains': 'blocklist',
      'linkgrabber.dlc': 'containers',
      'linkgrabber.mirrors': 'general',
      'linkgrabber.duplicates_history': 'general',
      'linkgrabber.link_filters': 'filters'
    })
    expect(tabs('interface')).toEqual({
      'interface.appearance': 'browser',
      'interface.language': 'browser',
      'interface.theme': 'browser',
      'interface.palette': 'browser',
      'interface.browser_notifications': 'browser',
      'interface.web_push': 'browser',
      'interface.display': 'display',
      'interface.byte_display': 'display',
      'interface.title_status': 'display',
      'interface.indexer_images': 'display',
      'interface.nzb_hand_over': 'display',
      'interface.package_groups': 'display'
    })
  })

  it('opens the fields of Post-processing and Tools on their tab, the moved program paths with them', () => {
    const tabs = (section: string) => Object.fromEntries(SETTINGS_SEARCH_ENTRIES
      .filter(entry => entry.section === section)
      .map(entry => [entry.id, settingsSearchLocation(entry).query?.tab]))
    expect(tabs('postprocess')).toEqual({
      'postprocess.defaults': 'unpack',
      'postprocess.passwords_file': 'unpack',
      'postprocess.unpack_to_subfolder': 'unpack',
      'postprocess.unwrap_package_folder': 'unpack',
      'postprocess.direct_unpack': 'unpack',
      'postprocess.delete_par2': 'repair',
      'postprocess.cleanup_extensions': 'repair',
      'postprocess.scripts_directory': 'delivery',
      'postprocess.mcp_scripts_allowed': 'delivery',
      'postprocess.package_names': 'names',
      'postprocess.malware_scan': 'malware',
      'postprocess.upload': 'delivery'
    })
    expect(tabs('tools')).toEqual({
      'tools.status': 'status',
      'tools.paths': 'paths',
      'postprocess.rar_executable': 'paths',
      'postprocess.rclone_executable': 'paths',
      'tools.vendor_directory': 'paths',
      'tools.managed': 'managed'
    })
  })
})

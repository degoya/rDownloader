/**
 * The settings pages, grouped in the order they appear in the sidebar and on the overview.
 *
 * One source for four consumers: the sub-routes under `/settings`, the sidebar's expandable
 * group, the overview page's cards, and the view that renders the active page. They used to be
 * two lists in `SettingsView.vue` that drifted apart — deep links to `bandwidth` and
 * `notifications` were rejected because only one of the lists knew about them.
 *
 * The six rubrics are the owner's decision of 2026-09-22 (RD-110-29, "Vorschlag A"): a page
 * sits where somebody looks for it — tools with the integrations, BitTorrent beside Usenet,
 * unattended operation not under bandwidth. `settingsSections.test.ts` holds this table to
 * that decision, so a page that wanders into another rubric fails a test rather than a reader.
 */
export interface SettingsSection {
  /** Route segment and section id, e.g. `/settings/usenet`. */
  value: string
  /** i18n key under `settings.tabs`. */
  labelKey: string
  icon: string
  /** i18n key of the page header's title, which the overview card repeats. */
  titleKey: string
  /** i18n key of the page header's description, which the overview card repeats. */
  descriptionKey: string
}

export interface SettingsSectionGroup {
  /** Stable id used by tests and navigation keys. */
  value: string
  /** i18n key under `settings.groups`. */
  labelKey: string
  sections: readonly SettingsSection[]
}

/** A page whose header lives under `settings.headers.<value>`, which is most of them. */
function page(value: string, icon: string): SettingsSection {
  return {
    value,
    labelKey: `settings.tabs.${value}`,
    icon,
    titleKey: `settings.headers.${value}.title`,
    descriptionKey: `settings.headers.${value}.description`
  }
}

/** A page whose header keys sit in the catalogue of its own domain. */
function domainPage(value: string, icon: string, domain: string): SettingsSection {
  return {
    ...page(value, icon),
    titleKey: `${domain}.header.title`,
    descriptionKey: `${domain}.header.description`
  }
}

export const SETTINGS_SECTION_GROUPS = [
  {
    value: 'general',
    labelKey: 'settings.groups.general',
    sections: [
      page('general', 'i-lucide-sliders-horizontal'),
      page('interface', 'i-lucide-monitor-cog'),
      page('desktop', 'i-lucide-monitor-smartphone')
    ]
  },
  {
    value: 'downloads',
    labelKey: 'settings.groups.downloads',
    sections: [
      domainPage('routing', 'i-lucide-folder-tree', 'routing'),
      page('hotfolders', 'i-lucide-folder-symlink'),
      page('bandwidth', 'i-lucide-gauge'),
      page('unattended', 'i-lucide-moon-star'),
      page('postprocess', 'i-lucide-workflow')
    ]
  },
  {
    value: 'sources',
    labelKey: 'settings.groups.sources',
    sections: [
      page('accounts', 'i-lucide-key-round'),
      page('captcha', 'i-lucide-scan-eye'),
      domainPage('siterules', 'i-lucide-scan-search', 'siterules'),
      domainPage('usenet', 'i-lucide-radio-tower', 'usenet'),
      page('torrent', 'i-lucide-share-2'),
      page('media', 'i-lucide-clapperboard'),
      page('transfers', 'i-lucide-server')
    ]
  },
  {
    value: 'integrations',
    labelKey: 'settings.groups.integrations',
    sections: [
      page('services', 'i-lucide-toggle-left'),
      domainPage('plugins', 'i-lucide-blocks', 'plugins'),
      page('tools', 'i-lucide-wrench'),
      page('notifications', 'i-lucide-bell'),
      page('mcp', 'i-lucide-bot')
    ]
  },
  {
    value: 'network',
    labelKey: 'settings.groups.network',
    sections: [
      page('network', 'i-lucide-waypoints'),
      page('security', 'i-lucide-shield-check')
    ]
  },
  {
    value: 'administration',
    labelKey: 'settings.groups.administration',
    sections: [
      page('backup', 'i-lucide-database-backup'),
      page('system', 'i-lucide-network'),
      page('about', 'i-lucide-info')
    ]
  }
] as const satisfies readonly SettingsSectionGroup[]

export const SETTINGS_SECTIONS: readonly SettingsSection[] = SETTINGS_SECTION_GROUPS.flatMap<SettingsSection>(
  group => group.sections
)

/** Every page's segment as a type, so a table keyed by page has to name all of them. */
export type SettingsSectionValue = (typeof SETTINGS_SECTION_GROUPS)[number]['sections'][number]['value']

export interface SettingsSubTab {
  /** The `?tab=` value and the name of the page's slot for it. */
  value: string
  labelKey: string
  icon: string
  /** The tab edits the settings document, so the page's save bar belongs under it. */
  saveBar?: true
}

/**
 * The pages split into sub-tabs, and their tabs in order (RD-180-15). A page with more than five
 * cards gets them — `design.md` has the rule and how the cards are counted; a page not in this
 * table has none. `/settings/plugins?tab=updates` opens one directly, which is how the search
 * and every other link reach a card on a tab that is not the first (RD-170-15).
 */
export const SETTINGS_SUB_TABS = {
  routing: [
    { value: 'roots', labelKey: 'routing.tabs.roots', icon: 'i-lucide-hard-drive' },
    { value: 'categories', labelKey: 'routing.tabs.categories', icon: 'i-lucide-folder-tree' },
    { value: 'rules', labelKey: 'routing.tabs.rules', icon: 'i-lucide-git-branch' },
    { value: 'collector', labelKey: 'routing.tabs.collector', icon: 'i-lucide-shield-ban', saveBar: true }
  ],
  plugins: [
    { value: 'installed', labelKey: 'plugins.tabs.installed', icon: 'i-lucide-blocks' },
    { value: 'add', labelKey: 'plugins.tabs.add', icon: 'i-lucide-package-plus' },
    { value: 'updates', labelKey: 'plugins.tabs.updates', icon: 'i-lucide-refresh-cw' },
    { value: 'repositories', labelKey: 'plugins.tabs.repositories', icon: 'i-lucide-library' },
    { value: 'trust', labelKey: 'plugins.tabs.trust', icon: 'i-lucide-badge-check' }
  ],
  network: [
    { value: 'proxies', labelKey: 'settings.subtabs.network.proxies', icon: 'i-lucide-waypoints', saveBar: true },
    { value: 'auth', labelKey: 'settings.subtabs.network.auth', icon: 'i-lucide-key-square' },
    { value: 'reconnect', labelKey: 'settings.subtabs.network.reconnect', icon: 'i-lucide-router', saveBar: true }
  ],
  security: [
    { value: 'signin', labelKey: 'settings.subtabs.security.signin', icon: 'i-lucide-key-round' },
    { value: 'sessions', labelKey: 'settings.subtabs.security.sessions', icon: 'i-lucide-monitor-smartphone', saveBar: true },
    { value: 'proxy', labelKey: 'settings.subtabs.security.proxy', icon: 'i-lucide-shield', saveBar: true }
  ],
  system: [
    { value: 'status', labelKey: 'settings.subtabs.system.status', icon: 'i-lucide-activity' },
    { value: 'updates', labelKey: 'settings.subtabs.system.updates', icon: 'i-lucide-refresh-cw', saveBar: true },
    { value: 'retention', labelKey: 'settings.subtabs.system.retention', icon: 'i-lucide-archive', saveBar: true }
  ]
} as const satisfies Partial<Record<SettingsSectionValue, readonly SettingsSubTab[]>>

export type SettingsSubTabSection = keyof typeof SETTINGS_SUB_TABS
/** Every sub-tab value of every page; which page each belongs to, the table and its test say. */
export type SettingsSubTabValue = (typeof SETTINGS_SUB_TABS)[SettingsSubTabSection][number]['value']

/** A page's sub-tabs, or none. */
export function settingsSubTabs(section: string): readonly SettingsSubTab[] {
  return (SETTINGS_SUB_TABS as Partial<Record<string, readonly SettingsSubTab[]>>)[section] ?? []
}

/** The sub-tab of this page a query value names, or null. */
export function settingsSubTab(section: string, value: unknown): string | null {
  return typeof value === 'string' && settingsSubTabs(section).some(tab => tab.value === value) ? value : null
}

/** The page a segment names, or null: an unknown segment belongs on the overview, not on a guess. */
export function settingsSection(value: unknown): string | null {
  return typeof value === 'string' && SETTINGS_SECTIONS.some(section => section.value === value)
    ? value
    : null
}

/**
 * Where an older address leads, or null when it needs no redirect.
 *
 * Two forms are kept alive for bookmarks and for the links other views hold: `/settings?tab=usenet`,
 * which every link used before the pages had addresses of their own, and a segment that names
 * no page, which goes to the overview rather than to an arbitrary page. Every segment that
 * existed before the six rubrics still exists, so no old page name has to be mapped to a new one.
 */
export function settingsRedirect(section: unknown, tab: unknown): string | null {
  if (section === undefined) {
    const target = settingsSection(tab)
    return target ? `/settings/${target}` : null
  }
  return settingsSection(section) ? null : '/settings'
}

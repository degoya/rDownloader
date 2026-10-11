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
 * unattended operation not under bandwidth. RD-1120-23 moved *Services* to the head of Sources &
 * protocols, joined *Desktop client* and *API & MCP* into *Clients & API* and gave the LinkGrabber
 * a page of its own (owner, 2026-10-06, decisions A and B). `settingsSections.test.ts` holds this
 * table to those decisions, so a page that wanders into another rubric fails a test rather than a
 * reader.
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

interface SettingsSectionGroup {
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
      page('interface', 'i-lucide-monitor-cog')
    ]
  },
  {
    value: 'downloads',
    labelKey: 'settings.groups.downloads',
    // The way a download takes (RD-1120-23): in, to its folder, after it, and at what pace.
    sections: [
      page('hotfolders', 'i-lucide-folder-symlink'),
      page('linkgrabber', 'i-lucide-link'),
      domainPage('routing', 'i-lucide-folder-tree', 'routing'),
      page('postprocess', 'i-lucide-workflow'),
      page('bandwidth', 'i-lucide-gauge'),
      page('unattended', 'i-lucide-moon-star')
    ]
  },
  {
    value: 'sources',
    labelKey: 'settings.groups.sources',
    sections: [
      page('services', 'i-lucide-toggle-left'),
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
      domainPage('plugins', 'i-lucide-blocks', 'plugins'),
      page('tools', 'i-lucide-wrench'),
      page('notifications', 'i-lucide-bell'),
      page('clients', 'i-lucide-monitor-smartphone')
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
      page('system', 'i-lucide-cpu'),
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
  /**
   * The tab shows values of the settings document without editing them; like a `saveBar` tab it
   * waits for the document rather than showing its placeholders (RA-WEB-05).
   */
  showsDocument?: true
  /**
   * The tab saves its own cards and carries one card of the settings document (RD-1120-21): the
   * save bar belongs under it, but the tab does not wait for the document — that card does, in
   * `SettingsDocumentGate` — so its own lists stay usable when the document cannot be loaded.
   */
  documentCard?: true
}

/**
 * The pages split into sub-tabs, and their tabs in order (RD-180-15). A page with more than five
 * cards gets them — `design.md` has the rule and how the cards are counted; a page not in this
 * table has none. `/settings/plugins?tab=updates` opens one directly, which is how the search
 * and every other link reach a card on a tab that is not the first (RD-170-15).
 */
export const SETTINGS_SUB_TABS = {
  routing: [
    { value: 'roots', labelKey: 'routing.tabs.roots', icon: 'i-lucide-hard-drive', documentCard: true },
    { value: 'categories', labelKey: 'routing.tabs.categories', icon: 'i-lucide-folder-tree' },
    { value: 'rules', labelKey: 'routing.tabs.rules', icon: 'i-lucide-git-branch' }
  ],
  // RD-1240-26: four cards, one subject each; the two switches that apply to every link first.
  linkgrabber: [
    { value: 'general', labelKey: 'settings.subtabs.linkgrabber.general', icon: 'i-lucide-sliders-horizontal', saveBar: true },
    { value: 'blocklist', labelKey: 'settings.subtabs.linkgrabber.blocklist', icon: 'i-lucide-shield-ban', saveBar: true },
    { value: 'containers', labelKey: 'settings.subtabs.linkgrabber.containers', icon: 'i-lucide-package-open', saveBar: true },
    { value: 'filters', labelKey: 'settings.subtabs.linkgrabber.filters', icon: 'i-lucide-filter' }
  ],
  // RD-1240-26: the pipeline card had some twenty-five fields; the malware scan and the package
  // names are tabs of their own (owner, 2026-10-10), the rest split in the order a package meets it.
  postprocess: [
    { value: 'unpack', labelKey: 'settings.subtabs.postprocess.unpack', icon: 'i-lucide-package-open', saveBar: true },
    { value: 'repair', labelKey: 'settings.subtabs.postprocess.repair', icon: 'i-lucide-wrench', saveBar: true },
    { value: 'names', labelKey: 'settings.subtabs.postprocess.names', icon: 'i-lucide-text-cursor-input', saveBar: true },
    { value: 'malware', labelKey: 'settings.subtabs.postprocess.malware', icon: 'i-lucide-shield-check', saveBar: true },
    { value: 'delivery', labelKey: 'settings.subtabs.postprocess.delivery', icon: 'i-lucide-cloud-upload', saveBar: true }
  ],
  // RD-1240-26: what was found, where it is looked up, and the versions the service installs.
  // The status loads and acts on its own, so it has no save bar and does not wait.
  tools: [
    { value: 'status', labelKey: 'settings.subtabs.tools.status', icon: 'i-lucide-activity' },
    { value: 'paths', labelKey: 'settings.subtabs.tools.paths', icon: 'i-lucide-folder-tree', saveBar: true },
    { value: 'managed', labelKey: 'settings.subtabs.tools.managed', icon: 'i-lucide-package-check', saveBar: true }
  ],
  bandwidth: [
    { value: 'status', labelKey: 'settings.subtabs.bandwidth.status', icon: 'i-lucide-gauge', documentCard: true },
    { value: 'profiles', labelKey: 'settings.subtabs.bandwidth.profiles', icon: 'i-lucide-sliders-horizontal' },
    { value: 'schedule', labelKey: 'settings.subtabs.bandwidth.schedule', icon: 'i-lucide-calendar-clock' }
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
    { value: 'reconnect', labelKey: 'settings.subtabs.network.reconnect', icon: 'i-lucide-router', saveBar: true }
  ],
  security: [
    { value: 'signin', labelKey: 'settings.subtabs.security.signin', icon: 'i-lucide-key-round', documentCard: true },
    { value: 'sessions', labelKey: 'settings.subtabs.security.sessions', icon: 'i-lucide-monitor-smartphone', saveBar: true },
    { value: 'proxy', labelKey: 'settings.subtabs.security.proxy', icon: 'i-lucide-shield', saveBar: true }
  ],
  system: [
    { value: 'status', labelKey: 'settings.subtabs.system.status', icon: 'i-lucide-activity', showsDocument: true },
    { value: 'updates', labelKey: 'settings.subtabs.system.updates', icon: 'i-lucide-refresh-cw', saveBar: true },
    { value: 'retention', labelKey: 'settings.subtabs.system.retention', icon: 'i-lucide-archive', saveBar: true }
  ],
  usenet: [
    { value: 'servers', labelKey: 'usenet.tabs.servers', icon: 'i-lucide-server', documentCard: true },
    { value: 'indexers', labelKey: 'usenet.tabs.indexers', icon: 'i-lucide-search' }
  ],
  // Fewer than six cards, split by service (RD-1160-01): each service has its own "off" notice.
  media: [
    { value: 'media', labelKey: 'settings.subtabs.media.media', icon: 'i-lucide-clapperboard', saveBar: true },
    { value: 'galleries', labelKey: 'settings.subtabs.media.galleries', icon: 'i-lucide-images', saveBar: true },
    { value: 'streams', labelKey: 'settings.subtabs.media.streams', icon: 'i-lucide-radio', saveBar: true }
  ],
  transfers: [
    { value: 'remote', labelKey: 'settings.subtabs.transfers.remote', icon: 'i-lucide-server', saveBar: true },
    { value: 's3', labelKey: 'settings.subtabs.transfers.s3', icon: 'i-lucide-cylinder' }
  ],
  accounts: [
    { value: 'accounts', labelKey: 'settings.subtabs.accounts.accounts', icon: 'i-lucide-key-round' },
    { value: 'logins', labelKey: 'settings.subtabs.accounts.logins', icon: 'i-lucide-key-square' }
  ],
  clients: [
    { value: 'desktop', labelKey: 'settings.subtabs.clients.desktop', icon: 'i-lucide-monitor' },
    { value: 'browser', labelKey: 'settings.subtabs.clients.browser', icon: 'i-lucide-puzzle' },
    { value: 'api', labelKey: 'settings.subtabs.clients.api', icon: 'i-lucide-bot', documentCard: true }
  ],
  // RD-1240-26: two subjects, what this browser keeps and what every browser shares. The first
  // edits nothing of the settings document, so it has no save bar and does not wait for it.
  interface: [
    { value: 'browser', labelKey: 'settings.subtabs.interface.browser', icon: 'i-lucide-app-window' },
    { value: 'display', labelKey: 'settings.subtabs.interface.display', icon: 'i-lucide-monitor', saveBar: true }
  ],
  // RD-1160-01: three pages split by their subjects rather than by a sixth card.
  notifications: [
    { value: 'targets', labelKey: 'settings.subtabs.notifications.targets', icon: 'i-lucide-send' },
    { value: 'history', labelKey: 'settings.subtabs.notifications.history', icon: 'i-lucide-history' }
  ],
  backup: [
    { value: 'config', labelKey: 'settings.subtabs.backup.config', icon: 'i-lucide-file-json-2' },
    { value: 'full', labelKey: 'settings.subtabs.backup.full', icon: 'i-lucide-database-backup' },
    { value: 'restore', labelKey: 'settings.subtabs.backup.restore', icon: 'i-lucide-archive-restore' }
  ],
  about: [
    { value: 'about', labelKey: 'settings.subtabs.about.about', icon: 'i-lucide-info' },
    { value: 'licenses', labelKey: 'settings.subtabs.about.licenses', icon: 'i-lucide-scale' }
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
 * Pages and sub-tabs that moved (RD-1120-23), old address to new. Keyed by the segment, or by
 * `segment?tab=value` where only one sub-tab left its page.
 */
const MOVED_SETTINGS_PAGES: Readonly<Record<string, string>> = {
  desktop: '/settings/clients',
  mcp: '/settings/clients?tab=api',
  'routing?tab=collector': '/settings/linkgrabber?tab=blocklist',
  'network?tab=auth': '/settings/accounts?tab=logins'
}

/**
 * Where an older address leads, or null when it needs no redirect.
 *
 * Three forms are kept alive for bookmarks and for the links other views hold: `/settings?tab=usenet`,
 * which every link used before the pages had addresses of their own; a page or a sub-tab that
 * moved (`MOVED_SETTINGS_PAGES`); and a segment that names no page, which goes to the overview
 * rather than to an arbitrary page.
 */
export function settingsRedirect(section: unknown, tab: unknown): string | null {
  if (section === undefined) {
    if (typeof tab === 'string' && MOVED_SETTINGS_PAGES[tab]) return MOVED_SETTINGS_PAGES[tab]
    const target = settingsSection(tab)
    return target ? `/settings/${target}` : null
  }
  if (typeof section === 'string') {
    const moved = (typeof tab === 'string' ? MOVED_SETTINGS_PAGES[`${section}?tab=${tab}`] : undefined)
      ?? MOVED_SETTINGS_PAGES[section]
    if (moved) return moved
  }
  return settingsSection(section) ? null : '/settings'
}

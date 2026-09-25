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

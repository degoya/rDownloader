import type { CommandPaletteGroup, CommandPaletteItem } from '@nuxt/ui'
import { ref } from 'vue'

import { router } from '@/router'
import { SETTINGS_SEARCH_ENTRIES, SETTINGS_SEARCH_PAGES, type SettingsSearchEntry, type SettingsSearchPage, settingsSearchEntry, settingsSearchLocation } from '@/settingsSearch'
import { SETTINGS_SECTION_GROUPS, SETTINGS_SECTIONS } from '@/settingsSections'
import { revealAnchor } from '@/utils/revealAnchor'

/**
 * The search palette's content and state (RD-170-15), kept free of `@nuxt/ui/composables` for
 * the same reason `shortcutDefinitions.ts` is: that barrel does not load under Vitest, and this
 * is what the tests exercise. `SearchPalette.vue` renders it in Nuxt UI's `UDashboardSearch`,
 * which binds Ctrl/Cmd+K itself; `/` opens it through the shortcut catalogue.
 */
export const paletteOpen = ref(false)

export function openPalette(): void {
  paletteOpen.value = true
}

/**
 * Set while a found field is about to take the focus. Closing a dialog hands the focus back to
 * whatever had it before — the search button, or the page — and would take it straight out of
 * the field again; `SearchPalette.vue` passes this to the modal's `onCloseAutoFocus`.
 */
let focusHandedOver = false

export function keepFocusOnClose(event: Event): void {
  if (!focusHandedOver) return
  focusHandedOver = false
  event.preventDefault()
}

/**
 * The palette searches `label`, `description` and `suffix` by default; `keywords` carries the
 * synonyms and the names that are the same in every language. Nuxt UI merges this with its own
 * options (`defu` appends arrays), so only the extra key is named here.
 */
export const PALETTE_FUSE = { fuseOptions: { keys: ['keywords'] } }

/**
 * The main views in sidebar order, with the key that opens each (`shortcutDefinitions.ts`); the
 * history, the statistics page's second tab (RD-1101-05), right after the page.
 */
export const MAIN_VIEWS = [
  { path: '/downloads', labelKey: 'nav.downloads', icon: 'i-lucide-arrow-down-to-line', key: '1' },
  { path: '/linkgrabber', labelKey: 'nav.linkgrabber', icon: 'i-lucide-magnet', key: '2' },
  { path: '/streams', labelKey: 'nav.streams', icon: 'i-lucide-radio', key: '3' },
  { path: '/subscriptions', labelKey: 'nav.subscriptions', icon: 'i-lucide-rss', key: '4' },
  { path: '/remote-jobs', labelKey: 'nav.remote_jobs', icon: 'i-lucide-cloud-cog', key: '5' },
  { path: '/automation', labelKey: 'nav.automation', icon: 'i-lucide-workflow', key: '6' },
  { path: '/stats', labelKey: 'nav.stats_history', icon: 'i-lucide-chart-column', key: '7' },
  { path: '/stats?tab=history', labelKey: 'nav.history', icon: 'i-lucide-history', key: 'h' },
  { path: '/logs', labelKey: 'nav.logs', icon: 'i-lucide-scroll-text', key: '8' },
  { path: '/audit', labelKey: 'nav.audit', icon: 'i-lucide-shield-check', key: '9' },
  { path: '/settings', labelKey: 'nav.settings', icon: 'i-lucide-sliders-horizontal', key: '0' }
] as const

type Translate = (key: string) => string

function keywords(t: Translate, keywordsKey: string | undefined, terms: readonly string[] | undefined, extra: string[] = []): string {
  return [...extra, ...(keywordsKey ? [t(keywordsKey)] : []), ...(terms ?? [])].join(', ')
}

/** Opens the entry's page (and sub-tab), then scrolls to it; a field also takes the focus. */
export async function openSettingsEntry(entry: SettingsSearchEntry): Promise<boolean> {
  focusHandedOver = entry.kind === 'field'
  await router.push(settingsSearchLocation(entry))
  return revealAnchor(entry.id, { focus: entry.kind === 'field' })
}

/** Opens the setting an anchor id names; an anchor that moved leads to the new place (RD-1120-21). */
export async function openSettingsAnchor(id: string): Promise<boolean> {
  const entry = settingsSearchEntry(id)
  return entry ? openSettingsEntry(entry) : false
}

export function buildPaletteGroups(t: Translate): CommandPaletteGroup<CommandPaletteItem>[] {
  const sectionOf = new Map(SETTINGS_SECTIONS.map(section => [section.value, section]))
  const groupOf = new Map(SETTINGS_SECTION_GROUPS.flatMap(group => group.sections.map(section => [section.value, group.labelKey])))

  const views: CommandPaletteItem[] = MAIN_VIEWS.map(view => ({
    id: `view:${view.path}`,
    label: t(view.labelKey),
    icon: view.icon,
    kbds: [view.key],
    onSelect: () => { void router.push(view.path) }
  }))

  const pages: CommandPaletteItem[] = SETTINGS_SECTIONS.map((section) => {
    const search: SettingsSearchPage = SETTINGS_SEARCH_PAGES[section.value as keyof typeof SETTINGS_SEARCH_PAGES] ?? {}
    const group = groupOf.get(section.value)
    return {
      id: `page:${section.value}`,
      label: t(section.labelKey),
      description: t(section.descriptionKey),
      ...(group ? { suffix: t(group) } : {}),
      icon: section.icon,
      // The page header's title often says more than the short navigation label.
      keywords: keywords(t, search.keywordsKey, search.terms, [t(section.titleKey)]),
      onSelect: () => { void router.push(`/settings/${section.value}`) }
    }
  })

  const settings: CommandPaletteItem[] = SETTINGS_SEARCH_ENTRIES.map((entry) => {
    const section = sectionOf.get(entry.section)
    return {
      id: `setting:${entry.id}`,
      label: t(entry.titleKey),
      ...(entry.descriptionKey ? { description: t(entry.descriptionKey) } : {}),
      ...(section ? { suffix: t(section.labelKey) } : {}),
      icon: entry.kind === 'field' ? 'i-lucide-text-cursor-input' : section?.icon ?? 'i-lucide-settings',
      keywords: keywords(t, entry.keywordsKey, entry.terms),
      onSelect: () => { void openSettingsEntry(entry) }
    }
  })

  return [
    { id: 'views', label: t('nav.search.groups.views'), items: views },
    { id: 'pages', label: t('nav.search.groups.pages'), items: pages },
    { id: 'settings', label: t('nav.search.groups.settings'), items: settings }
  ]
}

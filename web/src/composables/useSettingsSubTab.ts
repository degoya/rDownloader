import type { TabsItem } from '@nuxt/ui'
import { computed, type ComputedRef, type WritableComputedRef } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { type SettingsSubTab, type SettingsSubTabSection, SETTINGS_SUB_TABS, settingsSubTab, settingsSubTabs } from '@/settingsSections'

/**
 * The sub-tab of a settings page, held in the address as `?tab=` (RD-180-15).
 *
 * The address is the only state: the getter reads the query, the setter pushes a new one. So a
 * reload, a bookmark, back and forward, the search (RD-170-15) and any other link that names a
 * tab all land on it without a second copy to keep in step. A value the page does not have —
 * mistyped, or a tab of another page — shows the first tab rather than an empty panel. The first
 * tab carries no query, so the plain page address stays the plain address.
 *
 * A push rather than a replace: a tab is a place somebody went to, and back should return from it.
 */
export function useSettingsSubTab(section: () => string): {
  tabs: ComputedRef<readonly SettingsSubTab[]>
  active: WritableComputedRef<string>
} {
  const route = useRoute()
  const router = useRouter()
  const tabs = computed(() => settingsSubTabs(section()))
  const active = computed<string>({
    get: () => settingsSubTab(section(), route.query.tab) ?? tabs.value[0]?.value ?? '',
    set: (value) => {
      if (value === active.value || !settingsSubTab(section(), value)) return
      const query = { ...route.query }
      delete query.tab
      if (value !== tabs.value[0]?.value) query.tab = value
      void router.push({ path: route.path, query, hash: route.hash })
    }
  })
  return { tabs, active }
}

/**
 * The `UTabs` items of a page's sub-tabs: the table's labels translated, each tab rendered by the
 * slot of its own name, and a count where the page has one — a count lives in the badge so that
 * nothing waits unseen behind a tab the user has not opened.
 */
export function subTabItems(
  section: SettingsSubTabSection,
  t: (key: string) => string,
  badges: Partial<Record<string, number | undefined>> = {}
): TabsItem[] {
  return SETTINGS_SUB_TABS[section].map((tab: SettingsSubTab) => {
    const item: TabsItem = { value: tab.value, slot: tab.value, label: t(tab.labelKey), icon: tab.icon }
    const badge = badges[tab.value]
    if (badge !== undefined) item.badge = badge
    return item
  })
}

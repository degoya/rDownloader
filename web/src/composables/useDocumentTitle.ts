import { watchEffect } from 'vue'

import { MAIN_VIEWS } from '@/composables/searchPalette'
import { i18n } from '@/i18n'
import { router } from '@/router'
import { SETTINGS_SECTIONS } from '@/settingsSections'
import { useSessionStore } from '@/stores/session'
import { useTransfersStore } from '@/stores/transfers'
import { formatRate } from '@/utils/format'
import { titleStatus } from '@/utils/titleStatus'

/** What the tab is called when there is nothing to report; the same name `index.html` ships. */
function appName(): string {
  return i18n.global.t('common.app.name')
}

/**
 * The view an address shows, as the navigation names it, the settings page before "Settings"
 * (WCAG 2.2, 2.4.2); nothing for an address that is no view.
 */
export function viewName(path: string): string[] {
  const [, main = '', section] = path.split('/')
  const view = MAIN_VIEWS.find(entry => entry.path === `/${main}`)
  if (!view) return []
  const page = view.path === '/settings' ? SETTINGS_SECTIONS.find(entry => entry.value === section) : undefined
  return [...(page ? [i18n.global.t(page.labelKey)] : []), i18n.global.t(view.labelKey)]
}

interface TitleState {
  /** What the view is called, from the special to the general; empty before the sign-in. */
  view?: string[]
  /** The setting: `false` leaves the tab at the view's name whatever the queue does. */
  enabled: boolean
  /** Transfers actually moving — resolving, downloading, verifying, repairing, extracting. */
  activeCount: number
  /** Queue-wide throughput in bytes per second, as the service measured it. */
  rate: number | null | undefined
}

/**
 * The tab title for a given state — a pure function, so the cases that matter can be
 * checked without mounting anything.
 *
 * Order is deliberate: the rate comes first because a narrow tab shows only the first few
 * characters, and the number that changes is the one worth seeing there. The same restraint
 * `formatDuration` shows applies to the rate — a figure that is not a usable number is left
 * out entirely rather than printed as a placeholder, so an idle-but-busy queue reads
 * "2 active · Downloads · rDownloader" instead of "— B/s · 2 active · …".
 */
export function transferTitle({ enabled, activeCount, rate, view = [] }: TitleState): string {
  const place = [...view, appName()]
  if (!enabled || activeCount <= 0) return place.join(' · ')
  const measured = typeof rate === 'number' && Number.isFinite(rate) && rate > 0
  const parts = measured ? [formatRate(rate)] : []
  parts.push(i18n.global.t('common.app.title_active', { count: activeCount }))
  return [...parts, ...place].join(' · ')
}

/**
 * Keeps `document.title` in step with the queue.
 *
 * Called once, from `App.vue`, and nowhere else: the third acceptance criterion of RD-106-07
 * asks for exactly one writing place rather than one per view, and a per-view title would
 * fight itself on every route change. The effect re-runs on the rate, the count, the setting,
 * the route and the language, because every one of them is part of the sentence.
 * The route's name joined in RD-1480-17 (WCAG 2.2, 2.4.2).
 */
export function useDocumentTitle(): void {
  const transfers = useTransfersStore()
  const session = useSessionStore()
  watchEffect(() => {
    document.title = transferTitle({
      view: session.ready ? viewName(router.currentRoute.value.path) : [],
      enabled: titleStatus.value,
      activeCount: transfers.activeCount,
      rate: transfers.globalRate
    })
  })
}

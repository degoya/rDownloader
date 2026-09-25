import { watchEffect } from 'vue'

import { i18n } from '@/i18n'
import { useTransfersStore } from '@/stores/transfers'
import { formatRate } from '@/utils/format'
import { titleStatus } from '@/utils/titleStatus'

/** What the tab is called when there is nothing to report; the same name `index.html` ships. */
function appName(): string {
  return i18n.global.t('common.app.name')
}

export interface TitleState {
  /** The setting: `false` leaves the tab at the application name whatever the queue does. */
  enabled: boolean
  /** Transfers actually moving — resolving, downloading, verifying, repairing, extracting. */
  activeCount: number
  /** Queue-wide throughput in bytes per second, as the service measured it. */
  rate: number | null | undefined
}

/**
 * The tab title for a given state — a pure function, so the four cases that matter can be
 * checked without mounting anything.
 *
 * Order is deliberate: the rate comes first because a narrow tab shows only the first few
 * characters, and the number that changes is the one worth seeing there. The same restraint
 * `formatDuration` shows applies to the rate — a figure that is not a usable number is left
 * out entirely rather than printed as a placeholder, so an idle-but-busy queue reads
 * "2 active · rDownloader" instead of "— B/s · 2 active · rDownloader".
 */
export function transferTitle({ enabled, activeCount, rate }: TitleState): string {
  if (!enabled || activeCount <= 0) return appName()
  const measured = typeof rate === 'number' && Number.isFinite(rate) && rate > 0
  const parts = measured ? [formatRate(rate)] : []
  parts.push(i18n.global.t('common.app.title_active', { count: activeCount }))
  parts.push(appName())
  return parts.join(' · ')
}

/**
 * Keeps `document.title` in step with the queue.
 *
 * Called once, from `App.vue`, and nowhere else: the third acceptance criterion of RD-106-07
 * asks for exactly one writing place rather than one per view, and a per-view title would
 * fight itself on every route change. The effect re-runs on the rate, the count, the setting
 * and the language, because every one of them is part of the sentence.
 */
export function useDocumentTitle(): void {
  const transfers = useTransfersStore()
  watchEffect(() => {
    document.title = transferTitle({
      enabled: titleStatus.value,
      activeCount: transfers.active.length,
      rate: transfers.globalRate
    })
  })
}

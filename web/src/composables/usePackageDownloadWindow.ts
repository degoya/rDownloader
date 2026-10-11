import { useOverlay, useToast } from '@nuxt/ui/composables'
import { computed, type ComputedRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import type { Category, DownloadPackage, DownloadWindow } from '@/api/types'
import PackageDownloadWindowModal from '@/components/PackageDownloadWindowModal.vue'
import { useErrorToast } from '@/composables/useErrorToast'
import { useQueuePauseStore } from '@/stores/queuePause'
import { useTransfersStore } from '@/stores/transfers'
import { describeWindow, windowOpen } from '@/utils/downloadWindow'

interface WindowMenuItem {
  label: string
  icon: string
  description?: string
  onSelect: () => void
}

/** What the row's glyph says, or `null` when the package has no window that applies. */
export interface WindowGlyph {
  /** Outside its window right now: its files wait. */
  closed: boolean
  title: string
}

/** The window that applies to a package: its own, otherwise its category's. */
export function effectiveWindow(pkg: DownloadPackage, categories: Category[]): DownloadWindow | null {
  if (pkg.download_window) return pkg.download_window
  const category = categories.find(entry => entry.id === pkg.category_id)
  return category?.download_window ?? null
}

/**
 * A download package's download window (RD-1240-30). Like the "not before" (`usePackageStartAfter`)
 * the row asks this for its glyph and its menu entry — "Download window…", which opens a dialog
 * for the package's own window and says what it follows without one. A finished package offers
 * none. The glyph shows whenever a window applies, warning while it is closed; the service holds
 * the files, the glyph only says so, by the same rule in the schedule's timezone.
 */
export function usePackageDownloadWindow(
  pkg: () => DownloadPackage,
  categories: () => Category[],
  finished: () => boolean
): { glyph: ComputedRef<WindowGlyph | null>, items: ComputedRef<WindowMenuItem[]> } {
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()
  const transfers = useTransfersStore()
  const queuePause = useQueuePauseStore()

  const glyph = computed<WindowGlyph | null>(() => {
    const window = effectiveWindow(pkg(), categories())
    if (!window || finished()) return null
    if (!window.windows?.length) {
      return window.ignore_schedule_pause ? { closed: false, title: t('downloads.window.glyph_bypass') } : null
    }
    const described = describeWindow(window, t)
    const closed = !windowOpen(window, queuePause.scheduleTimezone, new Date(queuePause.now))
    const title = t(closed ? 'downloads.window.glyph_closed' : 'downloads.window.glyph_open', { window: described })
    return { closed, title: window.ignore_schedule_pause ? `${title}\n${t('downloads.window.glyph_bypass')}` : title }
  })

  async function store(window: DownloadWindow | null): Promise<void> {
    const { data, error } = await api.PUT('/api/v1/packages/{id}/download-window', {
      params: { path: { id: pkg().id } },
      body: { download_window: window }
    })
    if (!data) {
      showError(t('downloads.window.failed'), errorMessage(error))
      return
    }
    toast.add({
      title: data.download_window ? t('downloads.window.saved') : t('downloads.window.cleared'),
      color: 'success',
      icon: 'i-lucide-calendar-clock'
    })
    await transfers.refresh()
  }

  async function choose(): Promise<void> {
    const category = categories().find(entry => entry.id === pkg().category_id)
    // Created on demand rather than per row: the list renders a row per package.
    const modal = useOverlay().create(PackageDownloadWindowModal, { destroyOnClose: true })
    const result = await modal.open({
      name: pkg().name,
      current: pkg().download_window ?? null,
      categoryWindow: category?.download_window ?? null,
      timezone: queuePause.scheduleTimezone
    }).result
    if (result && typeof result === 'object' && 'window' in result) await store(result.window as DownloadWindow | null)
  }

  const items = computed<WindowMenuItem[]>(() => finished()
    ? []
    : [{
        label: t('downloads.window.menu'),
        icon: 'i-lucide-calendar-clock',
        description: glyph.value?.title ?? t('downloads.window.menu_hint'),
        onSelect: () => { void choose() }
      }])
  return { glyph, items }
}

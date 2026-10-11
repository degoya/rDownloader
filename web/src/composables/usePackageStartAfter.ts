import { useOverlay, useToast } from '@nuxt/ui/composables'
import { computed, type ComputedRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import type { DownloadPackage } from '@/api/types'
import PackageStartAfterModal from '@/components/PackageStartAfterModal.vue'
import { useErrorToast } from '@/composables/useErrorToast'
import { useTransfersStore } from '@/stores/transfers'
import { formatMoment } from '@/utils/format'

interface StartAfterMenuItem {
  label: string
  icon: string
  description?: string
  onSelect: () => void
}

/** The package's "not before" while it still lies ahead, else `null` (RD-1240-14). */
export function pendingStartAfter(value: string | null | undefined, now = Date.now()): string | null {
  return value && Date.parse(value) > now ? value : null
}

/** The moment a day (`YYYY-MM-DD`) and a clock (`HH:MM`) name in this browser's time zone, or `null`. */
export function startAfterMoment(day: string, clock: string): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day) || !/^\d{2}:\d{2}$/.test(clock)) return null
  const moment = new Date(`${day}T${clock}:00`)
  return Number.isNaN(moment.getTime()) ? null : moment
}

/** The day and clock the dialog opens on: the stored moment, or the next full hour. */
export function startAfterFields(value: string | null, now = new Date()): { day: string, clock: string } {
  const moment = value ? new Date(value) : new Date(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours() + 1)
  const two = (n: number) => String(n).padStart(2, '0')
  return {
    day: `${moment.getFullYear()}-${two(moment.getMonth() + 1)}-${two(moment.getDate())}`,
    clock: `${two(moment.getHours())}:${two(moment.getMinutes())}`
  }
}

/**
 * A download package's "not before" (RD-1240-14): its waiting files start no earlier than a
 * chosen moment. Like the stop mark (`useStopMark`) the row asks this for its glyph and its menu
 * entries — "Start not before…", which opens a dialog for the day and the time, and, while one is
 * set, "Remove the start time". A finished package offers neither. The toast says what was done.
 */
export function usePackageStartAfter(
  pkg: () => DownloadPackage,
  finished: () => boolean
): { pending: ComputedRef<string | null>, items: ComputedRef<StartAfterMenuItem[]> } {
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()
  const transfers = useTransfersStore()
  const pending = computed(() => pendingStartAfter(pkg().start_after))

  async function store(startAfter: string | null): Promise<void> {
    const { data, error } = await api.PUT('/api/v1/packages/{id}/start-after', {
      params: { path: { id: pkg().id } },
      body: { start_after: startAfter }
    })
    if (!data) {
      showError(t('downloads.start_after.failed'), errorMessage(error))
      return
    }
    toast.add({
      title: data.start_after
        ? t('downloads.start_after.set_done', { time: formatMoment(data.start_after) })
        : t('downloads.start_after.cleared'),
      color: 'success',
      icon: 'i-lucide-alarm-clock'
    })
    await transfers.refresh()
  }

  async function choose(): Promise<void> {
    // Created on demand rather than per row: the list renders a row per package.
    const modal = useOverlay().create(PackageStartAfterModal, { destroyOnClose: true })
    const result = await modal.open({ name: pkg().name, current: pending.value }).result
    if (result && typeof result === 'object' && 'at' in result) await store(result.at as string)
  }

  const items = computed<StartAfterMenuItem[]>(() => {
    if (finished()) return []
    return [
      {
        label: t('downloads.start_after.set'),
        icon: 'i-lucide-alarm-clock',
        description: pending.value ? t('downloads.start_after.glyph_title', { time: formatMoment(pending.value) }) : t('downloads.start_after.set_hint'),
        onSelect: () => { void choose() }
      },
      ...(pending.value
        ? [{ label: t('downloads.start_after.clear'), icon: 'i-lucide-alarm-clock-off', onSelect: () => { void store(null) } }]
        : [])
    ]
  })
  return { pending, items }
}

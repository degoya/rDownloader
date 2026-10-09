import { computed, type ComputedRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { useErrorToast } from '@/composables/useErrorToast'
import { useQueuePauseStore } from '@/stores/queuePause'

/**
 * The states in which a file is done as far as a stop mark is concerned — the scheduler's list
 * (`crates/rd-scheduler/src/stop_mark.rs`): a mark there would stop the queue at once, so the
 * menu does not offer one.
 */
export const STOP_MARK_DONE_STATES: readonly string[] = ['completed', 'failed', 'cancelled', 'skipped', 'seeding']

interface StopMarkMenuItem {
  label: string
  icon: string
  description?: string
  onSelect: () => void
}

/**
 * The stop mark on one queue row (RD-1210-02), the same on a file and on a package header:
 * whether the row carries it, and the menu entry that sets it ("set stop mark here") or removes it.
 * A row that is already finished offers no entry; one that is marked always offers the removal.
 */
export function useStopMark(
  kind: 'download' | 'package',
  id: () => string,
  finished: () => boolean
): { marked: ComputedRef<boolean>, items: ComputedRef<StopMarkMenuItem[]> } {
  const { t } = useI18n()
  const queuePause = useQueuePauseStore()
  const showError = useErrorToast()
  const marked = computed(() => queuePause.marks(kind, id()))

  async function run(action: () => Promise<boolean>, failure: string): Promise<void> {
    if (!(await action())) showError(failure, queuePause.error)
  }

  function set(): void {
    const target = kind === 'download' ? { download_id: id() } : { package_id: id() }
    void run(() => queuePause.setStopMark(target), t('downloads.stop_mark.set_failed'))
  }

  function clear(): void {
    void run(() => queuePause.clearStopMark(), t('downloads.stop_mark.clear_failed'))
  }

  const items = computed<StopMarkMenuItem[]>(() => {
    if (marked.value) return [{ label: t('downloads.stop_mark.clear'), icon: 'i-lucide-octagon-x', onSelect: clear }]
    if (finished()) return []
    return [{
      label: t('downloads.stop_mark.set'),
      icon: 'i-lucide-octagon-pause',
      description: t('downloads.stop_mark.set_hint'),
      onSelect: set
    }]
  })
  return { marked, items }
}

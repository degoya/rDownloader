import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { QueueGroup } from '@/composables/useQueueSelection'
import { useTransfersStore } from '@/stores/transfers'

/** The part of the list component this needs: putting focus back on a row that has moved. */
interface RowFocus {
  focusRow: (key: string) => Promise<boolean>
}

/**
 * Arranging packages and the files inside them in the Downloads view, by drag or by keyboard
 * (RD-106-12); split out of `DownloadsView` (WEB-13). A package moves only within its priority
 * tier, and a file only within its package.
 */
export function useQueueReorder(view: {
  /** The packages as displayed, with their files in displayed order. */
  groups: Ref<QueueGroup[]>
  /** True while the state filter or the name search hides part of the list. */
  filterActive: Ref<boolean>
  /** The list, to put focus back on a moved row. */
  list: Ref<RowFocus | null>
  /** True while the view is sorted for the eye: its order is not the queue's (RD-1190-16). */
  sorted?: Ref<boolean>
}) {
  const { t } = useI18n()
  const transfers = useTransfersStore()
  const draggingId = ref<string | null>(null)
  const draggingFileId = ref<string | null>(null)

  /** A sorted view has no queue position to drop on: the move is refused with the reason. */
  function refusedWhileSorted(): boolean {
    if (!view.sorted?.value) return false
    draggingId.value = null
    draggingFileId.value = null
    transfers.notice = t('downloads.view_sort.drag_off')
    return true
  }

  async function onDrop(targetId: string): Promise<void> {
    if (refusedWhileSorted()) return
    // A file dropped on a package header: moving files between packages is not a gesture this
    // list offers, so the drag ends here rather than doing something the user did not ask for.
    if (draggingFileId.value) {
      draggingFileId.value = null
      return
    }
    const sourceId = draggingId.value
    draggingId.value = null
    if (!sourceId || sourceId === targetId) return
    const source = transfers.packages.find(item => item.id === sourceId)
    const target = transfers.packages.find(item => item.id === targetId)
    if (!source || !target) return
    if (source.priority !== target.priority) {
      transfers.notice = t('downloads.notices.reorder_same_priority')
      return
    }
    const order = transfers.packages.map(item => item.id).filter(id => id !== sourceId)
    order.splice(order.indexOf(targetId), 0, sourceId)
    await transfers.reorderPackages(order)
  }

  /**
   * Writes the file order of one package after a drag or a keyboard move.
   *
   * A filtered list shows only part of the package, and the endpoint takes the complete id list,
   * so the filter case says why instead of dropping the gesture silently.
   */
  async function persistFileOrder(packageId: string, order: string[]): Promise<boolean> {
    if (view.filterActive.value) {
      transfers.notice = t('downloads.notices.reorder_filter_active')
      return false
    }
    transfers.notice = null
    await transfers.reorderDownloads(packageId, order)
    return true
  }

  /** File dropped on another file: both have to sit in the same package. */
  async function onFileDrop(targetId: string): Promise<void> {
    if (refusedWhileSorted()) return
    const sourceId = draggingFileId.value
    draggingFileId.value = null
    if (!sourceId || sourceId === targetId) return
    const source = transfers.downloads.find(item => item.id === sourceId)
    const target = transfers.downloads.find(item => item.id === targetId)
    if (!source || !target || !target.package_id || source.package_id !== target.package_id) return
    const group = view.groups.value.find(entry => entry.package.id === target.package_id)
    if (!group) return
    const order = group.downloads.map(item => item.id).filter(id => id !== sourceId)
    order.splice(order.indexOf(targetId), 0, sourceId)
    await persistFileOrder(target.package_id, order)
  }

  /** Keyboard counterpart of the file drag: one step up or down inside the package. */
  async function onFileMove(id: string, delta: -1 | 1): Promise<void> {
    if (refusedWhileSorted()) return
    const download = transfers.downloads.find(item => item.id === id)
    if (!download?.package_id) return
    const group = view.groups.value.find(entry => entry.package.id === download.package_id)
    if (!group) return
    const order = group.downloads.map(item => item.id)
    const from = order.indexOf(id)
    const to = from + delta
    if (from < 0 || to < 0 || to >= order.length) return
    order.splice(to, 0, ...order.splice(from, 1))
    // The row has moved, and with a windowed list its new place may be outside what is rendered.
    // Focus is put back on the same handle so a second press continues the move (RD-106-12).
    if (await persistFileOrder(download.package_id, order)) await view.list.value?.focusRow(`file:${id}`)
  }

  /** Keyboard counterpart of the package drag; the priority tier bounds it just as the drag does. */
  async function onPackageMove(id: string, delta: -1 | 1): Promise<void> {
    if (refusedWhileSorted()) return
    const order = transfers.packages.map(item => item.id)
    const from = order.indexOf(id)
    const to = from + delta
    if (from < 0 || to < 0 || to >= order.length) return
    const source = transfers.packages[from]
    const neighbour = transfers.packages[to]
    if (!source || !neighbour) return
    if (source.priority !== neighbour.priority) {
      transfers.notice = t('downloads.notices.reorder_same_priority')
      return
    }
    transfers.notice = null
    order.splice(to, 0, ...order.splice(from, 1))
    await transfers.reorderPackages(order)
    await view.list.value?.focusRow(`package:${id}`)
  }

  return { draggingId, draggingFileId, onDrop, onFileDrop, onFileMove, onPackageMove }
}

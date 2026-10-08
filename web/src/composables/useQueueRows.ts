import { computed, type Ref } from 'vue'

import type { Download } from '@/api/types'
import type { VirtualRow } from '@/composables/useVirtualRows'
import type { QueueGroup } from '@/composables/useQueueSelection'
import { useTransfersStore } from '@/stores/transfers'

/**
 * The queue as one stream of rows (RD-106-12).
 *
 * A package header, then its files while it is open. Collapsing is a filter on this stream
 * rather than a `v-if` inside a package, because a virtualized list can only window a flat
 * sequence — and the key of a row has to stay the same when the row moves, so that the focus
 * and the selection survive the window sliding past it.
 */
interface QueueRowBase extends VirtualRow { group: QueueGroup }
interface QueuePackageRow extends QueueRowBase { kind: 'package' }
interface QueueFileRow extends QueueRowBase { kind: 'file', download: Download }
type QueueRow = QueuePackageRow | QueueFileRow

/** Starting estimates only; the list measures what the rows really are once they are drawn. */
const PACKAGE_ROW_SIZE = 52
const FILE_ROW_SIZE = 40

/**
 * The Downloads view's packages and rows from the filtered files, and every file of a package
 * whatever the filter hides — split out of `DownloadsView` (WEB-13). `arrange` puts the packages
 * and their files in the order the view shows them: a sort for the eye (RD-1190-16), the queue
 * order without one.
 */
export function useQueueRows(
  visible: Ref<Download[]>,
  isOpen: (id: string) => boolean,
  arrange: (groups: QueueGroup[]) => QueueGroup[] = groups => groups
) {
  const transfers = useTransfersStore()

  // Bucketed in one pass rather than filtering the whole list once per package: that was
  // O(packages x downloads) and re-ran on every store refresh, several times a second under load.
  const groups = computed<QueueGroup[]>(() => {
    const byPackage = new Map<string, Download[]>()
    for (const item of visible.value) {
      if (!item.package_id) continue
      const bucket = byPackage.get(item.package_id)
      if (bucket) bucket.push(item)
      else byPackage.set(item.package_id, [item])
    }
    return arrange(transfers.packages
      .map(pkg => ({ package: pkg, downloads: byPackage.get(pkg.id) ?? [] }))
      .filter(group => group.downloads.length > 0))
  })

  const rows = computed<QueueRow[]>(() => {
    const result: QueueRow[] = []
    for (const group of groups.value) {
      result.push({ key: `package:${group.package.id}`, size: PACKAGE_ROW_SIZE, class: 'pt-2', kind: 'package', group })
      if (!isOpen(group.package.id)) continue
      for (const download of group.downloads) {
        result.push({ key: `file:${download.id}`, size: FILE_ROW_SIZE, kind: 'file', group, download })
      }
    }
    return result
  })

  /**
   * Every download by its package, the filter notwithstanding: a package action acts on all its
   * files. Built once per refresh — each package row asked twice per render, and scanned the whole
   * queue each time (WEB-08).
   */
  const downloadsByPackage = computed(() => {
    const byPackage = new Map<string, typeof transfers.downloads>()
    for (const download of transfers.downloads) {
      if (!download.package_id) continue
      const bucket = byPackage.get(download.package_id)
      if (bucket) bucket.push(download)
      else byPackage.set(download.package_id, [download])
    }
    return byPackage
  })

  function packageDownloads(id: string): typeof transfers.downloads {
    return downloadsByPackage.value.get(id) ?? []
  }

  return { groups, rows, packageDownloads }
}

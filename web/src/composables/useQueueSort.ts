import { computed, ref, watch } from 'vue'

import type { Download, DownloadPackage, DownloadState } from '@/api/types'
import type { QueueGroup } from '@/composables/useQueueSelection'

/** The columns the download list sorts by: the name and the four data columns (RD-1190-16). */
export type QueueSortColumn = 'name' | 'state' | 'progress' | 'size' | 'meta'
export const QUEUE_SORT_COLUMNS: readonly QueueSortColumn[] = ['name', 'state', 'progress', 'size', 'meta']
export type QueueSortDirection = 'asc' | 'desc'
export interface QueueSort { column: QueueSortColumn, direction: QueueSortDirection }

export const QUEUE_SORT_STORAGE_KEY = 'rdownloader-queue-sort-downloads'

/**
 * Ascending by state means "furthest along in the work first": what runs, then what waits, then
 * what stopped, then what is done.
 */
const STATE_ORDER: readonly DownloadState[] = [
  'downloading', 'resolving', 'verifying', 'repairing', 'extracting', 'seeding',
  'retry_wait', 'queued', 'paused', 'blocked', 'failed', 'cancelled', 'skipped', 'completed'
]

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })

function stateRank(state: DownloadState): number {
  const rank = STATE_ORDER.indexOf(state)
  return rank < 0 ? STATE_ORDER.length : rank
}

function bytes(value: string | null | undefined): number {
  return value ? Number(value) : 0
}

function ratio(committed: number, total: number): number {
  return total > 0 ? committed / total : 0
}

type Key = string | number

function compareKeys(left: Key, right: Key): number {
  if (typeof left === 'string' && typeof right === 'string') return collator.compare(left, right)
  return Number(left) - Number(right)
}

function isSort(value: unknown): value is QueueSort {
  if (!value || typeof value !== 'object') return false
  const { column, direction } = value as Record<string, unknown>
  return (QUEUE_SORT_COLUMNS as readonly unknown[]).includes(column) && (direction === 'asc' || direction === 'desc')
}

function read(): QueueSort | null {
  try {
    const raw = localStorage.getItem(QUEUE_SORT_STORAGE_KEY)
    const stored: unknown = raw ? JSON.parse(raw) : null
    return isSort(stored) ? { column: stored.column, direction: stored.direction } : null
  } catch {
    // Unreadable or unavailable storage: the queue order stands.
    return null
  }
}

function write(sort: QueueSort | null): void {
  try {
    if (sort) localStorage.setItem(QUEUE_SORT_STORAGE_KEY, JSON.stringify(sort))
    else localStorage.removeItem(QUEUE_SORT_STORAGE_KEY)
  } catch {
    // Storage unavailable (private mode, quota): the sort still holds while the view lives.
  }
}

/**
 * Sorting the download list for the eye only (RD-1190-16).
 *
 * A click on a column header sorts the packages by it, and the files inside each package too;
 * a second click turns the direction, a third goes back to the queue's own order. Nothing is
 * sent to the server: the queue keeps the order it runs in, and while a sort is in effect the
 * view says so and offers the way back. Equal keys keep their queue order (the sort is stable).
 * Remembered per browser, like the column widths; every access is guarded.
 */
export function useQueueSort(categoryName: (id: string | null | undefined) => string) {
  const sort = ref<QueueSort | null>(read())
  const active = computed(() => sort.value !== null)
  watch(sort, write)

  function toggle(column: QueueSortColumn): void {
    const current = sort.value
    if (current?.column !== column) sort.value = { column, direction: 'asc' }
    else if (current.direction === 'asc') sort.value = { column, direction: 'desc' }
    else sort.value = null
  }

  function reset(): void {
    sort.value = null
  }

  function packageKey(group: QueueGroup, column: QueueSortColumn): Key {
    const pkg: DownloadPackage = group.package
    switch (column) {
      case 'name': return pkg.name
      case 'state': return Math.min(...group.downloads.map(item => stateRank(item.state)))
      case 'size': return group.downloads.reduce((sum, item) => sum + bytes(item.total_bytes), 0)
      case 'progress': return ratio(
        group.downloads.reduce((sum, item) => sum + bytes(item.committed_bytes), 0),
        group.downloads.reduce((sum, item) => sum + bytes(item.total_bytes), 0))
      case 'meta': return categoryName(pkg.category_id)
    }
  }

  function fileKey(download: Download, column: QueueSortColumn): Key {
    switch (column) {
      case 'name': return download.file_name
      case 'state': return stateRank(download.state)
      case 'size': return bytes(download.total_bytes)
      case 'progress': return ratio(bytes(download.committed_bytes), bytes(download.total_bytes))
      // A package's files share its category: their queue order stands.
      case 'meta': return 0
    }
  }

  function sortedBy<T>(items: readonly T[], key: (item: T) => Key, sign: number): T[] {
    return items
      .map(item => ({ item, key: key(item) }))
      .sort((left, right) => sign * compareKeys(left.key, right.key))
      .map(entry => entry.item)
  }

  /** The groups in the order the view shows them; the queue order while no sort is in effect. */
  function arrange(groups: QueueGroup[]): QueueGroup[] {
    const current = sort.value
    if (!current) return groups
    const sign = current.direction === 'asc' ? 1 : -1
    return sortedBy(groups, group => packageKey(group, current.column), sign)
      .map(group => ({ ...group, downloads: sortedBy(group.downloads, item => fileKey(item, current.column), sign) }))
  }

  return { sort, active, toggle, reset, arrange }
}

import { computed, ref, type Ref } from 'vue'

import type { Download, DownloadPackage } from '@/api/types'
import { useRangeSelection } from '@/composables/useRangeSelection'
import { sumSelection } from '@/utils/selectionSize'

export interface QueueGroup { package: DownloadPackage, downloads: Download[] }

/** Prefix of a package row in the range order; the row stands for all of the package's files. */
const PACKAGE_ROW = 'package:'

export function packageRowKey(packageId: string): string {
  return `${PACKAGE_ROW}${packageId}`
}

/**
 * File-level selection with tri-state package checkboxes.
 *
 * `groups` holds what the active filter shows; `allDownloads` is the unfiltered queue. Package
 * checkboxes and the package-level actions (category, priority, delete package) reason over a
 * package's complete file list rather than the visible part of it — a package is the unit those
 * actions apply to, so a filtered view must not be able to report a half-selected package as
 * fully selected.
 *
 * `orderedIds` is the order the rows are actually on screen in, which is what a range selection
 * has to follow: with a virtualized list the visible order is the flattened row stream, not the
 * store's order, and a collapsed package contributes nothing to it (RD-106-12). A package row
 * takes part as `package:<id>` and brings all of its files along (RD-170-13).
 */
export function useQueueSelection(groups: Ref<QueueGroup[]>, allDownloads: Ref<Download[]>, orderedIds?: Ref<string[]>) {
  const selectedFiles = ref<Set<string>>(new Set())

  /** One pass over the queue instead of one filter per package (that was O(packages x files)). */
  const filesByPackage = computed(() => {
    const buckets = new Map<string, Download[]>()
    for (const download of allDownloads.value) {
      if (!download.package_id) continue
      const bucket = buckets.get(download.package_id)
      if (bucket) bucket.push(download)
      else buckets.set(download.package_id, [download])
    }
    return buckets
  })

  function filesOf(packageId: string): Download[] {
    return filesByPackage.value.get(packageId) ?? []
  }

  function fileIdsOf(packageId: string): string[] {
    return filesOf(packageId).map(download => download.id)
  }

  function setFiles(ids: string[], selected: boolean): void {
    const next = new Set(selectedFiles.value)
    for (const id of ids) selected ? next.add(id) : next.delete(id)
    selectedFiles.value = next
  }

  function togglePackage(group: QueueGroup, selected: boolean): void {
    setFiles(filesOf(group.package.id).map(download => download.id), selected)
  }

  const range = useRangeSelection(
    computed(() => orderedIds?.value ?? groups.value.flatMap(group => [packageRowKey(group.package.id), ...fileIdsOf(group.package.id)])),
    setFiles,
    key => key.startsWith(PACKAGE_ROW) ? fileIdsOf(key.slice(PACKAGE_ROW.length)) : undefined
  )
  const anchor = range.anchor

  /**
   * Picks one file, or — holding shift — everything between the last plain pick and this one.
   * `extend` defaults to the modifier the list noted for this click (`noteModifier`).
   */
  function pickFile(id: string, selected: boolean, extend?: boolean): void {
    range.pick(id, selected, extend)
  }

  /** The package checkbox: every file of the package, or a range of rows ending here. */
  function pickPackage(packageId: string, selected: boolean, extend?: boolean): void {
    range.pick(packageRowKey(packageId), selected, extend)
  }

  /** Tri-state per package, computed once for the whole queue rather than per rendered row. */
  const packageStates = computed<Record<string, 'none' | 'some' | 'all'>>(() => {
    const result: Record<string, 'none' | 'some' | 'all'> = {}
    for (const [packageId, files] of filesByPackage.value) {
      const chosen = files.filter(download => selectedFiles.value.has(download.id)).length
      result[packageId] = chosen === 0 ? 'none' : chosen === files.length ? 'all' : 'some'
    }
    return result
  })

  function packageState(group: QueueGroup): 'none' | 'some' | 'all' {
    return packageStates.value[group.package.id] ?? 'none'
  }

  /** Every file of every package the filter currently shows. */
  const selectableIds = computed(() => groups.value.flatMap(group => filesOf(group.package.id).map(download => download.id)))

  function selectAll(): void {
    selectedFiles.value = new Set(selectableIds.value)
  }

  function clear(): void {
    selectedFiles.value = new Set()
    range.reset()
  }

  const selectedDownloads = computed(() => allDownloads.value.filter(download => selectedFiles.value.has(download.id)))
  const selectedIds = computed(() => selectedDownloads.value.map(download => download.id))
  /** Packages whose files are all selected (category/priority apply to whole packages). */
  const fullySelectedPackageIds = computed(() => groups.value.filter(group => packageState(group) === 'all').map(group => group.package.id))
  const count = computed(() => selectedIds.value.length)
  /** The status bar's figure: selected files only, so a ticked package is its files (RD-170-14). */
  const size = computed(() => sumSelection(selectedDownloads.value.map(download => download.total_bytes)))
  const state = computed<'none' | 'some' | 'all'>(() => {
    const total = selectableIds.value.length
    if (!count.value || !total) return 'none'
    return count.value === total ? 'all' : 'some'
  })

  function toggleAll(): void {
    state.value === 'all' ? clear() : selectAll()
  }

  return { selectedFiles, selectedDownloads, selectedIds, fullySelectedPackageIds, count, size, state, anchor, noteModifier: range.noteModifier, setFiles, pickFile, pickPackage, togglePackage, packageState, selectAll, toggleAll, clear }
}

import { computed, ref, type Ref } from 'vue'

import type { Download, DownloadPackage } from '@/api/types'

export interface QueueGroup { package: DownloadPackage, downloads: Download[] }

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
 * store's order, and a collapsed package contributes nothing to it (RD-106-12).
 */
export function useQueueSelection(groups: Ref<QueueGroup[]>, allDownloads: Ref<Download[]>, orderedIds?: Ref<string[]>) {
  const selectedFiles = ref<Set<string>>(new Set())
  /** Where the last plain click landed; a shift-click selects from here to there. */
  const anchor = ref<string | null>(null)

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

  function setFiles(ids: string[], selected: boolean): void {
    const next = new Set(selectedFiles.value)
    for (const id of ids) selected ? next.add(id) : next.delete(id)
    selectedFiles.value = next
  }

  function togglePackage(group: QueueGroup, selected: boolean): void {
    setFiles(filesOf(group.package.id).map(download => download.id), selected)
  }

  /**
   * Picks one file, or — holding shift — everything between the last plain pick and this one.
   *
   * The anchor stays put while the range is being stretched, which is what every file list
   * does and what makes a second shift-click correct the first.
   */
  function pickFile(id: string, selected: boolean, extend = false): void {
    const order = orderedIds?.value ?? selectableIds.value
    const from = anchor.value ? order.indexOf(anchor.value) : -1
    const to = order.indexOf(id)
    if (extend && from >= 0 && to >= 0) {
      const [low, high] = from <= to ? [from, to] : [to, from]
      setFiles(order.slice(low, high + 1), selected)
      return
    }
    anchor.value = id
    setFiles([id], selected)
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
    anchor.value = null
  }

  const selectedDownloads = computed(() => allDownloads.value.filter(download => selectedFiles.value.has(download.id)))
  const selectedIds = computed(() => selectedDownloads.value.map(download => download.id))
  /** Packages whose files are all selected (category/priority apply to whole packages). */
  const fullySelectedPackageIds = computed(() => groups.value.filter(group => packageState(group) === 'all').map(group => group.package.id))
  const count = computed(() => selectedIds.value.length)
  const state = computed<'none' | 'some' | 'all'>(() => {
    const total = selectableIds.value.length
    if (!count.value || !total) return 'none'
    return count.value === total ? 'all' : 'some'
  })

  function toggleAll(): void {
    state.value === 'all' ? clear() : selectAll()
  }

  return { selectedFiles, selectedDownloads, selectedIds, fullySelectedPackageIds, count, state, anchor, setFiles, pickFile, togglePackage, packageState, selectAll, toggleAll, clear }
}

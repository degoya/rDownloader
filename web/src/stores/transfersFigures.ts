import { computed, type Ref } from 'vue'

import type { Download, DownloadPackage } from '@/api/types'

import { ACTIVE_STATES, PAUSABLE_STATES, PENDING_STATES, RESUMABLE_STATES, TRANSFERRING_STATES } from './transfersShared'

/**
 * Everything the transfers store derives from the queue and the measured rates: totals, the
 * global toggle and the per-package figures. Read-only; the store owns the refs it is given.
 */
export function useTransferFigures(
  downloads: Ref<Download[]>,
  packages: Ref<DownloadPackage[]>,
  downloadRates: Ref<Record<string, number>>
) {
  const active = computed(() => downloads.value.filter((item) =>
    ['resolving', 'downloading', 'verifying', 'repairing', 'extracting'].includes(item.state)))
  const queued = computed(() => downloads.value.filter((item) => item.state === 'queued'))
  const totalCommitted = computed(() => downloads.value.reduce(
    (sum, item) => sum + BigInt(item.committed_bytes), 0n))
  /** Bytes still to fetch for queued/running/paused files whose size is known. */
  const totalRemaining = computed(() => downloads.value.reduce((sum, item) => {
    if (!item.total_bytes || !PENDING_STATES.includes(item.state)) return sum
    const remaining = BigInt(item.total_bytes) - BigInt(item.committed_bytes)
    return remaining > 0n ? sum + remaining : sum
  }, 0n))
  /** One toggle instead of two buttons: pause wins while anything can still be paused. */
  const globalControl = computed<'pause' | 'resume' | null>(() => {
    if (downloads.value.some(download => PAUSABLE_STATES.includes(download.state))) return 'pause'
    if (downloads.value.some(download => RESUMABLE_STATES.includes(download.state))) return 'resume'
    return null
  })

  const filesByPackage = computed(() => {
    const map = new Map<string, Download[]>()
    for (const download of downloads.value) {
      const files = map.get(download.package_id)
      if (files) files.push(download)
      else map.set(download.package_id, [download])
    }
    return map
  })

  /** Packages holding at least one active file; the nav badge counts packages, not files. */
  const activePackages = computed(() => packages.value.filter(pkg =>
    (filesByPackage.value.get(pkg.id) ?? []).some(file =>
      ACTIVE_STATES.includes(file.state as typeof ACTIVE_STATES[number]))).length)

  /**
   * Packages whose files are all finished: every one completed or stood down as a mirror, at
   * least one completed.
   *
   * The files decide, never the package state alone (RD-1190-13): a package that still holds a
   * waiting, failed or blocked file is not finished, whatever post-processing made of the rest
   * — "5/14 · 9 errors" with the finished tick was the owner's report. The state counts only for
   * a package whose files are no longer listed.
   */
  const packageComplete = computed<Record<string, boolean>>(() => {
    const result: Record<string, boolean> = {}
    for (const pkg of packages.value) {
      const files = filesByPackage.value.get(pkg.id) ?? []
      result[pkg.id] = files.length === 0
        ? pkg.state === 'completed'
        : files.some(file => file.state === 'completed')
          && files.every(file => file.state === 'completed' || file.state === 'skipped')
    }
    return result
  })

  /** Combined live rate of every file in a package, in bytes per second. */
  const packageRates = computed<Record<string, number>>(() => {
    const result: Record<string, number> = {}
    for (const pkg of packages.value) {
      result[pkg.id] = (filesByPackage.value.get(pkg.id) ?? [])
        .reduce((sum, file) => sum + (downloadRates.value[file.id] ?? 0), 0)
    }
    return result
  })

  /**
   * Seconds left per package at its own current rate.
   *
   * `null` wherever the estimate would be invented: the package is not moving, or one of the
   * files still to be fetched has no known size, which would turn the sum into a lower bound.
   */
  const packageEtas = computed<Record<string, number | null>>(() => {
    const result: Record<string, number | null> = {}
    for (const pkg of packages.value) {
      const rate = packageRates.value[pkg.id] ?? 0
      const files = (filesByPackage.value.get(pkg.id) ?? [])
        .filter(file => TRANSFERRING_STATES.includes(file.state))
      const remaining = files.reduce<bigint | null>((sum, file) => {
        if (sum === null || !file.total_bytes) return null
        const left = BigInt(file.total_bytes) - BigInt(file.committed_bytes)
        return sum + (left > 0n ? left : 0n)
      }, 0n)
      result[pkg.id] = rate > 0 && remaining !== null ? Math.ceil(Number(remaining) / rate) : null
    }
    return result
  })

  return { active, queued, totalCommitted, totalRemaining, globalControl, activePackages, packageComplete, packageRates, packageEtas }
}

import { computed } from 'vue'

import type { Download } from '@/api/types'
import { isRecoveryVolume } from '@/utils/format'

/** A queue package's counts and its download progress, as its header row shows them. */
export function usePackageCounts(downloads: () => Download[], usenet: () => boolean) {
  /** Every file that is not repair data has arrived, so no repair block can still be wanted. */
  const payloadComplete = computed(() => downloads().every(
    item => isRecoveryVolume(item) || item.state === 'completed' || item.state === 'skipped'))
  /**
   * Recovery volumes that failed while the payload is complete — nobody ever asked for them.
   *
   * An expired `vol…par2` is routine on Usenet. Counting it made a package that verified and
   * unpacked cleanly report "47/48 · 1 error" at 99% (RD-107-10). While the payload is still
   * short this set is empty, so a package that genuinely needs its repair blocks keeps every
   * failure it has.
   */
  const dismissed = computed(() => payloadComplete.value
    ? downloads().filter(item => isRecoveryVolume(item) && ['failed', 'blocked'].includes(item.state))
    : [])
  const dismissedIds = computed(() => new Set(dismissed.value.map(item => item.id)))
  const finished = computed(() =>
    downloads().filter(item => item.state === 'completed').length + dismissed.value.length)
  const activeCount = computed(() => downloads().filter(item => ['resolving', 'downloading', 'verifying', 'repairing', 'extracting'].includes(item.state)).length)
  const errorCount = computed(() => downloads().filter(
    item => ['failed', 'blocked'].includes(item.state) && !dismissedIds.value.has(item.id)).length)
  /** What "reset failed and blocked" would take, recovery volumes nobody needs included (RD-1190-15). */
  const stuckCount = computed(() => downloads().filter(item => ['failed', 'blocked'].includes(item.state)).length)
  /**
   * A mirror standing down is not work left to do: its bytes arrive through the link that is
   * running, so counting them would keep the package short of 100% for good. A recovery volume
   * nobody needed is the same case — its bytes were never owed, so they must not weigh on the
   * total either.
   */
  const counted = computed(() => downloads().filter(
    item => item.state !== 'skipped' && !dismissedIds.value.has(item.id)))
  const committed = computed(() => counted.value.reduce((sum, item) => sum + BigInt(item.committed_bytes), 0n))
  const total = computed(() => counted.value.reduce((sum, item) => sum + BigInt(item.total_bytes ?? '0'), 0n))
  const downloadProgress = computed(() => total.value > 0n ? Number(committed.value * 100n / total.value) : 0)
  /**
   * Some files arrived, the rest wait, failed or are blocked, and nothing runs: post-processing
   * holds back until every file is there (RD-1190-13), and says so instead of looking stuck.
   * Usenet starts over a failed file, which PAR2 may rebuild, so the hint is not for it.
   */
  const waitingForParts = computed(() => {
    const missing = downloads().filter(item => !['completed', 'skipped', 'cancelled', 'seeding'].includes(item.state))
    return !usenet() && finished.value > 0 && missing.length > 0
      && missing.every(item => ['retry_wait', 'failed', 'blocked', 'paused'].includes(item.state))
  })

  return { finished, activeCount, errorCount, stuckCount, counted, committed, total, downloadProgress, waitingForParts }
}

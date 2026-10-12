import type { Ref } from 'vue'

import type { Download, DownloadPackage, PostprocessStage, TorrentAggregateStats } from '@/api/types'
import { useNotifications } from '@/composables/useNotifications'
import { usePostprocessStore } from '@/stores/postprocess'
import { useTorrentsStore } from '@/stores/torrents'
import { isRecoveryVolume } from '@/utils/format'

import { FAILED_STATES, PAUSABLE_STATES, t } from './transfersShared'

interface PostprocessProgressPayload {
  owner_id?: string
  stage?: PostprocessStage | null
  percent?: number | null
  current?: string | null
}

/**
 * Server events arrive as the whole envelope, with the event's own data nested under
 * `payload`. Reading the fields from the top level found nothing, so live post-processing
 * progress never reached the UI — it only appeared after the next full refresh.
 */
interface EventEnvelope<T> {
  payload?: T
}

/** At most one "download list finished" per this span (RD-1260-04). */
export const QUEUE_DONE_SPACING_MS = 10 * 60_000

/** States a file of a package can still fail from: it waits or moves. */
const MOVING_STATES: readonly string[] = PAUSABLE_STATES
/** States in which the queue is not empty: moving, or paused and waiting to go on. */
const WAITING_STATES: readonly string[] = [...PAUSABLE_STATES, 'paused']
/** After these, a file does nothing more by itself; the service's reading of a failed package. */
const SETTLED_FAILED_STATES: readonly string[] = ['failed', 'blocked', 'cancelled', 'skipped']

/**
 * Turns state transitions into desktop notifications (RD-1260-04).
 *
 * Failures are reported per package, once none of its files can fail any more, with how many
 * failed — fifty failed files are one notification, not fifty. A package all of whose files
 * failed is what push reports as `package_failed`, so it is left to push where push reaches this
 * browser. The "queue finished" message fires when nothing waits any more — a pause is no end —
 * at least one file completed since work started, and the last one is ten minutes ago.
 * Called inside the store's setup, since it takes the notifications composable.
 */
export function createQueueAnnouncer(now: () => number = () => Date.now()): (
  next: readonly Download[],
  packages?: readonly DownloadPackage[]
) => void {
  const notifications = useNotifications()
  /** Last seen state per file; `null` until the first snapshot, so a reload announces nothing. */
  let lastStates: Map<string, string> | null = null
  /** Files that failed since their package last reported, by package. */
  const failedSince = new Map<string, Set<string>>()
  let queueArmed = false
  let completionsSinceArm = 0
  let queueDoneAt: number | null = null

  function reportPackages(next: readonly Download[], packages: readonly DownloadPackage[]): void {
    for (const [packageId, failed] of failedSince) {
      const files = next.filter(download => download.package_id === packageId)
      if (files.some(download => MOVING_STATES.includes(download.state))) continue
      failedSince.delete(packageId)
      const names = files.filter(download => failed.has(download.id) && FAILED_STATES.includes(download.state))
      const [first] = names
      if (!first) continue
      const allFailed = files.every(download => SETTLED_FAILED_STATES.includes(download.state))
      const options = { tag: `package-failed:${packageId}`, ...(allFailed ? { pushEvent: 'package_failed' as const } : {}) }
      if (names.length === 1) {
        notifications.notify(
          t('downloads.notifications.failed_title'),
          t('downloads.notifications.failed_body', { file: first.file_name }),
          options
        )
        continue
      }
      const name = packages.find(entry => entry.id === packageId)?.name ?? first.file_name
      notifications.notify(
        t('downloads.notifications.package_failed_title', { package: name }),
        t('downloads.notifications.package_failed_body', { count: names.length, total: files.length }),
        options
      )
    }
  }

  return function announce(next: readonly Download[], packages: readonly DownloadPackage[] = []): void {
    const previous = lastStates
    if (previous) {
      for (const download of next) {
        if (previous.get(download.id) === download.state) continue
        if (FAILED_STATES.includes(download.state)) {
          // A PAR2 volume is repair data, and whether it was needed is not known when it
          // fails — only the verification decides that. Announcing it turned the routine
          // loss of an expired volume into "download failed" for a package that went on to
          // unpack cleanly (RD-107-10). A package that really cannot be repaired says so
          // itself, through its own state and notification.
          if (isRecoveryVolume(download)) continue
          const failed = failedSince.get(download.package_id) ?? new Set<string>()
          failed.add(download.id)
          failedSince.set(download.package_id, failed)
        } else if (download.state === 'completed') {
          completionsSinceArm += 1
        }
      }
    }
    lastStates = new Map(next.map(download => [download.id, download.state]))
    reportPackages(next, packages)

    if (next.some(download => WAITING_STATES.includes(download.state))) {
      queueArmed = true
      return
    }
    const spaced = queueDoneAt === null || now() - queueDoneAt >= QUEUE_DONE_SPACING_MS
    if (queueArmed && completionsSinceArm > 0 && spaced) {
      queueDoneAt = now()
      notifications.notify(
        t('downloads.notifications.queue_done_title'),
        t('downloads.notifications.queue_done_body', { count: completionsSinceArm }, completionsSinceArm),
        { tag: 'queue-done' }
      )
    }
    queueArmed = false
    completionsSinceArm = 0
  }
}

/** Patches the live stage/percent of one package without a full refresh. */
export function applyPostprocessProgress(packages: Ref<DownloadPackage[]>, event: MessageEvent<string>): void {
  let payload: PostprocessProgressPayload
  try {
    payload = (JSON.parse(event.data) as EventEnvelope<PostprocessProgressPayload>).payload ?? {}
  } catch {
    return
  }
  if (!payload.owner_id) return
  const pkg = packages.value.find(item => item.id === payload.owner_id)
  if (pkg) {
    pkg.postprocess = {
      stage: payload.stage ?? pkg.postprocess?.stage ?? null,
      percent: payload.percent ?? null,
      current: payload.current ?? null
    }
  }
  usePostprocessStore().applyProgress(payload.owner_id, payload.stage ?? null, payload.percent ?? null, payload.current ?? null)
}

/** Applies one aggregate torrent sample from the event stream. */
export function applyTorrentStats(event: MessageEvent<string>): void {
  let payload: { download_id?: string, stats?: TorrentAggregateStats }
  try {
    payload = (JSON.parse(event.data) as EventEnvelope<{ download_id?: string, stats?: TorrentAggregateStats }>).payload ?? {}
  } catch {
    return
  }
  if (!payload.download_id || !payload.stats) return
  useTorrentsStore().applyStats(payload.download_id, payload.stats)
}

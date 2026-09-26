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

/**
 * Turns state transitions into desktop notifications.
 *
 * Failures are reported per file; the "queue finished" message fires once when the last
 * running or waiting transfer is gone and at least one file completed since work started.
 * Called inside the store's setup, since it takes the notifications composable.
 */
export function createQueueAnnouncer(): (next: readonly Download[]) => void {
  const notifications = useNotifications()
  /** Last seen state per file; `null` until the first snapshot, so a reload announces nothing. */
  let lastStates: Map<string, string> | null = null
  let queueArmed = false
  let completionsSinceArm = 0

  return function announce(next: readonly Download[]): void {
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
          notifications.notify(
            t('downloads.notifications.failed_title'),
            t('downloads.notifications.failed_body', { file: download.file_name })
          )
        } else if (download.state === 'completed') {
          completionsSinceArm += 1
        }
      }
    }
    lastStates = new Map(next.map(download => [download.id, download.state]))

    if (next.some(download => PAUSABLE_STATES.includes(download.state))) {
      queueArmed = true
      return
    }
    if (queueArmed && completionsSinceArm > 0) {
      notifications.notify(
        t('downloads.notifications.queue_done_title'),
        t('downloads.notifications.queue_done_body', { count: completionsSinceArm }, completionsSinceArm)
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

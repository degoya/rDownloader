import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type { Download, DownloadBulkAction, DownloadPackage, DownloadPriority, DownloadRates, PackageUpdateRequest, PostprocessLevel, PostprocessStage, PostprocessStep, TorrentAggregateStats } from '@/api/types'
import { subscribeEvents } from '@/composables/useEventStream'
import { useNotifications } from '@/composables/useNotifications'
import { i18n } from '@/i18n'
import { translateServerMessage } from '@/i18n/server'
import { usePostprocessStore } from '@/stores/postprocess'
import { useTorrentsStore } from '@/stores/torrents'
import { MIB, isRecoveryVolume } from '@/utils/format'
import {
  appendTransferRateHistory,
  type TransferRateHistoryPoint
} from '@/utils/transferRates'
import { withBase } from '@/basePath'

interface DownloadSelection {
  categoryId?: string | undefined
  accountId?: string | undefined
  proxyProfileId?: string | undefined
  priority?: DownloadPriority | undefined
}

export type ClearScope = 'completed' | 'failed' | 'all'

/** A package the server refused to clear, with the stable code saying why. */
interface ClearSkip {
  package_id: string
  name: string
  code: string
}

interface ClearResult {
  removed: number
  skipped: ClearSkip[]
}

export interface PackageChange {
  categoryId?: string | null
  priority?: DownloadPriority
  name?: string
  /** null clears the stored password */
  password?: string | null
  /** null clears the package level (inherit category/global default) */
  postprocessLevel?: PostprocessLevel | null
  /** null clears the package script (inherit category/global default) */
  script?: string | null
}

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

const ACTIVE_STATES = ['resolving', 'downloading', 'verifying', 'repairing', 'extracting'] as const

/** States a pause acts on. Overlaps `RESUMABLE_STATES`, so pause always wins in the UI toggle. */
export const PAUSABLE_STATES: readonly string[] = ['queued', 'retry_wait', ...ACTIVE_STATES]
/** States a resume acts on. `queued` is left out – a queued transfer is already on its way. */
// `skipped` belongs here: a waiting mirror is started by resuming it, which stands its
// siblings down. Without it there is no way to choose a different link by hand.
export const RESUMABLE_STATES: readonly string[] = ['retry_wait', 'paused', 'failed', 'blocked', 'cancelled', 'skipped']
/**
 * States a reset acts on: everything that is not moving right now. A finished or seeding job is
 * included on purpose — starting over is exactly what a reset is for.
 */
export const RESETTABLE_STATES: readonly string[] = ['queued', 'retry_wait', 'paused', 'failed', 'blocked', 'cancelled', 'completed', 'seeding']
/** A transfer that stopped and needs attention. */
const FAILED_STATES: readonly string[] = ['failed', 'blocked']
/** States whose files still count towards the outstanding queue volume. */
const PENDING_STATES: readonly string[] = [...PAUSABLE_STATES, 'paused']
/**
 * States whose bytes are still going to come down the wire — the same line the server draws
 * for its estimate (`is_transferring` in `download_handlers.rs`). Paused, blocked, verifying,
 * repairing, extracting and seeding are all outstanding in some sense, but none of them is
 * being fetched, so none of them belongs in "how long at the current speed".
 */
const TRANSFERRING_STATES: readonly string[] = ['queued', 'retry_wait', 'resolving', 'downloading']

const t =(key: string, named: Record<string, unknown> = {}, plural?: number): string =>
  plural === undefined ? i18n.global.t(key, named) : i18n.global.t(key, named, plural)

export const useTransfersStore = defineStore('transfers', () => {
  const downloads = ref<Download[]>([])
  const packages = ref<DownloadPackage[]>([])
  const pending = ref(false)
  /**
   * True once the first refresh has settled. `pending` alone cannot answer "is the queue
   * really empty?": before `refresh()` is even called it is `false`, so a view reading it
   * would print the empty state for the first frames of its own mount (RD-104-07).
   */
  const settled = ref(false)
  const clearing = ref(false)
  const controlsBusy = ref(false)
  const error = ref<string | null>(null)
  const notice = ref<string | null>(null)
  const downloadRates = ref<Record<string, number>>({})
  /** Seconds left per file, as the server measured them. Absent means "nothing to say". */
  const downloadEtas = ref<Record<string, number>>({})
  const globalRate = ref(0)
  /** Seconds until the queue is through at the current rate; `null` when no honest figure exists. */
  const queueEta = ref<number | null>(null)
  const speedHistory = ref<TransferRateHistoryPoint[]>([])
  let releaseEvents: (() => void) | null = null
  /** Monotonic ticket so only the newest `refresh()` may apply its result (see `refresh`). */
  let refreshTicket = 0
  /** True while a refresh is awaiting the network; event bursts wait rather than pile up. */
  let refreshing = false

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
   * Packages whose files are all finished.
   *
   * `refresh_package_state` never sets `Completed` server-side (only post-processing does),
   * so the all-files check is the reliable predicate; the package state is honoured as well.
   */
  const packageComplete = computed<Record<string, boolean>>(() => {
    const result: Record<string, boolean> = {}
    for (const pkg of packages.value) {
      const files = filesByPackage.value.get(pkg.id) ?? []
      result[pkg.id] = pkg.state === 'completed'
        || (files.length > 0 && files.every(file => file.state === 'completed'))
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

  /**
   * Takes the rates the server measured. A refusal or a shape we do not recognise leaves the
   * last known figures standing rather than blanking the display for one bad read.
   */
  function applyRates(payload: DownloadRates | undefined): void {
    if (!payload || typeof payload !== 'object' || !Array.isArray(payload.downloads)) return
    const rates: Record<string, number> = {}
    const etas: Record<string, number> = {}
    for (const entry of payload.downloads) {
      rates[entry.id] = entry.bytes_per_second
      if (entry.eta_seconds !== null && entry.eta_seconds !== undefined) etas[entry.id] = entry.eta_seconds
    }
    downloadRates.value = rates
    downloadEtas.value = etas
    globalRate.value = payload.bytes_per_second ?? 0
    queueEta.value = payload.eta_seconds ?? null
  }

  async function refresh(): Promise<void> {
    // Refreshes are triggered from several places at once (events, user actions, the history
    // sampler). Without a sequence guard an older response could land last and rewind
    // `lastStates`, making `announce()` report the same transitions a second time.
    const ticket = ++refreshTicket
    refreshing = true
    pending.value = true
    const [downloadResponse, packageResponse, rateResponse] = await Promise.all([
      api.GET('/api/v1/downloads'),
      api.GET('/api/v1/packages'),
      api.GET('/api/v1/downloads/rates')
    ])
    if (ticket !== refreshTicket) return
    refreshing = false
    pending.value = false
    settled.value = true
    applyRates(rateResponse.data)
    if (downloadResponse.data && packageResponse.data) {
      announce(downloadResponse.data)
      downloads.value = downloadResponse.data
      packages.value = packageResponse.data
      error.value = null
    } else {
      error.value = responseError(downloadResponse.data ? packageResponse : downloadResponse)
    }
  }

  async function add(
    url: string,
    packageName?: string,
    fileName?: string,
    selection: DownloadSelection = {}
  ): Promise<boolean> {
    const response = await api.POST('/api/v1/downloads', {
      body: {
        url,
        ...(packageName ? { package_name: packageName } : {}),
        ...(fileName ? { file_name: fileName } : {}),
        ...(selection.categoryId ? { category_id: selection.categoryId } : {}),
        ...(selection.accountId ? { account_id: selection.accountId } : {}),
        ...(selection.proxyProfileId ? { proxy_profile_id: selection.proxyProfileId } : {}),
        ...(selection.priority ? { priority: selection.priority } : {})
      }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    downloads.value.push(response.data)
    error.value = null
    return true
  }

  async function control(id: string, action: 'pause' | 'resume' | 'cancel' | 'stop_seeding'): Promise<void> {
    const response = action === 'pause'
      ? await api.POST('/api/v1/downloads/{id}/pause', { params: { path: { id } } })
      : action === 'resume'
        ? await api.POST('/api/v1/downloads/{id}/resume', { params: { path: { id } } })
        : action === 'stop_seeding'
          ? await api.POST('/api/v1/downloads/{id}/seeding/stop', { params: { path: { id } } })
          : await api.POST('/api/v1/downloads/{id}/cancel', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    await refresh()
  }

  async function controlAll(action: 'pause' | 'resume'): Promise<number> {
    if (controlsBusy.value) return 0
    const states = action === 'pause' ? PAUSABLE_STATES : RESUMABLE_STATES
    const targets = downloads.value.filter(download => states.includes(download.state))
    if (!targets.length) {
      notice.value = t(action === 'pause' ? 'downloads.notices.nothing_to_pause' : 'downloads.notices.nothing_to_resume')
      return 0
    }
    controlsBusy.value = true
    notice.value = null
    error.value = null
    let changed = 0
    for (const download of targets) {
      const endpoint = action === 'pause'
        ? '/api/v1/downloads/{id}/pause' as const
        : '/api/v1/downloads/{id}/resume' as const
      const response = await api.POST(endpoint, { params: { path: { id: download.id } } })
      if (response.data) changed += 1
      else error.value = responseError(response)
    }
    controlsBusy.value = false
    await refresh()
    notice.value = t(action === 'pause' ? 'downloads.notices.paused_count' : 'downloads.notices.resumed_count', { count: changed }, changed)
    return changed
  }

  async function remove(id: string): Promise<boolean> {
    const response = await fetch(withBase(`/api/v1/downloads/${encodeURIComponent(id)}`), {
      method: 'DELETE',
      credentials: 'include'
    })
    const payload: unknown = await response.json().catch(() => null)
    if (!response.ok) {
      error.value = payloadError(payload)
      return false
    }
    downloads.value = downloads.value.filter(download => download.id !== id)
    error.value = null
    return true
  }

  /**
   * Clears the list on the server, in one request, over whole packages.
   *
   * It used to pick single rows here and delete them one at a time, in up to fifteen rounds.
   * Nothing in that chain asked what else was in the package, so "remove completed" tore the
   * finished rows out of a package that was still downloading and left the files behind with
   * nothing that knew they belonged together (RD-107-07). The rule is now one server-side
   * decision, and what it refused to touch comes back with a reason.
   *
   * Addressed by hand rather than through the generated client so the endpoint works before
   * the API contract is regenerated; `remove()` above does the same.
   */
  async function clear(scope: ClearScope): Promise<void> {
    if (clearing.value) return
    clearing.value = true
    notice.value = null
    error.value = null
    const response = await fetch(withBase('/api/v1/packages/clear'), {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ scope })
    })
    const payload: unknown = await response.json().catch(() => null)
    clearing.value = false
    if (!response.ok) {
      // The refresh has to come first: on success it clears `error`, so a message set before
      // it would be wiped and the refusal would read as a completed clear.
      await refresh()
      error.value = payloadError(payload)
      return
    }
    const result = payload as ClearResult | null
    const removed = result?.removed ?? 0
    const skipped = result?.skipped ?? []
    await refresh()
    if (!removed && !skipped.length) {
      notice.value = t('downloads.notices.nothing_to_clear')
      return
    }
    notice.value = [
      t('downloads.notices.cleared_packages', { count: removed }, removed),
      ...skipReasons(skipped)
    ].join(' ')
  }

  /**
   * One sentence per reason, not one per package: a list of thirty names is unreadable, and the
   * reason is what tells somebody whether to wait or to act.
   */
  function skipReasons(skipped: ClearSkip[]): string[] {
    const byCode = new Map<string, string[]>()
    for (const entry of skipped) {
      const names = byCode.get(entry.code)
      if (names) names.push(entry.name)
      else byCode.set(entry.code, [entry.name])
    }
    return [...byCode].map(([code, names]) => t('downloads.notices.clear_skipped', {
      count: names.length,
      reason: translateServerMessage({ code, message: code }),
      names: names.slice(0, 3).join(', ')
    }, names.length))
  }

  function changeBody(change: PackageChange): PackageUpdateRequest {
    return {
      ...(change.categoryId ? { category_id: change.categoryId } : {}),
      ...(change.categoryId === null ? { clear_category: true } : {}),
      ...(change.priority ? { priority: change.priority } : {}),
      ...(change.name ? { name: change.name } : {}),
      ...(change.password ? { password: change.password } : {}),
      ...(change.password === null ? { clear_password: true } : {}),
      ...(change.postprocessLevel ? { postprocess_level: change.postprocessLevel } : {}),
      ...(change.postprocessLevel === null ? { clear_postprocess_level: true } : {}),
      ...(change.script ? { script: change.script } : {}),
      ...(change.script === null ? { clear_script: true } : {})
    }
  }

  /** Applies one action to many files server-side; returns the number of affected files. */
  async function bulk(ids: string[], action: DownloadBulkAction): Promise<number> {
    if (!ids.length) return 0
    const response = await api.POST('/api/v1/downloads/bulk', { body: { ids, action } })
    if (!response.data) {
      error.value = responseError(response)
      await refresh()
      return 0
    }
    if (response.data.errors.length) error.value = response.data.errors.slice(0, 3).join(' · ')
    else error.value = null
    await refresh()
    return response.data.affected
  }

  /**
   * Discards what the given files produced and queues them again from the start.
   *
   * One file takes the dedicated endpoint and many take the bulk one, the way `updatePackages`
   * already splits, so a single reset reports its own error rather than a batch summary.
   */
  async function reset(ids: string[], deleteCompletedFiles: boolean): Promise<number> {
    if (!ids.length) return 0
    if (ids.length > 1) {
      return bulk(ids, deleteCompletedFiles ? 'reset_delete_files' : 'reset')
    }
    const response = await api.POST('/api/v1/downloads/{id}/reset', {
      params: { path: { id: ids[0]! } },
      body: { delete_completed_files: deleteCompletedFiles }
    })
    if (!response.data) {
      error.value = responseError(response)
      await refresh()
      return 0
    }
    notice.value = resultMessage(response.data)
    error.value = null
    await refresh()
    return 1
  }

  async function extractDownloads(ids: string[]): Promise<boolean> {
    if (!ids.length) return false
    const response = await api.POST('/api/v1/downloads/extract', { body: { ids } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    notice.value = resultMessage(response.data)
    error.value = null
    return true
  }

  async function extractPackages(ids: string[]): Promise<boolean> {
    if (!ids.length) return false
    const response = await api.POST('/api/v1/packages/extract', { body: { ids } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    notice.value = resultMessage(response.data)
    error.value = null
    return true
  }

  /**
   * Post-processes one package although its verification failed (RD-104-04).
   *
   * The one-off counterpart to the `safe_postproc` setting: a broken recovery set beside
   * intact archives is a real case, and the answer to it should not be a global switch
   * somebody then has to remember to put back.
   */
  async function forceExtractPackage(id: string): Promise<boolean> {
    const response = await api.POST('/api/v1/packages/{id}/extract/force', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    notice.value = resultMessage(response.data)
    error.value = null
    return true
  }

  async function loadPostprocess(id: string): Promise<PostprocessStep[]> {
    const response = await api.GET('/api/v1/packages/{id}/postprocess', { params: { path: { id } } })
    return response.data ?? []
  }

  async function renameDownload(id: string, fileName: string): Promise<boolean> {
    const response = await api.PATCH('/api/v1/downloads/{id}', { params: { path: { id } }, body: { file_name: fileName } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    downloads.value = downloads.value.map(download => download.id === id ? response.data! : download)
    error.value = null
    return true
  }

  async function updatePackages(ids: string[], change: PackageChange): Promise<boolean> {
    if (!ids.length) return false
    const response = ids.length === 1 && ids[0]
      ? await api.PATCH('/api/v1/packages/{id}', { params: { path: { id: ids[0] } }, body: changeBody(change) })
      : await api.POST('/api/v1/packages/bulk', { body: { ids, ...changeBody(change) } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  /**
   * Renames a package **and the folder its files live in** (RD-106-13).
   *
   * Separate from `updatePackages`, which changes the label alone: this one moves data, and a
   * name that is already taken comes back as an error instead of being worked around.
   */
  async function renamePackageFolder(id: string, name: string): Promise<boolean> {
    const response = await api.POST('/api/v1/packages/{id}/folder', { params: { path: { id } }, body: { name } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  /**
   * Removes whole packages.
   *
   * `force` is what turns this into a destructive action: without it the server refuses a
   * package that is still running, waiting, seeding or being post-processed instead of
   * cancelling its files and dropping what they had already written. Only pass it when the
   * person was told that is what happens.
   */
  async function deletePackages(ids: string[], force = false): Promise<boolean> {
    if (!ids.length) return false
    const response = await api.POST('/api/v1/packages/delete', { body: { ids, force } })
    if (!response.data) {
      error.value = responseError(response)
      await refresh()
      return false
    }
    notice.value = resultMessage(response.data)
    error.value = null
    await refresh()
    return true
  }

  /** Global speed limit in MiB/s (null = unlimited), mirrored from the settings. */
  const speedLimitMiB = ref<number | null>(null)
  const speedLimitBusy = ref(false)

  async function loadSpeedLimit(): Promise<void> {
    const response = await api.GET('/api/v1/settings')
    if (!response.data) return
    speedLimitMiB.value = response.data.speed_limit_bytes_per_second
      ? Number(response.data.speed_limit_bytes_per_second) / MIB
      : null
  }

  async function setSpeedLimit(mib: number | null): Promise<boolean> {
    speedLimitBusy.value = true
    const current = await api.GET('/api/v1/settings')
    if (!current.data) {
      speedLimitBusy.value = false
      error.value = responseError(current)
      return false
    }
    const response = await api.PUT('/api/v1/settings', {
      body: {
        ...current.data,
        speed_limit_bytes_per_second: mib && mib > 0 ? String(Math.round(mib * MIB)) : null
      }
    })
    speedLimitBusy.value = false
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    speedLimitMiB.value = response.data.speed_limit_bytes_per_second
      ? Number(response.data.speed_limit_bytes_per_second) / MIB
      : null
    notice.value = speedLimitMiB.value
      ? t('downloads.notices.speed_limit_set', { value: speedLimitMiB.value })
      : t('downloads.notices.speed_limit_cleared')
    error.value = null
    return true
  }

  async function reorderPackages(ids: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/packages/reorder', { body: { ids } })
    if (!response.data) {
      // The refresh has to come first: on success it clears `error`, so a message set before it
      // would be wiped and the refusal would read as a saved order.
      await refresh()
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  /**
   * Writes the manual file order inside one package.
   *
   * `ids` has to be exactly that package's files, each of them once — the server refuses
   * anything else, because it hands out the positions 1..n from this list.
   */
  async function reorderDownloads(packageId: string, ids: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/downloads/reorder', { body: { package_id: packageId, ids } })
    if (!response.data) {
      await refresh()
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  const notifications = useNotifications()
  /** Last seen state per file; `null` until the first snapshot, so a reload announces nothing. */
  let lastStates: Map<string, string> | null = null
  let queueArmed = false
  let completionsSinceArm = 0

  /**
   * Turns state transitions into desktop notifications.
   *
   * Failures are reported per file; the "queue finished" message fires once when the last
   * running or waiting transfer is gone and at least one file completed since work started.
   */
  function announce(next: readonly Download[]): void {
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

  let refreshTimer: number | null = null
  let historyTimer: number | null = null

  function sampleSpeedHistory(): void {
    speedHistory.value = appendTransferRateHistory(speedHistory.value, {
      measuredAt: Date.now(),
      bytesPerSecond: globalRate.value
    })
  }

  /// Coalesces bursts of events into one refresh per 400 ms.
  function scheduleRefresh(): void {
    if (refreshTimer !== null) return
    refreshTimer = window.setTimeout(() => {
      refreshTimer = null
      // Re-arm instead of stacking a second request on top of one already in flight; a burst
      // of events would otherwise multiply into parallel round trips.
      if (refreshing) return scheduleRefresh()
      void refresh()
    }, 400)
  }

  /// Patches the live stage/percent of one package without a full refresh.
  function applyPostprocessProgress(event: MessageEvent<string>): void {
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
  function applyTorrentStats(event: MessageEvent<string>): void {
    let payload: { download_id?: string, stats?: TorrentAggregateStats }
    try {
      payload = (JSON.parse(event.data) as EventEnvelope<{ download_id?: string, stats?: TorrentAggregateStats }>).payload ?? {}
    } catch {
      return
    }
    if (!payload.download_id || !payload.stats) return
    useTorrentsStore().applyStats(payload.download_id, payload.stats)
  }

  function connectEvents(): void {
    if (releaseEvents) return
    releaseEvents = subscribeEvents({
      'download.progress': scheduleRefresh,
      'download.state': scheduleRefresh,
      'package.state': scheduleRefresh,
      'usenet.changed': scheduleRefresh,
      // Aggregate torrent counters are pushed; the detail panel pulls peers and pieces.
      'torrent.stats': applyTorrentStats,
      'postprocess.progress': applyPostprocessProgress
    })
    // The former 3s safety-net poll is gone: the shared stream reconnects with backoff and
    // refreshes on every state event, so the extra round trips only crowded the connection pool.
    if (historyTimer === null) {
      sampleSpeedHistory()
      historyTimer = window.setInterval(sampleSpeedHistory, 1_000)
    }
  }

  function disconnectEvents(): void {
    releaseEvents?.()
    releaseEvents = null
    if (historyTimer !== null) {
      window.clearInterval(historyTimer)
      historyTimer = null
    }
  }

  /**
   * What a view shows in place of an empty queue: the **first** queue fetch has not settled yet.
   *
   * Deliberately not `pending.value || !settled.value` (RD-106-19). `pending` goes true on
   * every `refresh()`, and refreshes arrive constantly — state events, user actions, the speed
   * history sampler. On an empty queue that swapped the rendered empty state for the loading
   * skeleton for the few milliseconds each round trip takes, over and over: the flicker that
   * was reported. `design.md` promises the loading surface for the *first* fetch, and only
   * that one.
   *
   * `settled` alone is the right answer including for a first fetch that failed: it is set
   * whatever the answer was, so `loading` turns false, `error` is non-null, and `DataState`
   * renders the failure — which it prefers over the empty state. A retry then keeps that error
   * on screen instead of flashing the skeleton at the reader a second time.
   *
   * `pending` itself is untouched and stays exported: it is the per-refresh flag, and that
   * is a different question from the one this answers.
   */
  const loading = computed(() => !settled.value)

  return {
    active,
    activePackages,
    add,
    clear,
    clearing,
    connectEvents,
    control,
    controlAll,
    controlsBusy,
    updatePackages,
    reorderPackages,
    reorderDownloads,
    deletePackages,
    speedLimitMiB,
    speedLimitBusy,
    speedHistory,
    loadSpeedLimit,
    setSpeedLimit,
    renameDownload,
    extractPackages,
    forceExtractPackage,
    renamePackageFolder,
    loadPostprocess,
    bulk,
    reset,
    extractDownloads,
    disconnectEvents,
    downloadRates,
    downloadEtas,
    downloads,
    error,
    notice,
    packages,
    packageComplete,
    packageRates,
    packageEtas,
    pending,
    loading,
    globalRate,
    globalControl,
    queueEta,
    queued,
    remove,
    refresh,
    totalCommitted,
    totalRemaining
  }
})

function payloadError(value: unknown): string {
  return typeof value === 'object' && value !== null && 'error' in value && typeof value.error === 'string'
    ? value.error
    : t('downloads.notices.remove_failed')
}

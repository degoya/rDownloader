import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import type { Download, DownloadBulkAction, DownloadPackage, DownloadRates } from '@/api/types'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { serverMessageFrom } from '@/i18n/server'
import { batchError, combinedMessage, inBatches } from '@/utils/bulkBatches'
import {
  appendTransferRateHistory,
  type TransferRateHistoryPoint
} from '@/utils/transferRates'

import { useClearList } from './transfersClear'
import { applyPostprocessProgress, applyTorrentStats, createQueueAnnouncer } from './transfersEvents'
import { useTransferFigures } from './transfersFigures'
import { usePackageActions } from './transfersPackages'
import {
  PAUSABLE_STATES,
  RESUMABLE_STATES,
  bulkRefusals,
  payloadError,
  t,
  type DownloadSelection
} from './transfersShared'
import { useSpeedLimit } from './transfersSpeedLimit'

// The store is split over `transfersShared`, `transfersFigures`, `transfersEvents`,
// `transfersPackages` (RD-140-27), `transfersClear` and `transfersSpeedLimit` (WEB-13); these
// stay importable from here, where callers look for them.
export { PAUSABLE_STATES, PENDING_STATES, RESETTABLE_STATES, RESUMABLE_STATES } from './transfersShared'
export type { ClearScope, PackageChange } from './transfersShared'

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
  const controlsBusy = ref(false)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  const notice = ref<string | null>(null)
  /** What was left untouched or refused; the view keeps it until it is closed (RD-1220-03). */
  const warning = ref<string | null>(null)
  const downloadRates = ref<Record<string, number>>({})
  /** Seconds left per file, as the server measured them. Absent means "nothing to say". */
  const downloadEtas = ref<Record<string, number>>({})
  /** Queued files the service holds back for a connection to their host, with that host (RD-1130-02). */
  const downloadHostWaits = ref<Record<string, string>>({})
  const globalRate = ref(0)
  /** Seconds until the queue is through at the current rate; `null` when no honest figure exists. */
  const queueEta = ref<number | null>(null)
  const speedHistory = ref<TransferRateHistoryPoint[]>([])
  /** Monotonic ticket so only the newest `refresh()` may apply its result (see `refresh`). */
  let refreshTicket = 0
  /** True while a refresh is awaiting the network; event bursts wait rather than pile up. */
  let refreshing = false
  /**
   * The error the last failed `refresh()` put up, the only one a later successful refresh may
   * take down. Refreshes follow every queue event, so clearing whatever `error` held wiped an
   * action's refusal within 400 ms, and a refused cancel or removal looked like nothing at all.
   */
  let loadError: string | null = null

  const { active, queued, totalCommitted, totalRemaining, globalControl, activePackages, packageComplete, packageRates, packageEtas } =
    useTransferFigures(downloads, packages, downloadRates)
  const announce = createQueueAnnouncer()

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
    const hostWaits: Record<string, string> = {}
    for (const wait of payload.waiting_for_host ?? []) hostWaits[wait.id] = wait.host
    downloadRates.value = rates
    downloadEtas.value = etas
    downloadHostWaits.value = hostWaits
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
    try {
      const [downloadResponse, packageResponse, rateResponse] = await Promise.all([
        api.GET('/api/v1/downloads'),
        api.GET('/api/v1/packages'),
        api.GET('/api/v1/downloads/rates')
      ])
      if (ticket !== refreshTicket) return
      applyRates(rateResponse.data)
      if (downloadResponse.data && packageResponse.data) {
        announce(downloadResponse.data)
        downloads.value = downloadResponse.data
        packages.value = packageResponse.data
        if (error.value === loadError) error.value = null
        loadError = null
      } else {
        loadError = responseError(downloadResponse.data ? packageResponse : downloadResponse)
        error.value = loadError
      }
    } catch {
      if (ticket !== refreshTicket) return
      loadError = responseError(undefined)
      error.value = loadError
    } finally {
      // Whatever happened, the in-flight flag comes down: a rejection that left it set parked
      // every later event behind the event debounce, and the queue froze until a reload.
      if (ticket === refreshTicket) {
        refreshing = false
        pending.value = false
        settled.value = true
      }
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
    const response = await api.DELETE('/api/v1/downloads/{id}', { params: { path: { id } } })
    if (!response.data) {
      error.value = payloadError(response.error)
      return false
    }
    downloads.value = downloads.value.filter(download => download.id !== id)
    error.value = null
    return true
  }

  /**
   * Applies one action to many files server-side; returns the number of affected files.
   *
   * More files than one request may carry go in batches, and what they did and refused is
   * reported as one outcome.
   */
  async function bulk(ids: string[], action: DownloadBulkAction): Promise<number> {
    if (!ids.length) return 0
    const run = await inBatches(ids, batch => api.POST('/api/v1/downloads/bulk', { body: { ids: batch, action } }))
    await refresh()
    const refused = bulkRefusals({
      errors: run.data.flatMap(result => result.errors),
      refusals: run.data.flatMap(result => result.refusals)
    })
    error.value = [batchError(run), refused].filter(Boolean).join(' · ') || null
    return run.data.reduce((sum, result) => sum + result.affected, 0)
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
    // A batch without a completed file is refused on its own; that is no reason to stop the
    // batches that have some, so it counts as an empty answer unless every batch gives it.
    let noneCompleted: unknown = null
    const run = await inBatches(ids, async batch => {
      const response = await api.POST('/api/v1/downloads/extract', { body: { ids: batch } })
      if (response.data || serverMessageFrom(response.error)?.code !== 'download.none_completed') return response
      noneCompleted = response
      return { data: null }
    })
    const queued = run.data.filter(body => body !== null)
    if (!queued.length) {
      error.value = run.failure ? responseError(run.failure) : responseError(noneCompleted)
      return false
    }
    notice.value = combinedMessage(queued)
    error.value = batchError(run)
    return !run.failure
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

  const { extractPackages, forceExtractPackage, loadPostprocess, updatePackages, renamePackageFolder, deletePackages, reorderPackages, reorderDownloads } =
    usePackageActions({ error, notice, refresh })
  const { clear, clearing } = useClearList({ error, notice, refresh })
  const {
    applyRailSettings, loadRailSettings, setSpeedLimit, speedLimitBusy, speedLimitMiB,
    maxActiveFiles, maxActiveFilesBusy, setMaxActiveFiles, loadPackageSpeedLimit, setPackageSpeedLimit
  } = useSpeedLimit({ error, notice })

  let historyTimer: number | null = null

  function sampleSpeedHistory(): void {
    speedHistory.value = appendTransferRateHistory(speedHistory.value, {
      measuredAt: Date.now(),
      bytesPerSecond: globalRate.value
    })
  }

  /** Coalesces bursts of events into one refresh per 400 ms. */
  const events = debouncedEventRefresh(
    ['download.progress', 'download.state', 'package.state', 'usenet.changed'],
    refresh,
    {
      delayMs: 400,
      busy: () => refreshing,
      handlers: {
        // Aggregate torrent counters are pushed; the detail panel pulls peers and pieces.
        'torrent.stats': applyTorrentStats,
        'postprocess.progress': (event: MessageEvent<string>) => applyPostprocessProgress(packages, event)
      }
    }
  )

  function connectEvents(): void {
    events.connect()
    // The former 3s safety-net poll is gone: the shared stream reconnects with backoff and
    // refreshes on every state event, so the extra round trips only crowded the connection pool.
    if (historyTimer === null) {
      sampleSpeedHistory()
      historyTimer = window.setInterval(sampleSpeedHistory, 1_000)
    }
  }

  function disconnectEvents(): void {
    events.disconnect()
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
    applyRailSettings,
    loadRailSettings,
    maxActiveFiles,
    maxActiveFilesBusy,
    setMaxActiveFiles,
    loadPackageSpeedLimit,
    setPackageSpeedLimit,
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
    downloadHostWaits,
    downloads,
    error,
    notice,
    warning,
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

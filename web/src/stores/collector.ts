import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useNotifications } from '@/composables/useNotifications'
import { i18n } from '@/i18n'
import type { CollectorPackage, LinkCandidate } from '@/api/types'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { type PickListing, useSitePicksStore } from '@/stores/sitePicks'
import { batchError, inBatches } from '@/utils/bulkBatches'

import { useCandidateActions } from './collectorCandidates'
import { useLinkFilterActions } from './collectorLinkFilters'
import { useMirrorActions } from './collectorMirrors'
import {
  changeBody,
  type CollectorPackageChange,
  type EnqueueBatchResult,
  type GrabberOrderEntry,
  type IntakeInput,
  type IntakeOutcome
} from './collectorShared'

// The store is split over `collectorShared`, `collectorCandidates` and `collectorMirrors`
// (RD-140-27); these stay importable from here, where callers look for them.
export type { CollectorPackageChange, EnqueueBatchResult, GrabberOrderEntry, IntakeInput, IntakeOutcome } from './collectorShared'

/** Stored server-side as the batch origin; deliberately not translated. */
const WEB_UI_SOURCE_LABEL = 'Web UI'
/** Link deletions in flight at once; the server writes them one after another anyway. */
const DELETE_CONCURRENCY = 4
/**
 * What a paste answers when all it found was a series page whose releases wait for a choice
 * (RD-1170-03): not a failure, the pick board holds the list.
 */
const PICK_WAITING = 'site_rules.pick_waiting'

export const useCollectorStore = defineStore('collector', () => {
  const packages = ref<CollectorPackage[]>([])
  const candidates = ref<LinkCandidate[]>([])
  const pending = ref(false)
  /** True while `refresh()` is awaiting the network — the enqueue flag above covers actions only. */
  const fetching = ref(false)
  /**
   * True once the first refresh has settled, so a view can tell "still asking" from "nothing
   * collected". `fetching` alone is `false` before the first call and would let the empty
   * state render for the first frames of a mount (RD-104-07).
   */
  const settled = ref(false)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  const enqueuingIds = ref<Set<string>>(new Set())
  const deletingIds = ref<Set<string>>(new Set())
  /** True while a refresh is awaiting the network; event bursts wait rather than pile up. */
  let refreshing = false
  /** Monotonic ticket so a late response from an older refresh cannot overwrite a newer one. */
  let refreshTicket = 0

  async function refresh(): Promise<void> {
    const nzb = useNzbImportsStore()
    const ticket = ++refreshTicket
    refreshing = true
    fetching.value = true
    try {
      const [packageResponse, candidateResponse] = await Promise.all([
        api.GET('/api/v1/collector/packages'),
        api.GET('/api/v1/collector/candidates'),
        nzb.refresh()
      ])
      if (ticket !== refreshTicket) return
      if (packageResponse.data && candidateResponse.data) {
        packages.value = packageResponse.data
        candidates.value = candidateResponse.data
        error.value = null
      } else {
        error.value = responseError(packageResponse.data ? candidateResponse : packageResponse)
      }
    } catch {
      if (ticket === refreshTicket) error.value = responseError(undefined)
    } finally {
      // A rejection must not leave the flag set: the event debounce would re-arm behind it for
      // good and the LinkGrabber would stop following events until a reload.
      if (ticket === refreshTicket) {
        refreshing = false
        fetching.value = false
        settled.value = true
      }
    }
  }


  const { renameCandidate, setMediaVariant, fetchMediaFormats, previewMediaSelection, previewMediaOutput, setAuthProfile, setMediaSelection, replayPreview, grantReplayConsent, revokeReplayConsent } =
    useCandidateActions({ candidates, error, refresh })
  const { mirrorPreference, loadMirrorPreference, setMirrorPreference, chooseMirror, dissolveMirror } =
    useMirrorActions({ error, refresh })
  const { applyLinkFilters, unhideCandidates } = useLinkFilterActions({ error, refresh })

  const notifications = useNotifications()

  /**
   * Raises the desktop notification for freshly arrived links.
   *
   * The server emits `collector.intake` for every intake path (web UI, browser extension,
   * hotfolder, subscriptions), so this covers all of them. The capture agent shows the same
   * message, but it is a separate process that is often not running — the web UI should not
   * depend on it.
   */
  function announceIntake(event: MessageEvent<string>): void {
    let payload: { candidate_count?: number, package_count?: number }
    try {
      payload = (JSON.parse(event.data) as { payload?: typeof payload }).payload ?? {}
    } catch {
      return
    }
    const links = payload.candidate_count ?? 0
    if (links <= 0) return
    notifications.notify(
      i18n.global.t('linkgrabber.notifications.intake_title'),
      i18n.global.t('linkgrabber.notifications.intake_body', { count: links }, links)
    )
  }

  async function collect(input: IntakeInput): Promise<IntakeOutcome> {
    pending.value = true
    const response = await api.POST('/api/v1/collector/batches', {
      body: {
        source: 'manual',
        source_label: WEB_UI_SOURCE_LABEL,
        text: input.text,
        ...(input.packageName ? { package_name: input.packageName } : {}),
        ...(input.password ? { password: input.password } : {})
      }
    })
    pending.value = false
    if (!response.data) {
      const failure = response.error as { code?: string, params?: Record<string, string> } | undefined
      if (failure?.code === PICK_WAITING) {
        error.value = null
        void useSitePicksStore().listed()
        const listed = Number(failure.params?.entries ?? 0)
        return { ok: true, skippedExcluded: 0, skippedDisabled: 0, crawledFound: 0, crawledDropped: 0, listed }
      }
      error.value = responseError(response)
      return { ok: false, skippedExcluded: 0, skippedDisabled: 0, crawledFound: 0, crawledDropped: 0 }
    }
    error.value = null
    const skippedExcluded = response.data.skipped_excluded
    const skippedDisabled = response.data.skipped_disabled
    const crawledFound = response.data.crawled_found
    const crawledDropped = response.data.crawled_dropped
    await refresh()
    return { ok: true, skippedExcluded, skippedDisabled, crawledFound, crawledDropped }
  }

  async function updatePackages(ids: string[], change: CollectorPackageChange): Promise<boolean> {
    if (!ids.length) return false
    if (ids.length === 1 && ids[0]) {
      const response = await api.PATCH('/api/v1/collector/packages/{id}', { params: { path: { id: ids[0] } }, body: changeBody(change) })
      if (!response.data) {
        error.value = responseError(response)
        return false
      }
      error.value = null
      await refresh()
      return true
    }
    // The bulk routes take at most 500 ids; a larger selection goes in batches.
    const run = await inBatches(ids, batch => api.POST('/api/v1/collector/packages/bulk', { body: { ids: batch, ...changeBody(change) } }))
    if (run.data.length) await refresh()
    error.value = batchError(run)
    return !run.failure
  }

  /**
   * Stores the manual order of the whole LinkGrabber list — collector packages and NZB imports
   * in one sequence, which is what lets an NZB row be dragged at all.
   *
   * `after` is the row the listed ones are placed behind, `null` the head of the list. Naming it
   * is what keeps the request the size of the move instead of the size of the list.
   */
  async function reorderEntries(entries: GrabberOrderEntry[], after: GrabberOrderEntry | null = null): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/entries/reorder', { body: { entries, after } })
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  async function reorderCandidates(packageId: string, ids: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/reorder', { body: { package_id: packageId, ids } })
    if (!response.data) error.value = responseError(response)
    return Boolean(response.data)
  }

  /**
   * Moves links into a package. In batches, only the first one may create a new package: the
   * ones after it go into the package that batch created, not into a namesake of their own.
   */
  async function moveCandidates(ids: string[], target: { packageId?: string, newPackageName?: string }): Promise<boolean> {
    let packageId = target.packageId
    const run = await inBatches(ids, async batch => {
      const response = await api.POST('/api/v1/collector/candidates/move', {
        body: { ids: batch, ...(packageId ? { package_id: packageId } : {}), ...(!packageId && target.newPackageName ? { new_package_name: target.newPackageName } : {}) }
      })
      packageId ??= response.data?.id
      return response
    })
    await refresh()
    // After the refresh, which clears `error` when the lists load.
    if (run.failure) error.value = batchError(run)
    return !run.failure
  }

  async function checkLinks(ids?: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/check', { body: { ids: ids ?? null } })
    if (!response.data) error.value = responseError(response)
    return Boolean(response.data)
  }

  async function regroup(): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/packages/regroup')
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  /**
   * Hands packages to the queue. `candidateIds` narrows it to those of their links — what the
   * LinkGrabber shows while a filter hides the rest, which then stay behind in their package.
   */
  async function enqueuePackages(ids: string[], paused = false, candidateIds?: string[]): Promise<EnqueueBatchResult> {
    const empty: EnqueueBatchResult = { created: 0, links: 0, failed: 0, firstError: null, freeDownloadFiles: 0 }
    if (!ids.length) return empty
    pending.value = true
    enqueuingIds.value = new Set(ids)
    // Read before the request: a refused batch refreshes the list, and the links that did go
    // are counted from what was there when they were sent.
    const before = candidates.value
    const run = await inBatches(ids, batch => api.POST('/api/v1/collector/packages/enqueue', {
      body: { ids: batch, paused, ...(candidateIds ? { candidate_ids: candidateIds } : {}) }
    }))
    enqueuingIds.value = new Set()
    pending.value = false
    if (run.failure) {
      await refresh()
      error.value = batchError(run)
      if (!run.data.length) return empty
    } else {
      error.value = null
    }
    // Drop the enqueued packages locally instead of waiting for a round trip: the caller
    // refreshes both lists straight afterwards, so re-reading here only delayed the rows
    // disappearing by the length of three more GETs.
    // Only what was sent goes: a package that kept links the filter hid keeps its row.
    const enqueued = new Set(run.data.flatMap((result, index) => result.created.length ? run.sent[index] ?? [] : []))
    const sent = candidateIds ? new Set(candidateIds) : null
    const went = (item: LinkCandidate): boolean => Boolean(item.package_id && enqueued.has(item.package_id) && (sent === null || sent.has(item.id)))
    if (enqueued.size) {
      candidates.value = candidates.value.filter(item => !went(item))
      const kept = new Set(candidates.value.map(item => item.package_id))
      packages.value = packages.value.filter(item => !enqueued.has(item.id) || kept.has(item.id))
    }
    return {
      created: run.data.reduce((sum, result) => sum + result.created.length, 0),
      links: enqueued.size ? before.filter(went).length : 0,
      failed: run.data.reduce((sum, result) => sum + result.failed, 0),
      firstError: run.data.find(result => result.first_error)?.first_error ?? null,
      freeDownloadFiles: run.data.reduce((sum, result) => sum + result.free_download_files, 0)
    }
  }

  async function enqueueCandidate(id: string): Promise<boolean> {
    if (enqueuingIds.value.has(id)) return false
    enqueuingIds.value = new Set([...enqueuingIds.value, id])
    const response = await api.POST('/api/v1/collector/candidates/{id}/enqueue', { params: { path: { id } } })
    const next = new Set(enqueuingIds.value)
    next.delete(id)
    enqueuingIds.value = next
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  async function deleteCandidate(id: string): Promise<boolean> {
    if (deletingIds.value.has(id)) return false
    deletingIds.value = new Set([...deletingIds.value, id])
    const response = await api.DELETE('/api/v1/collector/candidates/{id}', { params: { path: { id } } })
    const next = new Set(deletingIds.value)
    next.delete(id)
    deletingIds.value = next
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  /**
   * Deletes these links, a few requests at a time, and reads the list once at the end.
   *
   * There is no route for a chosen set of links, and one request after another — each followed
   * by a full refresh — took 70 s for 392 links with nothing on screen moving. `onProgress` is
   * told how many have been answered.
   */
  async function deleteCandidates(ids: readonly string[], onProgress?: (done: number) => void): Promise<number> {
    const queue = ids.filter(id => !deletingIds.value.has(id))
    if (!queue.length) return 0
    deletingIds.value = new Set([...deletingIds.value, ...queue])
    const removed = new Set<string>()
    let failure: unknown = null
    let next = 0
    let answered = 0
    async function worker(): Promise<void> {
      while (next < queue.length) {
        const id = queue[next++] as string
        const response = await api.DELETE('/api/v1/collector/candidates/{id}', { params: { path: { id } } })
        if (response.data) removed.add(id)
        else failure ??= response
        onProgress?.(++answered)
      }
    }
    await Promise.all(Array.from({ length: Math.min(DELETE_CONCURRENCY, queue.length) }, worker))
    const left = new Set(deletingIds.value)
    for (const id of queue) left.delete(id)
    deletingIds.value = left
    candidates.value = candidates.value.filter(item => !removed.has(item.id))
    await refresh()
    // After the refresh, which clears `error` when the lists load.
    if (failure) error.value = responseError(failure)
    return removed.size
  }

  async function deletePackage(id: string): Promise<boolean> {
    const response = await api.DELETE('/api/v1/collector/packages/{id}', { params: { path: { id } } })
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  async function clearCandidates(): Promise<boolean> {
    pending.value = true
    const response = await api.DELETE('/api/v1/collector/candidates')
    pending.value = false
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  /**
   * The LinkGrabber changed. One kind of change is news of its own (RD-1190-17): a page whose
   * releases wait for a choice, listed by whichever intake — a copied link, the browser
   * extension, Click'n'Load — goes to the pick board, which opens its drawer.
   */
  function collectorChanged(event: MessageEvent<string>): void {
    events.schedule()
    let listing: PickListing | undefined
    try {
      listing = (JSON.parse(event.data) as { payload?: { pick_listed?: PickListing } }).payload?.pick_listed
    } catch {
      return
    }
    if (listing?.list) void useSitePicksStore().announced(listing)
  }

  const events = debouncedEventRefresh(
    ['usenet.changed', 'category.changed'],
    refresh,
    {
      busy: () => refreshing,
      // Links arriving from anywhere (web UI, extension, hotfolder, subscriptions) announce
      // themselves here; the desktop toast is raised from the same envelope.
      handlers: { 'collector.intake': announceIntake, 'collector.changed': collectorChanged }
    }
  )

  /** The NZB imports follow their remote jobs too, for the hand-over badge (RD-191-13). */
  function connectEvents(): void {
    events.connect()
    useNzbImportsStore().connectEvents()
  }

  function disconnectEvents(): void {
    events.disconnect()
    useNzbImportsStore().disconnectEvents()
  }

  /**
   * The collector's own fetch, as a view has to show it: the **first** one has not settled yet.
   *
   * Not `fetching.value || !settled.value` (RD-106-19). `fetching` goes true on every
   * `refresh()`, and collector refreshes are driven by event bursts, so on an empty list the
   * empty state was replaced by the loading skeleton for the length of each round trip and
   * came straight back — the reported flicker. `design.md` promises the loading surface for
   * the first fetch only.
   *
   * A first fetch that failed sets `settled` too, so `loading` turns false and `DataState`
   * shows the error it prefers over the empty state; a retry leaves that error standing rather
   * than flashing the skeleton again. `fetching` itself is untouched; it stays the
   * per-refresh flag, as `pending` stays the enqueue flag the buttons bind.
   */
  const loading = computed(() => !settled.value)

  return {
    packages,
    candidates,
    pending,
    loading,
    error,
    enqueuingIds,
    deletingIds,
    refresh,
    collect,
    updatePackages,
    reorderEntries,
    reorderCandidates,
    moveCandidates,
    renameCandidate,
    setMediaVariant,
    fetchMediaFormats,
    previewMediaSelection,
    previewMediaOutput,
    setAuthProfile,
    setMediaSelection,
    checkLinks,
    regroup,
    mirrorPreference,
    loadMirrorPreference,
    setMirrorPreference,
    chooseMirror,
    dissolveMirror,
    applyLinkFilters,
    unhideCandidates,
    enqueuePackages,
    enqueueCandidate,
    replayPreview,
    grantReplayConsent,
    revokeReplayConsent,
    deleteCandidate,
    deleteCandidates,
    deletePackage,
    clearCandidates,
    connectEvents,
    disconnectEvents
  }
})

import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { subscribeEvents } from '@/composables/useEventStream'
import { useNotifications } from '@/composables/useNotifications'
import { i18n } from '@/i18n'
import type { CandidateAuthProfileMode, CollectorPackage, CollectorPackageUpdateRequest, DownloadPriority, GrabberEntryRef, LinkCandidate, MediaFormatCriteria, MediaFormatsResponse, MediaOutputPreview, MediaResolution, PostprocessLevel, ReplayPreview } from '@/api/types'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { EMPTY_PREFERENCE, type MirrorPreference } from '@/utils/mirrorGroups'

/** Stored server-side as the batch origin; deliberately not translated. */
const WEB_UI_SOURCE_LABEL = 'Web UI'

export interface CollectorPackageChange {
  name?: string
  categoryId?: string | null
  priority?: DownloadPriority
  /** null clears the password */
  password?: string | null
  /** null clears the package level (inherit category/global default) */
  postprocessLevel?: PostprocessLevel | null
  /** null clears the package script (inherit category/global default) */
  script?: string | null
}

/**
 * One row of the LinkGrabber's manual order. The order spans both kinds of entry.
 *
 * The shape is the contract's, not a copy of it: this was hand-written while the endpoint was
 * still being built, and a second declaration of a generated type is one that drifts from it
 * silently.
 */
export type GrabberOrderEntry = GrabberEntryRef

export interface IntakeInput {
  text: string
  packageName?: string
  password?: string
}

export interface IntakeOutcome {
  ok: boolean
  /** Links dropped by the domain blocklist before candidates were created. */
  skippedExcluded: number
  /** Links refused because their transfer service is switched off. */
  skippedDisabled: number
  /** Addresses the folder crawlers and site rules returned for this paste. */
  crawledFound: number
  /**
   * How many of those were refused: nothing claims them and no probe confirmed a file
   * behind them (RD-110-07). Reported rather than swallowed — a rule whose pattern reaches
   * one element too far otherwise looks exactly like an empty page.
   */
  crawledDropped: number
}

/** Outcome of a batch enqueue, including partial failures and account-less files. */
export interface EnqueueBatchResult {
  created: number
  failed: number
  firstError: string | null
  /** Files enqueued without a provider account (free/direct download attempt). */
  freeDownloadFiles: number
}

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
  const enqueuingIds = ref<Set<string>>(new Set())
  const deletingIds = ref<Set<string>>(new Set())
  let releaseEvents: (() => void) | null = null
  let refreshTimer: number | null = null
  /** True while a refresh is awaiting the network; event bursts wait rather than pile up. */
  let refreshing = false
  /** Monotonic ticket so a late response from an older refresh cannot overwrite a newer one. */
  let refreshTicket = 0

  async function refresh(): Promise<void> {
    const nzb = useNzbImportsStore()
    const ticket = ++refreshTicket
    refreshing = true
    fetching.value = true
    const [packageResponse, candidateResponse] = await Promise.all([
      api.GET('/api/v1/collector/packages'),
      api.GET('/api/v1/collector/candidates'),
      nzb.refresh()
    ])
    if (ticket !== refreshTicket) return
    refreshing = false
    fetching.value = false
    settled.value = true
    if (packageResponse.data && candidateResponse.data) {
      packages.value = packageResponse.data
      candidates.value = candidateResponse.data
      error.value = null
    } else {
      error.value = responseError(packageResponse.data ? candidateResponse : packageResponse)
    }
  }

  function scheduleRefresh(): void {
    if (refreshTimer !== null) return
    refreshTimer = window.setTimeout(() => {
      refreshTimer = null
      // Re-arm instead of stacking a second request on top of one already in flight; a burst
      // of events would otherwise multiply into parallel round trips.
      if (refreshing) return scheduleRefresh()
      void refresh()
    }, 300)
  }

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

  function changeBody(change: CollectorPackageChange): CollectorPackageUpdateRequest {
    return {
      ...(change.name ? { name: change.name } : {}),
      ...(change.categoryId ? { category_id: change.categoryId } : {}),
      ...(change.categoryId === null ? { clear_category: true } : {}),
      ...(change.priority ? { priority: change.priority } : {}),
      ...(change.password ? { password: change.password } : {}),
      ...(change.password === null ? { clear_password: true } : {}),
      ...(change.postprocessLevel ? { postprocess_level: change.postprocessLevel } : {}),
      ...(change.postprocessLevel === null ? { clear_postprocess_level: true } : {}),
      ...(change.script ? { script: change.script } : {}),
      ...(change.script === null ? { clear_script: true } : {})
    }
  }

  async function updatePackages(ids: string[], change: CollectorPackageChange): Promise<boolean> {
    if (!ids.length) return false
    const response = ids.length === 1 && ids[0]
      ? await api.PATCH('/api/v1/collector/packages/{id}', { params: { path: { id: ids[0] } }, body: changeBody(change) })
      : await api.POST('/api/v1/collector/packages/bulk', { body: { ids, ...changeBody(change) } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
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

  async function moveCandidates(ids: string[], target: { packageId?: string, newPackageName?: string }): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/move', {
      body: { ids, ...(target.packageId ? { package_id: target.packageId } : {}), ...(target.newPackageName ? { new_package_name: target.newPackageName } : {}) }
    })
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  async function patchCandidate(id: string, body: { file_name?: string, media_variant?: string }): Promise<boolean> {
    const response = await api.PATCH('/api/v1/collector/candidates/{id}', { params: { path: { id } }, body })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    candidates.value = candidates.value.map(candidate => candidate.id === id ? response.data! : candidate)
    return true
  }

  async function renameCandidate(id: string, fileName: string): Promise<boolean> {
    return patchCandidate(id, { file_name: fileName })
  }

  async function setMediaVariant(id: string, variantId: string): Promise<boolean> {
    return patchCandidate(id, { media_variant: variantId })
  }

  /** Full format inventory of one media candidate, fetched only when the selector opens. */
  async function fetchMediaFormats(id: string): Promise<MediaFormatsResponse | null> {
    const response = await api.GET('/api/v1/collector/candidates/{id}/media', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    return response.data
  }

  /**
   * What a set of criteria would resolve to, without storing it.
   *
   * A refusal is returned rather than raised: "nothing matches" is the answer the selector
   * needs to render its explanation, not an error to swallow. Its stable code travels along,
   * because "no formats" and "no audio track" call for different advice than a filter
   * combination that keeps nothing (RD-120-50).
   */
  async function previewMediaSelection(
    id: string,
    criteria: MediaFormatCriteria
  ): Promise<{ resolution: MediaResolution | null, code: string | null }> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/media/preview', {
      params: { path: { id } },
      body: { criteria }
    })
    if (response.data) return { resolution: response.data, code: null }
    const failure = response.error as { code?: string | null } | undefined
    return { resolution: null, code: failure?.code ?? null }
  }

  /**
   * What an output template expands to for one link.
   *
   * Previewed on the server through the same evaluator the download uses, so the path shown
   * is the path that gets written. An invalid template returns its reason as the error.
   */
  async function previewMediaOutput(id: string, template: string): Promise<MediaOutputPreview | { error: string }> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/media/output-preview', {
      params: { path: { id } },
      body: { template }
    })
    return response.data ?? { error: responseError(response) }
  }

  /**
   * Chooses the cookie profile a link is queued with (RD-080-04).
   *
   * `auto` lets the scope decide, `none` deliberately sends nothing, `pinned` names one.
   * The server refuses a profile that does not cover the link, so the error is worth
   * surfacing rather than swallowing.
   */
  async function setAuthProfile(
    id: string,
    mode: CandidateAuthProfileMode,
    profileId?: string
  ): Promise<boolean> {
    const response = await api.PUT('/api/v1/collector/candidates/{id}/auth-profile', {
      params: { path: { id } },
      body: { mode, profile_id: profileId ?? null }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    candidates.value = candidates.value.map(candidate => candidate.id === id ? response.data! : candidate)
    return true
  }

  async function setMediaSelection(id: string, criteria: MediaFormatCriteria): Promise<boolean> {
    const response = await api.PUT('/api/v1/collector/candidates/{id}/media/selection', {
      params: { path: { id } },
      body: { criteria }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    candidates.value = candidates.value.map(candidate => candidate.id === id ? response.data! : candidate)
    return true
  }

  async function checkLinks(ids?: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/check', { body: { ids: ids ?? null } })
    if (!response.data) error.value = responseError(response)
    return Boolean(response.data)
  }

  /**
   * The standing mirror preference (RD-110-19).
   *
   * Server state, not view state: it decides which member of every group the queue will
   * fetch, so it has to outlive this tab and reach the next package that arrives. The store
   * holds the last answer the server gave, never a value the interface hopes it stored.
   */
  const mirrorPreference = ref<MirrorPreference>({ ...EMPTY_PREFERENCE })

  async function loadMirrorPreference(): Promise<void> {
    const response = await api.GET('/api/v1/collector/mirror-preference')
    if (!response.data) return
    mirrorPreference.value = {
      quality: response.data.quality ?? null,
      language: response.data.language ?? null,
      hoster: response.data.hoster ?? null,
      hidden_hosters: response.data.hidden_hosters ?? []
    }
  }

  async function setMirrorPreference(preference: MirrorPreference): Promise<boolean> {
    const response = await api.PUT('/api/v1/collector/mirror-preference', {
      body: {
        quality: preference.quality ?? null,
        language: preference.language ?? null,
        hoster: preference.hoster ?? null,
        hidden_hosters: preference.hidden_hosters
      }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    mirrorPreference.value = {
      quality: response.data.quality ?? null,
      language: response.data.language ?? null,
      hoster: response.data.hoster ?? null,
      hidden_hosters: response.data.hidden_hosters ?? []
    }
    // The server re-chose every group under the new preference, so the rows on screen are the
    // previous answer until this lands.
    await refresh()
    return true
  }

  /** Chooses one mirror of a group by hand, or hands the group back to the preference. */
  async function chooseMirror(id: string, chosen: boolean): Promise<boolean> {
    const response = chosen
      ? await api.POST('/api/v1/collector/candidates/{id}/mirror', { params: { path: { id } } })
      : await api.DELETE('/api/v1/collector/candidates/{id}/mirror', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  /**
   * Takes a proposed mirror group apart, so its links stand on their own again (RD-110-34).
   *
   * Only a proposal can be taken apart, and the server is the one that says so: a declared or
   * a name-and-size group is refused there with `collector.mirror_group_not_proposed`, so a
   * row that offers the action wrongly still changes nothing.
   */
  async function dissolveMirror(id: string): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/mirror/dissolve', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
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
    const empty: EnqueueBatchResult = { created: 0, failed: 0, firstError: null, freeDownloadFiles: 0 }
    if (!ids.length) return empty
    pending.value = true
    enqueuingIds.value = new Set(ids)
    const response = await api.POST('/api/v1/collector/packages/enqueue', {
      body: { ids, paused, ...(candidateIds ? { candidate_ids: candidateIds } : {}) }
    })
    enqueuingIds.value = new Set()
    pending.value = false
    if (!response.data) {
      error.value = responseError(response)
      await refresh()
      return empty
    }
    error.value = null
    // Drop the enqueued packages locally instead of waiting for a round trip: the caller
    // refreshes both lists straight afterwards, so re-reading here only delayed the rows
    // disappearing by the length of three more GETs.
    // Only what was sent goes: a package that kept links the filter hid keeps its row.
    const enqueued = new Set(response.data.created.length ? ids : [])
    if (enqueued.size) {
      const sent = candidateIds ? new Set(candidateIds) : null
      candidates.value = candidates.value.filter(item => !item.package_id || !enqueued.has(item.package_id) || (sent !== null && !sent.has(item.id)))
      const kept = new Set(candidates.value.map(item => item.package_id))
      packages.value = packages.value.filter(item => !enqueued.has(item.id) || kept.has(item.id))
    }
    return {
      created: response.data.created.length,
      failed: response.data.failed,
      firstError: response.data.first_error ?? null,
      freeDownloadFiles: response.data.free_download_files
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

  /** What a captured request would send, for the consent dialog. */
  async function replayPreview(id: string): Promise<ReplayPreview | null> {
    const response = await api.GET('/api/v1/collector/candidates/{id}/replay-preview', {
      params: { path: { id } }
    })
    if (!response.data) {
      error.value = responseError(response)
      return null
    }
    return response.data
  }

  /**
   * Approves one captured request.
   *
   * The hash binds the approval to the template the user actually saw; the server refuses
   * it if the capture changed in between.
   */
  async function grantReplayConsent(
    id: string,
    templateHash: string,
    approvedOrigins: string[]
  ): Promise<boolean> {
    const response = await api.POST('/api/v1/collector/candidates/{id}/replay-consent', {
      params: { path: { id } },
      body: { template_hash: templateHash, approved_origins: approvedOrigins }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  /**
   * Withdraws an approval that was granted but never enqueued.
   *
   * The enqueue skips the dialog for a candidate that already carries a consent, so an
   * approval left behind by a cancelled or failed enqueue would send the credentials on the
   * next attempt without asking again. This is the way back out.
   */
  async function revokeReplayConsent(id: string): Promise<boolean> {
    const response = await api.DELETE('/api/v1/collector/candidates/{id}/replay-consent', {
      params: { path: { id } }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
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

  function connectEvents(): void {
    if (releaseEvents) return
    releaseEvents = subscribeEvents({
      'collector.changed': scheduleRefresh,
      'usenet.changed': scheduleRefresh,
      'category.changed': scheduleRefresh,
      // Links arriving from anywhere (web UI, extension, hotfolder, subscriptions) announce
      // themselves here; the desktop toast is raised from the same envelope.
      'collector.intake': announceIntake
    })
  }

  function disconnectEvents(): void {
    releaseEvents?.()
    releaseEvents = null
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
    enqueuePackages,
    enqueueCandidate,
    replayPreview,
    grantReplayConsent,
    revokeReplayConsent,
    deleteCandidate,
    deletePackage,
    clearCandidates,
    connectEvents,
    disconnectEvents
  }
})

import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type {
  AuthProfile,
  CandidateAuthProfileMode,
  LinkCandidate,
  MediaFormatCriteria,
  MediaFormatsResponse,
  ResolvedRemoteListing,
  TorrentPlanRequest
} from '@/api/types'
import type MediaFormatSelector from '@/components/MediaFormatSelector.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useCollectorStore } from '@/stores/collector'
import { useTorrentsStore } from '@/stores/torrents'

/**
 * The details panel of one LinkGrabber row: the captured request, the torrent and remote
 * trees, the media format inventory and the replay approval (`CollectorCandidateRow`).
 *
 * Everything heavy is fetched the first time the row is opened and kept while it is closed, so
 * a second opening costs nothing.
 */
export function useCandidateDetails(candidate: () => LinkCandidate) {
  const { t } = useI18n()
  const collector = useCollectorStore()
  const torrents = useTorrentsStore()

  /** Captured browser-download request; shown read-only so the user sees it before queueing. */
  const request = computed(() => candidate().request ?? null)
  const media = computed(() => candidate().media ?? null)
  const expanded = ref(false)

  /**
   * An approval that was granted but whose link is still here.
   *
   * The enqueue asks only once: a candidate that already carries a consent is queued without
   * the dialog. An approval left behind by a cancelled or failed enqueue would therefore send
   * the captured credentials on the next attempt in silence, so it is named in the row and can
   * be taken back from the details panel.
   */
  const consent = computed(() => candidate().replay_consent ?? null)
  const consentBusy = ref(false)
  const confirm = useConfirm()

  async function withdrawConsent(): Promise<void> {
    const confirmed = await confirm({
      title: t('linkgrabber.replay.consent.withdraw_title'),
      description: t('linkgrabber.replay.consent.withdraw_description'),
      confirmLabel: t('linkgrabber.replay.consent.withdraw'),
      confirmIcon: 'i-lucide-shield-off',
      destructive: true
    })
    if (!confirmed) return
    consentBusy.value = true
    await collector.revokeReplayConsent(candidate().id)
    consentBusy.value = false
  }

  /** Torrent summary carried in the list; the file tree itself is fetched on demand. */
  const torrent = computed(() => candidate().torrent ?? null)
  const torrentDetail = computed(() => torrents.detail('candidate', candidate().id))
  const torrentBusy = computed(() => torrents.isBusy('candidate', candidate().id))
  const torrentError = computed(() => torrents.errorOf('candidate', candidate().id))
  /** Remote directory summary carried in the list; the tree itself is fetched on demand. */
  const listing = computed(() => candidate().listing ?? null)
  const listingDetail = ref<ResolvedRemoteListing | null>(null)
  const listingBusy = ref(false)
  const listingError = ref<string | null>(null)
  /** Full format inventory of a media link; fetched on demand, never carried in the list. */
  const mediaFormats = ref<MediaFormatsResponse | null>(null)
  const mediaBusy = ref(false)
  const mediaError = ref<string | null>(null)
  const selectorRef = ref<InstanceType<typeof MediaFormatSelector> | null>(null)
  /** The mirrors a Metalink parser stated for this link (RD-150-03), reviewed before queueing. */
  const sources = computed(() => candidate().sources ?? [])
  const expandable = computed(() =>
    Boolean(request.value || torrent.value || listing.value || media.value || sources.value.length)
  )

  /** Loads the tree the first time the row is opened; magnets resolve their metadata first. */
  async function expand(): Promise<void> {
    expanded.value = !expanded.value
    if (!expanded.value) return
    if (listing.value && !listingDetail.value) await loadListing()
    if (media.value && !mediaFormats.value) await Promise.all([loadMediaFormats(), loadAuthProfiles()])
    if (!torrent.value || torrentDetail.value) return
    if (torrent.value.metadata_state === 'pending') {
      await torrents.resolveMetadata(candidate().id)
    } else {
      await torrents.load('candidate', candidate().id)
    }
  }

  /** The inventory is fetched per candidate so the list response stays bounded. */
  async function loadMediaFormats(): Promise<void> {
    mediaBusy.value = true
    mediaFormats.value = await collector.fetchMediaFormats(candidate().id)
    mediaBusy.value = false
    // A link probed before the selector existed has no stored inventory; the preset dropdown
    // in the row keeps working, so this is a missing extra rather than a failure.
    mediaError.value = mediaFormats.value ? null : t('linkgrabber.media.no_inventory')
  }

  /** Live preview while filters are being changed; a refusal is a result, not an error. */
  async function previewMedia(criteria: MediaFormatCriteria): Promise<void> {
    mediaBusy.value = true
    const { resolution, code } = await collector.previewMediaSelection(candidate().id, criteria)
    mediaBusy.value = false
    selectorRef.value?.setResolution(resolution, code)
  }

  /** Server-side template preview; the same evaluator the download uses. */
  function resolveOutput(template: string) {
    return collector.previewMediaOutput(candidate().id, template)
  }

  /**
   * Cookie profiles are loaded once for the row rather than per keystroke: the list is small,
   * changes rarely, and the picker only needs it while the panel is open.
   */
  const authProfiles = ref<AuthProfile[]>([])

  async function loadAuthProfiles(): Promise<void> {
    const response = await api.GET('/api/v1/auth-profiles')
    authProfiles.value = response.data ?? []
  }

  async function applyAuthProfile(mode: CandidateAuthProfileMode, profileId?: string): Promise<void> {
    mediaBusy.value = true
    await collector.setAuthProfile(candidate().id, mode, profileId)
    mediaBusy.value = false
  }

  async function applyMedia(criteria: MediaFormatCriteria): Promise<void> {
    mediaBusy.value = true
    const stored = await collector.setMediaSelection(candidate().id, criteria)
    mediaBusy.value = false
    if (stored) await loadMediaFormats()
  }

  /** The full tree is fetched per candidate so the list response stays bounded. */
  async function loadListing(): Promise<void> {
    listingBusy.value = true
    const response = await api.GET('/api/v1/collector/candidates/{id}/listing', {
      params: { path: { id: candidate().id } }
    })
    listingBusy.value = false
    if (!response.data) {
      listingError.value = responseError(response)
      return
    }
    listingError.value = null
    listingDetail.value = response.data
  }

  async function saveListingPlan(excluded: string[]): Promise<void> {
    listingBusy.value = true
    const response = await api.PUT('/api/v1/collector/candidates/{id}/listing/plan', {
      params: { path: { id: candidate().id } },
      body: { excluded }
    })
    listingBusy.value = false
    if (!response.data) {
      listingError.value = responseError(response)
      return
    }
    listingError.value = null
    listingDetail.value = response.data
  }

  function savePlan(plan: TorrentPlanRequest): void {
    void torrents.savePlan('candidate', candidate().id, plan)
  }

  return {
    request,
    media,
    expanded,
    expandable,
    expand,
    consent,
    consentBusy,
    withdrawConsent,
    torrent,
    torrentDetail,
    torrentBusy,
    torrentError,
    savePlan,
    listing,
    listingDetail,
    listingBusy,
    listingError,
    saveListingPlan,
    mediaFormats,
    mediaBusy,
    mediaError,
    selectorRef,
    previewMedia,
    resolveOutput,
    authProfiles,
    applyAuthProfile,
    applyMedia,
    sources
  }
}

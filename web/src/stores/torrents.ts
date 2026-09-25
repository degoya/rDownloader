import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, responseError } from '@/api/client'
import type {
  TorrentAggregateStats,
  TorrentDetail,
  TorrentEngineCapabilities,
  TorrentPeerPage,
  TorrentPieceAvailability,
  TorrentPlanRequest,
  SeedingPolicyRequest,
  SeedingPolicyResponse,
  TrackerListResponse
} from '@/api/types'
import { i18n } from '@/i18n'

/** Where a torrent's file tree is being reviewed. */
export type TorrentScope = 'candidate' | 'download'

const t = (key: string, named: Record<string, unknown> = {}): string => i18n.global.t(key, named)

/** Cache key, so a candidate and a download with the same id never collide. */
function cacheKey(scope: TorrentScope, id: string): string {
  return `${scope}:${id}`
}

/**
 * Torrent detail state.
 *
 * Details are fetched per torrent rather than embedded in the candidate and queue lists:
 * a torrent can hold thousands of files, and a list response must not grow with it.
 */
export const useTorrentsStore = defineStore('torrents', () => {
  const details = ref<Record<string, TorrentDetail>>({})
  const busy = ref<Record<string, boolean>>({})
  const errors = ref<Record<string, string>>({})
  const capabilities = ref<TorrentEngineCapabilities | null>(null)
  const trackers = ref<Record<string, TrackerListResponse>>({})
  const stats = ref<Record<string, TorrentAggregateStats>>({})
  const peers = ref<Record<string, TorrentPeerPage>>({})
  const pieces = ref<Record<string, TorrentPieceAvailability>>({})
  const seeding = ref<Record<string, SeedingPolicyResponse>>({})

  function detail(scope: TorrentScope, id: string): TorrentDetail | null {
    return details.value[cacheKey(scope, id)] ?? null
  }

  function isBusy(scope: TorrentScope, id: string): boolean {
    return busy.value[cacheKey(scope, id)] === true
  }

  function errorOf(scope: TorrentScope, id: string): string {
    return errors.value[cacheKey(scope, id)] ?? ''
  }

  async function load(scope: TorrentScope, id: string): Promise<void> {
    const key = cacheKey(scope, id)
    if (busy.value[key]) return
    busy.value[key] = true
    errors.value[key] = ''
    const response = scope === 'candidate'
      ? await api.GET('/api/v1/collector/candidates/{id}/torrent', { params: { path: { id } } })
      : await api.GET('/api/v1/downloads/{id}/torrent', { params: { path: { id } } })
    busy.value[key] = false
    if (response.data) {
      details.value[key] = response.data
      capabilities.value = response.data.capabilities
    } else {
      errors.value[key] = responseError(response)
    }
  }

  /** Replaces the plan and stores the resolved tree the server sends back. */
  async function savePlan(
    scope: TorrentScope,
    id: string,
    body: TorrentPlanRequest
  ): Promise<boolean> {
    const key = cacheKey(scope, id)
    busy.value[key] = true
    errors.value[key] = ''
    const response = scope === 'candidate'
      ? await api.PUT('/api/v1/collector/candidates/{id}/torrent/plan', { params: { path: { id } }, body })
      : await api.PUT('/api/v1/downloads/{id}/torrent/plan', { params: { path: { id } }, body })
    busy.value[key] = false
    if (response.data) {
      details.value[key] = response.data
      return true
    }
    errors.value[key] = responseError(response)
    return false
  }

  /** Fetches the metadata behind a magnet so it can be reviewed like a `.torrent`. */
  async function resolveMetadata(id: string): Promise<void> {
    const key = cacheKey('candidate', id)
    busy.value[key] = true
    errors.value[key] = ''
    const response = await api.POST('/api/v1/collector/candidates/{id}/torrent/resolve', {
      params: { path: { id } }
    })
    busy.value[key] = false
    if (response.data) {
      details.value[key] = response.data
      if (response.data.metadata_state === 'failed') {
        errors.value[key] = response.data.metadata_error ?? t('torrent.errors.metadata_failed')
      }
    } else {
      errors.value[key] = responseError(response)
    }
  }

  function trackersOf(id: string): TrackerListResponse | null {
    return trackers.value[id] ?? null
  }

  async function loadTrackers(id: string): Promise<void> {
    const key = cacheKey('download', id)
    errors.value[key] = ''
    const response = await api.GET('/api/v1/downloads/{id}/torrent/trackers', {
      params: { path: { id } }
    })
    if (response.data) trackers.value[id] = response.data
    else errors.value[key] = responseError(response)
  }

  /**
   * Replaces the tracker list.
   *
   * Existing entries are referenced by id because their URL reaches the browser redacted;
   * sending the displayed URL back would store the masked form as the announce address.
   */
  async function saveTrackers(
    id: string,
    entries: { id?: string, url?: string, tier: number }[]
  ): Promise<void> {
    const key = cacheKey('download', id)
    busy.value[key] = true
    errors.value[key] = ''
    const response = await api.PUT('/api/v1/downloads/{id}/torrent/trackers', {
      params: { path: { id } },
      body: { trackers: entries }
    })
    busy.value[key] = false
    if (response.data) trackers.value[id] = response.data
    else errors.value[key] = responseError(response)
  }

  async function scrapeTrackers(id: string): Promise<void> {
    const key = cacheKey('download', id)
    busy.value[key] = true
    errors.value[key] = ''
    const response = await api.POST('/api/v1/downloads/{id}/torrent/trackers/scrape', {
      params: { path: { id } }
    })
    busy.value[key] = false
    if (response.data) trackers.value[id] = response.data
    else errors.value[key] = responseError(response)
  }

  /** Forces a fresh announce; rate limited by the server. */
  async function reannounce(id: string): Promise<boolean> {
    const key = cacheKey('download', id)
    errors.value[key] = ''
    const response = await api.POST('/api/v1/downloads/{id}/torrent/trackers/reannounce', {
      params: { path: { id } }
    })
    if (response.data) return true
    errors.value[key] = responseError(response)
    return false
  }

  function statsOf(id: string): TorrentAggregateStats | null {
    return stats.value[id] ?? null
  }

  function peersOf(id: string): TorrentPeerPage | null {
    return peers.value[id] ?? null
  }

  function piecesOf(id: string): TorrentPieceAvailability | null {
    return pieces.value[id] ?? null
  }

  /**
   * Applies an aggregate sample from the SSE stream.
   *
   * Aggregates arrive pushed because they are a handful of numbers; peers and pieces are
   * pulled instead, and only while a detail panel is open.
   */
  function applyStats(id: string, sample: TorrentAggregateStats): void {
    stats.value[id] = sample
  }

  async function loadPeers(id: string, cursor?: string): Promise<void> {
    const key = cacheKey('download', id)
    const response = await api.GET('/api/v1/downloads/{id}/torrent/peers', {
      params: { path: { id }, query: cursor ? { cursor } : {} }
    })
    if (response.data) peers.value[id] = response.data
    else errors.value[key] = responseError(response)
  }

  async function loadPieces(id: string): Promise<void> {
    const key = cacheKey('download', id)
    const response = await api.GET('/api/v1/downloads/{id}/torrent/pieces', {
      params: { path: { id } }
    })
    if (response.data) pieces.value[id] = response.data
    else errors.value[key] = responseError(response)
  }

  function seedingOf(id: string): SeedingPolicyResponse | null {
    return seeding.value[id] ?? null
  }

  async function loadSeeding(id: string): Promise<void> {
    const response = await api.GET('/api/v1/downloads/{id}/torrent/seeding', {
      params: { path: { id } }
    })
    if (response.data) seeding.value[id] = response.data
  }

  /** Stores a per-torrent override; the server applies a lowered limit at once. */
  async function saveSeeding(id: string, body: SeedingPolicyRequest): Promise<void> {
    const key = cacheKey('download', id)
    busy.value[key] = true
    errors.value[key] = ''
    const response = await api.PUT('/api/v1/downloads/{id}/torrent/seeding', {
      params: { path: { id } },
      body
    })
    busy.value[key] = false
    if (response.data) seeding.value[id] = response.data
    else errors.value[key] = responseError(response)
  }

  /** Clears the override so the torrent inherits again. */
  async function clearSeeding(id: string): Promise<void> {
    const response = await api.DELETE('/api/v1/downloads/{id}/torrent/seeding', {
      params: { path: { id } }
    })
    if (response.data) seeding.value[id] = response.data
  }

  /** Drops a cached detail, e.g. after the candidate was removed. */
  function forget(scope: TorrentScope, id: string): void {
    const key = cacheKey(scope, id)
    delete details.value[key]
    delete busy.value[key]
    delete errors.value[key]
  }

  return {
    details,
    capabilities,
    trackers,
    stats,
    peers,
    pieces,
    seeding,
    detail,
    isBusy,
    errorOf,
    load,
    savePlan,
    resolveMetadata,
    trackersOf,
    loadTrackers,
    saveTrackers,
    scrapeTrackers,
    reannounce,
    statsOf,
    peersOf,
    piecesOf,
    applyStats,
    loadPeers,
    loadPieces,
    seedingOf,
    loadSeeding,
    saveSeeding,
    clearSeeding,
    forget
  }
})

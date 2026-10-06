import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import { useLatestFetch } from '@/composables/useLatestFetch'
import type { components } from '@/api/schema'
import type { HistoryEntry, HistoryOutcome } from '@/api/types'

type DownloadKind = components['schemas']['DownloadKind']

/** Entries one read asks for; the server caps a page at 1000. */
const HISTORY_PAGE_SIZE = 50

/** The kinds the filter offers, in the order people meet them. */
export const HISTORY_KINDS: readonly DownloadKind[] = [
  'http', 'usenet', 'torrent', 'media', 'gallery', 'record', 'ftp', 'sftp', 'plugin', 'object_storage'
]

/** The time ranges the filter offers, counted back from now. */
export const HISTORY_PERIODS = ['day', 'week', 'month', 'year'] as const
type HistoryPeriod = typeof HISTORY_PERIODS[number]
const PERIOD_DAYS: Record<HistoryPeriod, number> = { day: 1, week: 7, month: 30, year: 365 }

/**
 * `'all'` is "any" for the three choices: a sentinel rather than an empty string, because the
 * select offering them refuses an item whose value is `''`.
 */
interface HistoryFilters {
  search: string
  outcome: HistoryOutcome | 'all'
  kind: DownloadKind | 'all'
  period: HistoryPeriod | 'all'
}

/** The query string of `GET /api/v1/history`, with only the parameters that are set. */
interface HistoryQuery {
  limit: number
  offset: number
  q?: string
  outcome?: HistoryOutcome
  kind?: DownloadKind
  from?: string
}

function emptyFilters(): HistoryFilters {
  return { search: '', outcome: 'all', kind: 'all', period: 'all' }
}

/** The length of the whole list the server names beside a page. */
function totalOf(response: Response | undefined): number {
  const value = Number(response?.headers.get('x-total-count') ?? 0)
  return Number.isFinite(value) ? value : 0
}

/**
 * The download history (RD-1100-04): every package that completed or failed, kept after it
 * left the queue.
 *
 * Filtered and paged by the server — the history grows with every finished package, so the
 * browser never holds more than the pages somebody scrolled to. `stored` is the unfiltered
 * count, which the clear names in its question.
 */
export const useHistoryStore = defineStore('history', () => {
  const filters = reactive<HistoryFilters>(emptyFilters())
  const entries = ref<HistoryEntry[]>([])
  const total = ref(0)
  const stored = ref<number | null>(null)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  const { fetching, settled, loading, run } = useLatestFetch()

  function query(offset: number): HistoryQuery {
    const params: HistoryQuery = { limit: HISTORY_PAGE_SIZE, offset }
    const search = filters.search.trim()
    if (search) params.q = search
    if (filters.outcome !== 'all') params.outcome = filters.outcome
    if (filters.kind !== 'all') params.kind = filters.kind
    if (filters.period !== 'all') {
      params.from = new Date(Date.now() - PERIOD_DAYS[filters.period] * 86_400_000).toISOString()
    }
    return params
  }

  function read(offset: number) {
    return api.GET('/api/v1/history', { params: { query: query(offset) } })
  }

  function apply(response: Awaited<ReturnType<typeof read>>, append: boolean): void {
    if (!response.data) {
      error.value = responseError(response)
      return
    }
    entries.value = append ? [...entries.value, ...response.data] : response.data
    total.value = totalOf(response.response)
    error.value = null
  }

  async function countStored(): Promise<void> {
    const response = await api.GET('/api/v1/history', { params: { query: { limit: 1 } } })
    if (response.data) stored.value = totalOf(response.response)
  }

  /** The first page under the current filter; a page still on its way is dropped. */
  async function refresh(): Promise<void> {
    await Promise.all([
      run(() => read(0), response => apply(response, false)),
      countStored()
    ])
  }

  /** Appends the next page behind the last entry shown. */
  async function loadMore(): Promise<void> {
    if (fetching.value || entries.value.length >= total.value) return
    await run(() => read(entries.value.length), response => apply(response, true))
  }

  /**
   * Puts the entry's sources back into the LinkGrabber. Answers whether it went; a refusal is
   * in `error`.
   */
  async function readd(entry: HistoryEntry): Promise<boolean> {
    const response = await api.POST('/api/v1/history/{id}/readd', { params: { path: { id: entry.id } } })
    if (response.data) {
      error.value = null
      return true
    }
    error.value = responseError(response)
    return false
  }

  function clearFilters(): void {
    Object.assign(filters, emptyFilters())
  }

  return {
    filters,
    entries,
    total,
    stored,
    error,
    fetching,
    settled,
    loading,
    refresh,
    loadMore,
    readd,
    clearFilters
  }
})

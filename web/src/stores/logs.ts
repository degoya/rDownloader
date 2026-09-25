import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { BundleCreated, BundlePreview, LogLevel, LogRecord, LogRecordsPage } from '@/api/types'

/** Records one read asks for; the server caps a page at 500. */
export const LOG_PAGE_SIZE = 200

export interface LogFilters {
  /** This level and the more severe ones; empty means every level. */
  level: LogLevel | ''
  component: string
  code: string
  correlationId: string
  search: string
}

/** The query string of `GET /api/v1/diagnostics/logs`, with only the parameters that are set. */
interface LogQuery {
  limit: number
  level?: LogLevel
  component?: string
  code?: string
  correlation_id?: string
  search?: string
  before_id?: number
}

function emptyFilters(): LogFilters {
  return { level: '', component: '', code: '', correlationId: '', search: '' }
}

/**
 * The structured log store and the diagnostic bundle (RD-110-02).
 *
 * The bundle half keeps the rule the server enforces visible in the client too: `create()`
 * sends nothing until a preview was loaded, and it sends that preview's digest back, so the
 * archive is always the inventory the person looked at. A `409 diagnostics.preview_stale`
 * reloads the preview rather than retrying blindly — the person has to look again.
 */
export const useLogsStore = defineStore('logs', () => {
  const records = ref<LogRecord[]>([])
  const filters = reactive<LogFilters>(emptyFilters())
  const fullPage = ref(false)
  const total = ref(0)
  const dropped = ref(0)
  const retention = ref<LogRecordsPage['retention'] | null>(null)
  const error = ref<string | null>(null)
  const fetching = ref(false)
  const settled = ref(false)
  /** The first fetch alone shows the loading surface (`design.md`). */
  const loading = computed(() => !settled.value)

  const preview = ref<BundlePreview | null>(null)
  const selected = ref<string[]>([])
  const created = ref<BundleCreated | null>(null)
  const bundleError = ref<string | null>(null)
  const bundleBusy = ref(false)

  function query(beforeId?: number): LogQuery {
    const params: LogQuery = { limit: LOG_PAGE_SIZE }
    if (filters.level) params.level = filters.level
    if (filters.component.trim()) params.component = filters.component.trim()
    if (filters.code.trim()) params.code = filters.code.trim()
    if (filters.correlationId.trim()) params.correlation_id = filters.correlationId.trim()
    if (filters.search.trim()) params.search = filters.search.trim()
    if (beforeId !== undefined) params.before_id = beforeId
    return params
  }

  function take(page: LogRecordsPage): void {
    fullPage.value = page.full_page
    total.value = page.total
    dropped.value = page.dropped
    retention.value = page.retention
    error.value = null
  }

  async function refresh(): Promise<void> {
    fetching.value = true
    const response = await api.GET('/api/v1/diagnostics/logs', { params: { query: query() } })
    if (response.data) {
      records.value = response.data.records
      take(response.data)
    } else {
      error.value = responseError(response)
    }
    fetching.value = false
    settled.value = true
  }

  /** Appends the page behind the oldest record shown. */
  async function loadOlder(): Promise<void> {
    const oldest = records.value.at(-1)
    if (!oldest || fetching.value) return
    fetching.value = true
    const response = await api.GET('/api/v1/diagnostics/logs', { params: { query: query(oldest.id) } })
    if (response.data) {
      records.value = [...records.value, ...response.data.records]
      take(response.data)
    } else {
      error.value = responseError(response)
    }
    fetching.value = false
  }

  function clearFilters(): void {
    Object.assign(filters, emptyFilters())
  }

  async function loadPreview(): Promise<void> {
    bundleBusy.value = true
    created.value = null
    const response = await api.GET('/api/v1/diagnostics/bundle/preview')
    if (response.data) {
      preview.value = response.data
      selected.value = response.data.entries.map(entry => entry.id)
      bundleError.value = null
    } else {
      preview.value = null
      selected.value = []
      bundleError.value = responseError(response)
    }
    bundleBusy.value = false
  }

  /** Whether `create()` would send anything: a preview was seen and something is ticked. */
  const canCreate = computed(() => preview.value !== null && selected.value.length > 0 && !bundleBusy.value)

  async function create(): Promise<void> {
    const seen = preview.value
    if (!seen || selected.value.length === 0) return
    bundleBusy.value = true
    const response = await api.POST('/api/v1/diagnostics/bundle', {
      body: { approved: true, digest: seen.digest, entries: [...selected.value] }
    })
    if (response.data) {
      created.value = response.data
      bundleError.value = null
      bundleBusy.value = false
      return
    }
    const message = responseError(response)
    bundleBusy.value = false
    if (response.response?.status === 409) {
      // The inventory moved under the approval: show the new one, and ask again — with the
      // refusal still on screen, so the person knows why they are looking twice.
      await loadPreview()
    }
    bundleError.value = message
  }

  return {
    records,
    filters,
    fullPage,
    total,
    dropped,
    retention,
    error,
    fetching,
    settled,
    loading,
    preview,
    selected,
    created,
    bundleError,
    bundleBusy,
    canCreate,
    refresh,
    loadOlder,
    clearFilters,
    loadPreview,
    create
  }
})

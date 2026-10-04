import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { usePagedRecords } from '@/composables/usePagedRecords'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import type { BundleCreated, BundlePreview, LogLevel, LogRecord, LogRecordsPage } from '@/api/types'

/** Records one read asks for; the server caps a page at 500. */
const LOG_PAGE_SIZE = 200

interface LogFilters {
  /**
   * This level and the more severe ones; `'all'` means every level. A sentinel rather than an
   * empty string: the select offering it refuses an item whose value is `''`.
   */
  level: LogLevel | 'all'
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
  return { level: 'all', component: '', code: '', correlationId: '', search: '' }
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
  const filters = reactive<LogFilters>(emptyFilters())
  const fullPage = ref(false)
  const total = ref(0)
  const dropped = ref(0)
  const retention = ref<LogRecordsPage['retention'] | null>(null)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)

  const preview = ref<BundlePreview | null>(null)
  const selected = ref<string[]>([])
  const created = ref<BundleCreated | null>(null)
  const bundleError = ref<string | null>(null)
  const bundleBusy = ref(false)

  function query(beforeId?: number): LogQuery {
    const params: LogQuery = { limit: LOG_PAGE_SIZE }
    if (filters.level !== 'all') params.level = filters.level
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
  }

  /**
   * The first page under the current filter and the older ones behind it; a refresh drops an
   * older page still on its way, which belonged to the filter before (WEB-07). The first fetch
   * alone shows the loading surface (`design.md`).
   */
  const { records, fetching, settled, loading, refresh, loadOlder } = usePagedRecords<LogRecord, LogRecordsPage>(
    beforeId => api.GET('/api/v1/diagnostics/logs', { params: { query: query(beforeId) } }),
    take,
    error
  )

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

import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'

import { api } from '@/api/client'
import { usePagedRecords } from '@/composables/usePagedRecords'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import { BASE_PATH } from '@/basePath'
import type { AuditAction, AuditActorKind, AuditChannel, AuditOutcome, AuditRecord, AuditRecordsPage } from '@/api/types'

/** Records one read asks for; the server caps a page at 500. */
const AUDIT_PAGE_SIZE = 200

/**
 * `'all'` is "any" for the three choices: a sentinel rather than an empty string, because the
 * select offering them refuses an item whose value is `''`.
 */
interface AuditFilters {
  action: AuditAction | 'all'
  outcome: AuditOutcome | 'all'
  actorKind: AuditActorKind | 'all'
  /** How the action came: REST, MCP, the capture door, a compatibility client (RD-1200-04). */
  via: AuditChannel | 'all'
  actorId: string
  targetKind: string
  targetId: string
  traceId: string
}

/** The query string of `GET /api/v1/audit/records`, with only the parameters that are set. */
interface AuditQuery {
  limit: number
  action?: AuditAction
  outcome?: AuditOutcome
  actor_kind?: AuditActorKind
  via?: AuditChannel
  actor_id?: string
  target_kind?: string
  target_id?: string
  trace_id?: string
  before_id?: number
}

function emptyFilters(): AuditFilters {
  return { action: 'all', outcome: 'all', actorKind: 'all', via: 'all', actorId: '', targetKind: '', targetId: '', traceId: '' }
}

/**
 * The append-only audit log (RD-110-03).
 *
 * A read-only store on purpose: the service offers no route that writes, edits or deletes a
 * record, so there is nothing here that could. The export is a plain link rather than a fetch
 * — the browser saves the file the server names, and a blob built here would lose the name.
 */
export const useAuditStore = defineStore('audit', () => {
  const filters = reactive<AuditFilters>(emptyFilters())
  const fullPage = ref(false)
  const total = ref(0)
  const retention = ref<AuditRecordsPage['retention'] | null>(null)
  const actions = ref<AuditAction[]>([])
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)

  function query(beforeId?: number): AuditQuery {
    const params: AuditQuery = { limit: AUDIT_PAGE_SIZE }
    if (filters.action !== 'all') params.action = filters.action
    if (filters.outcome !== 'all') params.outcome = filters.outcome
    if (filters.actorKind !== 'all') params.actor_kind = filters.actorKind
    if (filters.via !== 'all') params.via = filters.via
    if (filters.actorId.trim()) params.actor_id = filters.actorId.trim()
    if (filters.targetKind.trim()) params.target_kind = filters.targetKind.trim()
    if (filters.targetId.trim()) params.target_id = filters.targetId.trim()
    if (filters.traceId.trim()) params.trace_id = filters.traceId.trim()
    if (beforeId !== undefined) params.before_id = beforeId
    return params
  }

  function take(page: AuditRecordsPage): void {
    fullPage.value = page.full_page
    total.value = page.total
    retention.value = page.retention
    actions.value = page.actions
  }

  /**
   * The first page under the current filter and the older ones behind it; a refresh drops an
   * older page still on its way, which belonged to the filter before (WEB-07). The first fetch
   * alone shows the loading surface (`design.md`).
   */
  const { records, fetching, settled, loading, refresh, loadOlder } = usePagedRecords<AuditRecord, AuditRecordsPage>(
    beforeId => api.GET('/api/v1/audit/records', { params: { query: query(beforeId) } }),
    take,
    error
  )

  function clearFilters(): void {
    Object.assign(filters, emptyFilters())
  }

  /** The export link for the filter currently shown, so the file matches the list. */
  const exportHref = computed(() => {
    const params = new URLSearchParams()
    for (const [key, value] of Object.entries(query())) {
      if (key === 'limit' || value === undefined) continue
      params.set(key, String(value))
    }
    const search = params.toString()
    return `${BASE_PATH}/api/v1/audit/export${search ? `?${search}` : ''}`
  })

  return {
    records,
    filters,
    fullPage,
    total,
    retention,
    actions,
    error,
    fetching,
    settled,
    loading,
    exportHref,
    refresh,
    loadOlder,
    clearFilters
  }
})

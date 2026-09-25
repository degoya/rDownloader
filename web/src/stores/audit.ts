import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'

import { api, responseError } from '@/api/client'
import { BASE_PATH } from '@/basePath'
import type { AuditAction, AuditActorKind, AuditOutcome, AuditRecord, AuditRecordsPage } from '@/api/types'

/** Records one read asks for; the server caps a page at 500. */
export const AUDIT_PAGE_SIZE = 200

export interface AuditFilters {
  action: AuditAction | ''
  outcome: AuditOutcome | ''
  actorKind: AuditActorKind | ''
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
  actor_id?: string
  target_kind?: string
  target_id?: string
  trace_id?: string
  before_id?: number
}

function emptyFilters(): AuditFilters {
  return { action: '', outcome: '', actorKind: '', actorId: '', targetKind: '', targetId: '', traceId: '' }
}

/**
 * The append-only audit log (RD-110-03).
 *
 * A read-only store on purpose: the service offers no route that writes, edits or deletes a
 * record, so there is nothing here that could. The export is a plain link rather than a fetch
 * — the browser saves the file the server names, and a blob built here would lose the name.
 */
export const useAuditStore = defineStore('audit', () => {
  const records = ref<AuditRecord[]>([])
  const filters = reactive<AuditFilters>(emptyFilters())
  const fullPage = ref(false)
  const total = ref(0)
  const retention = ref<AuditRecordsPage['retention'] | null>(null)
  const actions = ref<AuditAction[]>([])
  const error = ref<string | null>(null)
  const fetching = ref(false)
  const settled = ref(false)
  /** The first fetch alone shows the loading surface (`design.md`). */
  const loading = computed(() => !settled.value)

  function query(beforeId?: number): AuditQuery {
    const params: AuditQuery = { limit: AUDIT_PAGE_SIZE }
    if (filters.action) params.action = filters.action
    if (filters.outcome) params.outcome = filters.outcome
    if (filters.actorKind) params.actor_kind = filters.actorKind
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
    error.value = null
  }

  async function refresh(): Promise<void> {
    fetching.value = true
    const response = await api.GET('/api/v1/audit/records', { params: { query: query() } })
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
    const response = await api.GET('/api/v1/audit/records', { params: { query: query(oldest.id) } })
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

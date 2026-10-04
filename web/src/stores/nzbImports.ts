import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, responseError } from '@/api/client'
import { clearWhenReconnected } from '@/composables/serviceConnection'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import type { DownloadPriority, NzbImport, NzbImportUpdateRequest } from '@/api/types'
import type { FileImportEntry } from '@/composables/useNzbImportModal'
import { i18n } from '@/i18n'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'
import { MIB } from '@/utils/format'

/** Extra multipart fields accepted alongside `file` at import time. */
interface NzbImportOptions {
  /** Overrides the uploaded file name; empty keeps it. */
  name?: string
  categoryId?: string | null
  priority?: DownloadPriority | null
}

interface NzbImportChange {
  categoryId?: string | null
  priority?: DownloadPriority
}

/**
 * Result of an import attempt. The server de-duplicates by SHA-256 and answers a repeated
 * upload with the *existing* row flagged `duplicate`, so callers must be able to tell a fresh
 * import from a duplicate — otherwise an already enqueued NZB looks like it vanished.
 */
export type NzbImportResult =
  | { status: 'created', item: NzbImport }
  | { status: 'duplicate', item: NzbImport }
  | { status: 'error', message: string }

/**
 * What handing one import to a provider produced (RD-191-13). `alreadyRunning` is the server's
 * duplicate guard: the account already had a job for this NZB, and nothing was sent.
 */
export type NzbHandOverResult =
  | { ok: true, item: NzbImport, alreadyRunning: boolean }
  | { ok: false, message: string }

/** Aggregated outcome of a multi-file import batch; one entry lands in exactly one bucket. */
export interface NzbBatchResult {
  created: NzbImport[]
  duplicates: NzbImport[]
  errors: { file: string, message: string }[]
}

export const useNzbImportsStore = defineStore('nzbImports', () => {
  const imports = ref<NzbImport[]>([])
  const pending = ref(false)
  const error = ref<string | null>(null)
  // A "service could not be reached" alert ends with the outage.
  clearWhenReconnected(error)
  const deletingIds = ref<Set<string>>(new Set())
  const enqueuingIds = ref<Set<string>>(new Set())
  const handingOverIds = ref<Set<string>>(new Set())

  async function refresh(): Promise<void> {
    const response = await api.GET('/api/v1/nzb/imports')
    if (response.data) {
      imports.value = response.data
      error.value = null
    } else {
      error.value = responseError(response)
    }
  }

  /** Posts a single file; does not touch `pending` so batch callers can hold it across a fan-out. */
  async function importOne(file: File, options: NzbImportOptions = {}): Promise<NzbImportResult> {
    if (file.size > 64 * MIB) {
      error.value = i18n.global.t('linkgrabber.nzb.size_limit')
      return { status: 'error', message: error.value }
    }
    const form = new FormData()
    form.append('file', file, file.name)
    if (options.name) form.append('name', options.name)
    // Empty strings are the documented "no category" / "server default priority" values.
    form.append('category_id', options.categoryId ?? '')
    form.append('priority', options.priority ?? '')
    try {
      // Through the client, multipart included, so a lost session or connection is noticed.
      const response = await api.POST('/api/v1/nzb/imports', { body: form as never })
      const payload: unknown = response.data
      if (!isNzbImport(payload)) {
        error.value = payloadMessage(response.error)
        return { status: 'error', message: error.value }
      }
      imports.value = [payload, ...imports.value.filter((item) => item.id !== payload.id)]
      error.value = null
      return { status: payload.duplicate ? 'duplicate' : 'created', item: payload }
    } catch (requestError: unknown) {
      error.value = requestError instanceof Error ? requestError.message : i18n.global.t('linkgrabber.nzb.import_failed')
      return { status: 'error', message: error.value }
    }
  }

  async function importNzb(file: File, options: NzbImportOptions = {}): Promise<NzbImportResult> {
    pending.value = true
    try {
      return await importOne(file, options)
    } finally {
      pending.value = false
    }
  }

  /**
   * Sequential fan-out over the single-file endpoint (the server accepts one file per request).
   * `pending` stays true for the whole batch instead of toggling per file. A file over the size
   * guard becomes an error entry rather than aborting the remaining files.
   */
  async function importMany(entries: FileImportEntry[], options: { categoryId: string | null, priority: DownloadPriority }): Promise<NzbBatchResult> {
    const result: NzbBatchResult = { created: [], duplicates: [], errors: [] }
    if (!entries.length) return result
    pending.value = true
    try {
      for (const entry of entries) {
        const outcome = await importOne(entry.file, { name: entry.name, categoryId: options.categoryId, priority: options.priority })
        if (outcome.status === 'created') result.created.push(outcome.item)
        else if (outcome.status === 'duplicate') result.duplicates.push(outcome.item)
        else result.errors.push({ file: entry.file.name, message: outcome.message })
      }
    } finally {
      pending.value = false
    }
    return result
  }

  /**
   * Moves the import into the download queue as a package; the LinkGrabber list drops it.
   *
   * `paused` creates the package with every download paused, the same promise "add paused"
   * makes for collector links (RD-107-09).
   */
  async function enqueue(id: string, paused = false): Promise<boolean> {
    const enqueued = await enqueueMany([id], paused)
    if (enqueued) await refresh()
    return enqueued > 0
  }

  /** Fans out over the single-import endpoint; the caller refreshes once afterwards. */
  async function enqueueMany(ids: string[], paused = false): Promise<number> {
    if (!ids.length) return 0
    enqueuingIds.value = new Set([...enqueuingIds.value, ...ids])
    const responses = await Promise.all(ids.map(id => api.POST('/api/v1/nzb/imports/{id}/enqueue', { params: { path: { id } }, body: { paused } })))
    const next = new Set(enqueuingIds.value)
    for (const id of ids) next.delete(id)
    enqueuingIds.value = next
    const failure = responses.find(response => !response.data)
    error.value = failure ? responseError(failure) : null
    return responses.filter(response => response.data).length
  }

  async function update(id: string, change: NzbImportChange): Promise<boolean> {
    const body: NzbImportUpdateRequest = {
      ...(change.categoryId ? { category_id: change.categoryId } : {}),
      ...(change.categoryId === null ? { clear_category: true } : {}),
      ...(change.priority ? { priority: change.priority } : {})
    }
    const response = await api.PATCH('/api/v1/nzb/imports/{id}', {
      params: { path: { id } },
      body
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    imports.value = imports.value.map(item => item.id === id ? response.data! : item)
    error.value = null
    return true
  }

  /**
   * Hands one import to an account's provider instead of the queue (RD-191-13). The import
   * stays listed, carrying `handed_over`; the caller reports the outcome.
   */
  async function handOver(id: string, accountId: string): Promise<NzbHandOverResult> {
    handingOverIds.value = new Set([...handingOverIds.value, id])
    try {
      const response = await api.POST('/api/v1/nzb/imports/{id}/remote-job', {
        params: { path: { id } },
        body: { account_id: accountId }
      })
      if (!response.data) return { ok: false, message: responseError(response) }
      const updated = response.data.import
      imports.value = imports.value.map(item => item.id === updated.id ? updated : item)
      return { ok: true, item: updated, alreadyRunning: response.data.already_running }
    } finally {
      const next = new Set(handingOverIds.value)
      next.delete(id)
      handingOverIds.value = next
    }
  }

  /**
   * A remote job forgotten or discarded takes the hand-over badge with it on the server
   * (`ON DELETE SET NULL`, RD-191-13), and so does a removed account with its jobs; no
   * `collector.changed` says so, only `remote_job.changed` or `account.changed`. Re-read only
   * while a badge is on screen: the remote-job event also reports every state change of every
   * job, and without a badge there is nothing it could change here.
   */
  const { connect: connectEvents, disconnect: disconnectEvents } = debouncedEventRefresh(
    ['remote_job.changed', 'account.changed'],
    () => imports.value.some(item => item.handed_over) ? refresh() : undefined
  )

  /**
   * Hands the NZB behind a queued package to an account's provider, from the Downloads view
   * (RD-191-13). The package stays; the import behind it carries `handed_over`, which is what
   * the package's badge reads.
   */
  async function handOverPackage(packageId: string, accountId: string): Promise<NzbHandOverResult> {
    const response = await api.POST('/api/v1/packages/{id}/remote-job', {
      params: { path: { id: packageId } },
      body: { account_id: accountId }
    })
    if (!response.data) return { ok: false, message: responseError(response) }
    const updated = response.data.import
    imports.value = imports.value.some(item => item.id === updated.id)
      ? imports.value.map(item => item.id === updated.id ? updated : item)
      : [...imports.value, updated]
    return { ok: true, item: updated, alreadyRunning: response.data.already_running }
  }

  async function remove(id: string): Promise<boolean> {
    if (deletingIds.value.has(id)) return false
    deletingIds.value = new Set([...deletingIds.value, id])
    const response = await api.DELETE('/api/v1/nzb/imports/{id}', { params: { path: { id } } })
    const next = new Set(deletingIds.value)
    next.delete(id)
    deletingIds.value = next
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    imports.value = imports.value.filter(item => item.id !== id)
    error.value = null
    return true
  }

  return { imports, pending, error, deletingIds, enqueuingIds, handingOverIds, refresh, importNzb, importMany, enqueue, enqueueMany, handOver, handOverPackage, update, remove, connectEvents, disconnectEvents }
})

function isNzbImport(value: unknown): value is NzbImport {
  return typeof value === 'object' && value !== null
    && typeof (value as Record<string, unknown>).id === 'string'
    && typeof (value as Record<string, unknown>).sha256 === 'string'
}

function payloadMessage(value: unknown): string {
  const message = serverMessageFrom(value)
  return message ? translateServerMessage(message) : i18n.global.t('linkgrabber.nzb.import_failed')
}

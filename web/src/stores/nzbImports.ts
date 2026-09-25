import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { DownloadPriority, NzbImport, NzbImportUpdateRequest } from '@/api/types'
import type { FileImportEntry } from '@/composables/useNzbImportModal'
import { i18n } from '@/i18n'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'
import { withBase } from '@/basePath'
import { MIB } from '@/utils/format'

/** Extra multipart fields accepted alongside `file` at import time. */
export interface NzbImportOptions {
  /** Overrides the uploaded file name; empty keeps it. */
  name?: string
  categoryId?: string | null
  priority?: DownloadPriority | null
}

export interface NzbImportChange {
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
  const deletingIds = ref<Set<string>>(new Set())
  const enqueuingIds = ref<Set<string>>(new Set())

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
      const response = await fetch(withBase('/api/v1/nzb/imports'), { method: 'POST', body: form, credentials: 'include' })
      const payload: unknown = await response.json()
      if (!response.ok || !isNzbImport(payload)) {
        error.value = payloadMessage(payload)
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

  return { imports, pending, error, deletingIds, enqueuingIds, refresh, importNzb, importMany, enqueue, enqueueMany, update, remove }
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

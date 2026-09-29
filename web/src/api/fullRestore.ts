/**
 * The calls of a full restore (RD-160-03): the chunked upload, the preview, the test restore,
 * the restore and where it stands.
 *
 * The passphrase goes into a request body and nowhere else: not into a store, not into the
 * URL, not into anything that outlives the dialog that asked for it.
 */
import { api, responseError } from '@/api/client'
import type {
  RestorePreview,
  RestoreReport,
  RestoreSource,
  RestoreStaged,
  RestoreStatus
} from '@/api/types'

/** A mapping of one of the backup's storage roots onto a folder here. */
export interface RestoreMapping {
  storage_root_id: string
  path: string
}

/** What a call answered: the payload, or the translated reason it did not. */
export type RestoreAnswer<T> = { ok: true, data: T } | { ok: false, error: string }

function answer<T>(response: { data?: T, error?: unknown }): RestoreAnswer<T> {
  return response.data !== undefined
    ? { ok: true, data: response.data }
    : { ok: false, error: responseError(response) }
}

/**
 * Sends `file` in the chunks the service accepts, each at the offset the upload has reached;
 * `progress` hears the share sent so far. Answers with the upload's id.
 */
export async function uploadArchive(
  file: Blob,
  progress: (share: number) => void
): Promise<RestoreAnswer<string>> {
  const created = await api.POST('/api/v1/backups/restore/uploads')
  if (!created.data) return { ok: false, error: responseError(created) }
  const { id, chunk_limit: limit } = created.data
  let offset = 0
  progress(0)
  while (offset < file.size) {
    const chunk = file.slice(offset, offset + limit)
    const response = await api.PUT('/api/v1/backups/restore/uploads/{id}', {
      params: { path: { id }, query: { offset } },
      body: chunk as never,
      bodySerializer: () => chunk,
      headers: { 'Content-Type': 'application/octet-stream' }
    })
    if (!response.data) {
      await api.DELETE('/api/v1/backups/restore/uploads/{id}', { params: { path: { id } } })
      return { ok: false, error: responseError(response) }
    }
    offset = response.data.size
    progress(file.size === 0 ? 1 : offset / file.size)
  }
  return { ok: true, data: id }
}

export async function previewRestore(
  source: RestoreSource,
  passphrase: string
): Promise<RestoreAnswer<RestorePreview>> {
  return answer(await api.POST('/api/v1/backups/restore/preview', { body: { source, passphrase } }))
}

export async function testRestore(
  source: RestoreSource,
  passphrase: string,
  mappings: RestoreMapping[]
): Promise<RestoreAnswer<RestoreReport>> {
  return answer(await api.POST('/api/v1/backups/restore/test', { body: { source, passphrase, mappings } }))
}

export async function startRestore(
  source: RestoreSource,
  passphrase: string,
  mappings: RestoreMapping[]
): Promise<RestoreAnswer<RestoreStaged>> {
  return answer(await api.POST('/api/v1/backups/restore', { body: { source, passphrase, mappings } }))
}

export async function restoreStatus(): Promise<RestoreAnswer<RestoreStatus>> {
  return answer(await api.GET('/api/v1/backups/restore'))
}

export async function discardRestore(): Promise<RestoreAnswer<RestoreStatus>> {
  return answer(await api.DELETE('/api/v1/backups/restore'))
}

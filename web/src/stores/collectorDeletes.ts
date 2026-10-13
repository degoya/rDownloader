import type { Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { LinkCandidate } from '@/api/types'

/** Link deletions in flight at once; the server writes them one after another anyway. */
const DELETE_CONCURRENCY = 4

/** What the delete actions write back into the collector store. */
interface DeleteActionContext {
  candidates: Ref<LinkCandidate[]>
  deletingIds: Ref<Set<string>>
  pending: Ref<boolean>
  error: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The collector store's deletions (RD-1280-08): one link, a chosen set of links, a package and
 * the whole list. They share the store's `error` and `deletingIds` and read the list again once
 * answered, so the store stays one surface.
 */
export function useDeleteActions({ candidates, deletingIds, pending, error, refresh }: DeleteActionContext) {
  async function deleteCandidate(id: string): Promise<boolean> {
    if (deletingIds.value.has(id)) return false
    deletingIds.value = new Set([...deletingIds.value, id])
    const response = await api.DELETE('/api/v1/collector/candidates/{id}', { params: { path: { id } } })
    const next = new Set(deletingIds.value)
    next.delete(id)
    deletingIds.value = next
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  /**
   * Deletes these links, a few requests at a time, and reads the list once at the end.
   *
   * There is no route for a chosen set of links, and one request after another — each followed
   * by a full refresh — took 70 s for 392 links with nothing on screen moving. `onProgress` is
   * told how many have been answered.
   */
  async function deleteCandidates(ids: readonly string[], onProgress?: (done: number) => void): Promise<number> {
    const queue = ids.filter(id => !deletingIds.value.has(id))
    if (!queue.length) return 0
    deletingIds.value = new Set([...deletingIds.value, ...queue])
    const removed = new Set<string>()
    let failure: unknown = null
    let next = 0
    let answered = 0
    async function worker(): Promise<void> {
      while (next < queue.length) {
        const id = queue[next++] as string
        const response = await api.DELETE('/api/v1/collector/candidates/{id}', { params: { path: { id } } })
        if (response.data) removed.add(id)
        else failure ??= response
        onProgress?.(++answered)
      }
    }
    await Promise.all(Array.from({ length: Math.min(DELETE_CONCURRENCY, queue.length) }, worker))
    const left = new Set(deletingIds.value)
    for (const id of queue) left.delete(id)
    deletingIds.value = left
    candidates.value = candidates.value.filter(item => !removed.has(item.id))
    await refresh()
    // After the refresh, which clears `error` when the lists load.
    if (failure) error.value = responseError(failure)
    return removed.size
  }

  async function deletePackage(id: string): Promise<boolean> {
    const response = await api.DELETE('/api/v1/collector/packages/{id}', { params: { path: { id } } })
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  async function clearCandidates(): Promise<boolean> {
    pending.value = true
    const response = await api.DELETE('/api/v1/collector/candidates')
    pending.value = false
    if (!response.data) error.value = responseError(response)
    await refresh()
    return Boolean(response.data)
  }

  return { deleteCandidate, deleteCandidates, deletePackage, clearCandidates }
}

import type { Ref } from 'vue'

import { api, responseError, resultMessage } from '@/api/client'
import type { PostprocessStep } from '@/api/types'

import { batchError, combinedMessage, inBatches } from '@/utils/bulkBatches'

import { changeBody, type PackageChange } from './transfersShared'

/** What the package actions write back into the transfers store. */
interface PackageActionContext {
  error: Ref<string | null>
  notice: Ref<string | null>
  refresh: () => Promise<void>
}

/**
 * The transfers store's actions on whole packages: change, rename, delete, order, extract.
 * They share the store's `error` and `notice` and refresh it, so the store stays one surface.
 * The bulk routes take at most 500 ids, so a larger selection goes in batches (`inBatches`).
 */
export function usePackageActions({ error, notice, refresh }: PackageActionContext) {
  async function extractPackages(ids: string[]): Promise<boolean> {
    if (!ids.length) return false
    const run = await inBatches(ids, batch => api.POST('/api/v1/packages/extract', { body: { ids: batch } }))
    if (run.data.length) notice.value = combinedMessage(run.data)
    error.value = batchError(run)
    return !run.failure
  }

  /**
   * Post-processes one package although its verification failed (RD-104-04).
   *
   * The one-off counterpart to the `safe_postproc` setting: a broken recovery set beside
   * intact archives is a real case, and the answer to it should not be a global switch
   * somebody then has to remember to put back.
   */
  async function forceExtractPackage(id: string): Promise<boolean> {
    const response = await api.POST('/api/v1/packages/{id}/extract/force', { params: { path: { id } } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    notice.value = resultMessage(response.data)
    error.value = null
    return true
  }

  async function loadPostprocess(id: string): Promise<PostprocessStep[]> {
    const response = await api.GET('/api/v1/packages/{id}/postprocess', { params: { path: { id } } })
    return response.data ?? []
  }

  async function updatePackages(ids: string[], change: PackageChange): Promise<boolean> {
    if (!ids.length) return false
    if (ids.length === 1 && ids[0]) {
      const response = await api.PATCH('/api/v1/packages/{id}', { params: { path: { id: ids[0] } }, body: changeBody(change) })
      if (!response.data) {
        error.value = responseError(response)
        return false
      }
      error.value = null
      await refresh()
      return true
    }
    const run = await inBatches(ids, batch => api.POST('/api/v1/packages/bulk', { body: { ids: batch, ...changeBody(change) } }))
    if (run.data.length) await refresh()
    error.value = batchError(run)
    return !run.failure
  }

  /**
   * Renames a package **and the folder its files live in** (RD-106-13).
   *
   * Separate from `updatePackages`, which changes the label alone: this one moves data, and a
   * name that is already taken comes back as an error instead of being worked around.
   */
  async function renamePackageFolder(id: string, name: string): Promise<boolean> {
    const response = await api.POST('/api/v1/packages/{id}/folder', { params: { path: { id } }, body: { name } })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    await refresh()
    return true
  }

  /**
   * Removes whole packages.
   *
   * `force` is what turns this into a destructive action: without it the server refuses a
   * package that is still running, waiting, seeding or being post-processed instead of
   * cancelling its files and dropping what they had already written. Only pass it when the
   * person was told that is what happens.
   */
  async function deletePackages(ids: string[], force = false): Promise<boolean> {
    if (!ids.length) return false
    const run = await inBatches(ids, batch => api.POST('/api/v1/packages/delete', { body: { ids: batch, force } }))
    if (run.data.length) notice.value = combinedMessage(run.data)
    error.value = batchError(run)
    await refresh()
    return !run.failure
  }

  async function reorderPackages(ids: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/packages/reorder', { body: { ids } })
    if (!response.data) {
      // The refresh has to come first: on success it clears `error`, so a message set before it
      // would be wiped and the refusal would read as a saved order.
      await refresh()
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  /**
   * Writes the manual file order inside one package.
   *
   * `ids` has to be exactly that package's files, each of them once — the server refuses
   * anything else, because it hands out the positions 1..n from this list.
   */
  async function reorderDownloads(packageId: string, ids: string[]): Promise<boolean> {
    const response = await api.POST('/api/v1/downloads/reorder', { body: { package_id: packageId, ids } })
    if (!response.data) {
      await refresh()
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  return {
    extractPackages,
    forceExtractPackage,
    loadPostprocess,
    updatePackages,
    renamePackageFolder,
    deletePackages,
    reorderPackages,
    reorderDownloads
  }
}

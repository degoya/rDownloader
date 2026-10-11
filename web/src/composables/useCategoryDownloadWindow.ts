import { ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { Category, DownloadWindow } from '@/api/types'
import { draftOf, sameWindow, windowBody, type DownloadWindowDraft } from '@/utils/downloadWindow'

/** What a save sends: the window wanted, and the one the category had when it was opened. */
export interface WindowChange {
  window: DownloadWindow | null
  before: DownloadWindow | null
}

/**
 * The category editor's download window (RD-1240-30). It rides a route of its own, which needs
 * the id a create has only just produced, so it is read before the category is saved — a save
 * empties the form — and sent afterwards, only when it changed. The create and update routes
 * answer without it, so the list's row is given the window that is stored.
 */
export function useCategoryDownloadWindow() {
  const draft = ref<DownloadWindowDraft>(draftOf(null))
  const stored = ref<DownloadWindow | null>(null)

  function reset(): void {
    draft.value = draftOf(null)
    stored.value = null
  }

  function fill(category: Category): void {
    draft.value = draftOf(category.download_window)
    stored.value = category.download_window ?? null
  }

  /** The change to send once the category is saved; read before the save. */
  function change(): WindowChange {
    return { window: windowBody(draft.value), before: stored.value }
  }

  /** Sends the change to the category `id` and puts it on its row; answers a failure's message. */
  async function save(id: string, wanted: WindowChange, categories: Ref<Category[]>): Promise<string | null> {
    let window = wanted.before
    let failure: string | null = null
    if (!sameWindow(wanted.window, wanted.before)) {
      const response = await api.PUT('/api/v1/categories/{id}/download-window', {
        params: { path: { id } },
        body: { download_window: wanted.window }
      })
      if (response.data) window = response.data.download_window ?? null
      else failure = responseError(response)
    }
    categories.value = categories.value.map(item => item.id === id ? { ...item, download_window: window } : item)
    return failure
  }

  return { draft, reset, fill, change, save }
}

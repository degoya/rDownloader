import { useOverlay } from '@nuxt/ui/composables'

import type { Category, DownloadPriority } from '@/api/types'
import NzbImportModal from '@/components/NzbImportModal.vue'
import { setFileImportModalOpen } from '@/composables/nzbImportRequest'

export interface FileImportEntry {
  file: File
  /** Package name without the `.nzb` suffix; empty = keep the uploaded file name. */
  name: string
}

export interface FileImportInput {
  entries: FileImportEntry[]
  categoryId: string | null
  priority: DownloadPriority
}

export function useFileImportModal(): (categories: Category[], initialFiles?: File[]) => Promise<FileImportInput | null> {
  const overlay = useOverlay()
  const modal = overlay.create(NzbImportModal)
  return async (categories: Category[], initialFiles?: File[]): Promise<FileImportInput | null> => {
    setFileImportModalOpen(true)
    try {
      const instance = modal.open(initialFiles ? { categories, initialFiles } : { categories })
      const result = await instance.result
      return result && typeof result === 'object' && 'entries' in result ? result as FileImportInput : null
    } finally {
      setFileImportModalOpen(false)
    }
  }
}

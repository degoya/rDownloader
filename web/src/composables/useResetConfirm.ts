import { useOverlay } from '@nuxt/ui/composables'

import ResetConfirmModal from '@/components/ResetConfirmModal.vue'

export interface ResetConfirmResult {
  confirmed: boolean
  /** Whether the finished payload should be deleted along with the partial data. */
  deleteFiles: boolean
}

/**
 * Confirms a reset, naming the files it applies to.
 *
 * Separate from `useConfirm` because this dialog answers two questions at once — whether to go
 * ahead, and whether a finished file goes with it — which a plain boolean cannot carry.
 */
export function useResetConfirm(): (files: string[], hasCompleted: boolean) => Promise<ResetConfirmResult> {
  const overlay = useOverlay()
  const modal = overlay.create(ResetConfirmModal)

  return async (files: string[], hasCompleted: boolean): Promise<ResetConfirmResult> => {
    const instance = modal.open({ files, hasCompleted })
    const result = await instance.result as ResetConfirmResult | undefined
    return result ?? { confirmed: false, deleteFiles: false }
  }
}

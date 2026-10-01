import { useOverlay } from '@nuxt/ui/composables'

import ClearEverythingModal from '@/components/ClearEverythingModal.vue'

export interface ClearEverythingResult {
  confirmed: boolean
  /** Whether what unfinished files wrote beside their target goes as well. */
  deletePartial: boolean
}

/**
 * Confirms "clear the entire list" (RD-180-21), saying how many packages go and how many of
 * them are still working.
 *
 * Separate from `useConfirm` for the reason `useResetConfirm` is: the answer carries a second
 * decision, whether the partial data goes too, which a plain boolean cannot.
 */
export function useClearEverythingConfirm(): (packages: number, active: number) => Promise<ClearEverythingResult> {
  const overlay = useOverlay()
  const modal = overlay.create(ClearEverythingModal)

  return async (packages: number, active: number): Promise<ClearEverythingResult> => {
    const instance = modal.open({ packages, active })
    const result = await instance.result as ClearEverythingResult | undefined
    return result ?? { confirmed: false, deletePartial: false }
  }
}

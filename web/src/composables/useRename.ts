import { useOverlay } from '@nuxt/ui/composables'

import RenameModal from '@/components/RenameModal.vue'

export interface RenameOptions {
  title: string
  label: string
  value: string
  description?: string
  maxLength?: number
}

/** Opens the rename dialog and resolves with the new name, or null when unchanged/cancelled. */
export function useRename(): (options: RenameOptions) => Promise<string | null> {
  const overlay = useOverlay()
  const modal = overlay.create(RenameModal)

  return async (options: RenameOptions): Promise<string | null> => {
    const instance = modal.open(options)
    const result = await instance.result
    return typeof result === 'string' ? result : null
  }
}

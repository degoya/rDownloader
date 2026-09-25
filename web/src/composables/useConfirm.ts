import { useOverlay } from '@nuxt/ui/composables'

import ConfirmModal from '@/components/ConfirmModal.vue'

export interface ConfirmOptions {
  title: string
  description: string
  confirmLabel?: string
  confirmIcon?: string
  destructive?: boolean
}

export function useConfirm(): (options: ConfirmOptions) => Promise<boolean> {
  const overlay = useOverlay()
  const modal = overlay.create(ConfirmModal)

  return async (options: ConfirmOptions): Promise<boolean> => {
    const instance = modal.open(options)
    return (await instance.result) === true
  }
}

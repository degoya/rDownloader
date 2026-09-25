import { useOverlay } from '@nuxt/ui/composables'

import CollectorIntakeModal from '@/components/CollectorIntakeModal.vue'

export interface IntakeResult {
  text: string
  packageName: string | null
  password: string | null
}

export function useIntakeModal(): () => Promise<IntakeResult | null> {
  const overlay = useOverlay()
  const modal = overlay.create(CollectorIntakeModal)
  return async (): Promise<IntakeResult | null> => {
    const instance = modal.open({})
    const result = await instance.result
    return result && typeof result === 'object' && 'text' in result ? result as IntakeResult : null
  }
}

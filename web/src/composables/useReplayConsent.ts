import { useOverlay } from '@nuxt/ui/composables'

import type { ReplayPreview } from '@/api/types'
import CollectorReplayConsentModal from '@/components/CollectorReplayConsentModal.vue'

export interface ReplayConsentResult {
  approvedOrigins: string[]
}

/**
 * Shows what a captured request would send, and returns the origins the user approved.
 *
 * Resolves `null` when the dialog is dismissed, which the caller must treat as a refusal:
 * the server rejects the enqueue anyway, but nothing should be attempted without an answer.
 */
export function useReplayConsent(): (
  preview: ReplayPreview,
  readonly?: boolean
) => Promise<ReplayConsentResult | null> {
  const overlay = useOverlay()
  const modal = overlay.create(CollectorReplayConsentModal)

  return async (preview, readonly = false) => {
    const instance = modal.open({ preview, readonly })
    const result = await instance.result
    return result && typeof result === 'object' && 'approvedOrigins' in result
      ? (result as ReplayConsentResult)
      : null
  }
}

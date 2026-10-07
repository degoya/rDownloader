import { useOverlay } from '@nuxt/ui/composables'

import RegexEditorModal from '@/components/routing/RegexEditorModal.vue'

/** The name a pattern is tried on in the editor's tester; a category rule's `name_target`. */
export type RegexTarget = 'file' | 'package' | 'either'

/**
 * Opens the visual regex editor seeded with the current pattern; resolves with the
 * edited pattern (`{ pattern: null }` clears it) or null when cancelled.
 */
export function useRegexEditor(): (pattern: string | null, target?: RegexTarget) => Promise<{ pattern: string | null } | null> {
  const overlay = useOverlay()
  const modal = overlay.create(RegexEditorModal)

  return async (pattern, target = 'file'): Promise<{ pattern: string | null } | null> => {
    const instance = modal.open({ pattern, target })
    const result = await instance.result
    return result && typeof result === 'object' && 'pattern' in result ? result as { pattern: string | null } : null
  }
}

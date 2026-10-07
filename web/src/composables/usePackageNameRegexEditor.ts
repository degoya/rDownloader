import { useOverlay } from '@nuxt/ui/composables'

import type { PackageNameRegex } from '@/api/types'
import RegexEditorModal from '@/components/routing/RegexEditorModal.vue'

/**
 * Opens the regex editor in its replacement mode for one package-name regex rule (RD-1140-05);
 * resolves with the edited pair, or null when it was cancelled or the pattern was cleared.
 */
export function usePackageNameRegexEditor(): (pair: PackageNameRegex | null) => Promise<PackageNameRegex | null> {
  const overlay = useOverlay()
  const modal = overlay.create(RegexEditorModal)

  return async (pair): Promise<PackageNameRegex | null> => {
    const instance = modal.open({ pattern: pair?.pattern ?? null, replacement: pair?.replacement ?? '' })
    const result: unknown = await instance.result
    if (!result || typeof result !== 'object') return null
    const { pattern, replacement } = result as { pattern?: string | null, replacement?: string }
    return pattern ? { pattern, replacement: replacement ?? '' } : null
  }
}

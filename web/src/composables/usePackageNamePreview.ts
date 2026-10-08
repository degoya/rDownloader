import { watchDebounced } from '@vueuse/core'
import { onMounted, ref } from 'vue'

import { api } from '@/api/client'
import type { PackageNamePreviewResponse, PackageNameRegex, PackageNameRulesOverride } from '@/api/types'
import { translateServerMessage, type ServerMessage } from '@/i18n/server'
import { PACKAGE_NAME_EXAMPLE } from '@/utils/packageNameRules'

/**
 * What the package-name rules of a form make of the example name (RD-1140-05), saved or not.
 *
 * The service tidies — the same function every new package goes through — so the preview never
 * drifts from what a package gets. Switches the form leaves at `null`, and a `null` regex list,
 * take the saved global setting there, which is what a category editor shows. `naming`
 * answering `null` asks nothing and shows no example. Only the newest answer lands; a list the
 * service refuses shows its message instead.
 */
interface PackageNamePreviewInput {
  rules: PackageNameRulesOverride
  regex: PackageNameRegex[] | null
}

export function usePackageNamePreview(naming: () => PackageNamePreviewInput | null) {
  const preview = ref<PackageNamePreviewResponse | null>(null)
  const refusal = ref<string | null>(null)
  let sequence = 0

  async function evaluate(): Promise<void> {
    const id = ++sequence
    const current = naming()
    if (current === null) {
      preview.value = null
      refusal.value = null
      return
    }
    let answer: PackageNamePreviewResponse | null = null
    let refused: string | null = null
    try {
      const response = await api.POST('/api/v1/postprocess/package-name-preview', {
        body: { name: PACKAGE_NAME_EXAMPLE, rules: current.rules, regex: current.regex }
      })
      answer = response.data ?? null
      if (!response.data && response.error) refused = translateServerMessage(response.error as ServerMessage)
    } catch {
      // An unreachable service costs the example, not the form around it.
    }
    if (id !== sequence) return
    preview.value = answer
    refusal.value = refused
  }

  onMounted(() => void evaluate())
  watchDebounced(naming, () => void evaluate(), { debounce: 250, deep: true })

  return { example: PACKAGE_NAME_EXAMPLE, preview, refusal }
}

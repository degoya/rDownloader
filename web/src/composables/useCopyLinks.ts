import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { useCopy } from '@/composables/useCopy'

/**
 * "Copy links" of the queue and the LinkGrabber rows (RD-190-21): the addresses one per line,
 * the way a link list is pasted anywhere else, and a toast that says how many went.
 *
 * A copy that did not happen gets `useCopy`'s error toast instead, so nobody pastes an old
 * clipboard believing it holds the links.
 */
export function useCopyLinks(): (links: readonly string[]) => Promise<boolean> {
  const copy = useCopy()
  const toast = useToast()
  const { t } = useI18n()
  return async (links: readonly string[]): Promise<boolean> => {
    const unique = [...new Set(links.filter(Boolean))]
    if (!unique.length) return false
    const copied = await copy(unique.join('\n'))
    if (copied) {
      toast.add({
        title: t('common.copy.links_copied', { count: unique.length }, unique.length),
        color: 'success',
        icon: 'i-lucide-clipboard-check'
      })
    }
    return copied
  }
}
